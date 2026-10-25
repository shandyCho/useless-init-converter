//! File descriptor store (systemd's FDSTORE=1 / FileDescriptorStoreMax=),
//! so descriptors survive service restarts. Also keeps a service's
//! listening sockets open across restarts (key "<unit>#sockets").
//!
//! Served by the standalone sdcompat-fdstore, its own s6 service, so the
//! descriptors also survive a restart of sdcompatd; the daemon serves it
//! itself (Store) when that service is not running.
//!
//! Protocol: SOCK_SEQPACKET at RUN_DIR/fdstore, one fd per message (SCM_RIGHTS).
//!   PUT\0<key>\0<name>\0<max>  +fd   -> "OK" | "ERR <msg>"
//!     <max>: how many fds the key may hold (FileDescriptorStoreMax=); the
//!     client sends it so the server needs no unit files. Without it the
//!     server falls back to its own limit for the key.
//!   GET\0<key>                -> "FD\0<name>" +fd ... then "END"
//!   DEL\0<key>[\0<name>]      -> "OK"

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

pub fn send(sock: RawFd, data: &[u8], fd: Option<RawFd>) -> std::io::Result<()> {
    unsafe {
        let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut libc::c_void, iov_len: data.len() };
        // u64s: aligned for cmsghdr, as a byte array is not.
        let mut cbuf = [0u64; 8];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut RawFd, fd);
        }
        if libc::sendmsg(sock, &msg, libc::MSG_NOSIGNAL) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Receive one message; returns (data, fds, sender credentials). The
/// credentials are only present if the socket has SO_PASSCRED set.
pub fn recv(sock: RawFd) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    recv_max(sock, 8192)
}

/// `recv` with a receive buffer of `cap` bytes; a longer datagram is cut.
pub fn recv_max(sock: RawFd, cap: usize) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    unsafe {
        let mut buf = vec![0u8; cap];
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
        // u64s: aligned for cmsghdr, as a byte array is not.
        let mut cbuf = [0u64; 128];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = std::mem::size_of_val(&cbuf) as _;
        let n = libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC);
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        let mut fds = Vec::new();
        let mut cred = None;
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c) as *const RawFd;
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            } else if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_CREDENTIALS {
                cred = Some(std::ptr::read_unaligned(libc::CMSG_DATA(c) as *const libc::ucred));
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        Ok((buf, fds, cred))
    }
}

fn connect(path: &Path) -> Option<OwnedFd> {
    let p = path.display().to_string();
    if p.len() >= 100 {
        return None;
    }
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return None;
        }
        let owned = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as u16;
        for (i, b) in p.bytes().enumerate() {
            addr.sun_path[i] = b as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + p.len() + 1) as u32;
        if libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) < 0 {
            return None;
        }
        Some(owned)
    }
}

/// Client: store one fd; `max` is how many the key may hold.
pub fn put(path: &Path, key: &str, name: &str, fd: RawFd, max: usize) -> Result<(), String> {
    // NUL separates the fields: "x\0<n>" as a name would turn the retry
    // below into a request with the client's own <max>.
    if key.contains('\0') || name.contains('\0') {
        return Err("invalid fd store key or name".into());
    }
    match put_msg(path, &format!("PUT\0{key}\0{name}\0{max}"), fd) {
        // A store from before <max> was added (still running until the next
        // boot after an upgrade) reads the limit from the unit file itself.
        Err(e) if e == "ERR bad request" => put_msg(path, &format!("PUT\0{key}\0{name}"), fd),
        r => r,
    }
}

fn put_msg(path: &Path, msg: &str, fd: RawFd) -> Result<(), String> {
    let s = connect(path).ok_or("fd store not reachable")?;
    send(s.as_raw_fd(), msg.as_bytes(), Some(fd)).map_err(|e| e.to_string())?;
    let (reply, _, _) = recv(s.as_raw_fd()).map_err(|e| e.to_string())?;
    if reply == b"OK" {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&reply).into_owned())
    }
}

/// Client: fetch every fd stored under `key` (the store keeps its copies).
pub fn get(path: &Path, key: &str) -> Vec<(String, OwnedFd)> {
    let Some(s) = connect(path) else { return Vec::new() };
    if send(s.as_raw_fd(), format!("GET\0{key}").as_bytes(), None).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        let Ok((data, mut fds, _)) = recv(s.as_raw_fd()) else { break };
        if data == b"END" || data.is_empty() {
            break;
        }
        if let (Some(name), Some(fd)) = (data.strip_prefix(b"FD\0"), fds.pop()) {
            out.push((String::from_utf8_lossy(name).into_owned(), fd));
        }
    }
    out
}

/// The limit for keys that are not a unit's FDSTORE=: held sockets
/// ("<unit>#sockets") and a transient unit's stdio ("<unit>#stdio").
pub fn key_limit(key: &str) -> Option<usize> {
    if key.ends_with("#sockets") {
        Some(SOCKETS_MAX)
    } else if key.ends_with("#stdio") {
        Some(3)
    } else {
        None
    }
}

/// How many listening sockets a unit's "#sockets" key may hold.
pub const SOCKETS_MAX: usize = 1024;

/// Whether a store answers at `path`.
pub fn is_served(path: &Path) -> bool {
    connect(path).is_some()
}

/// FileDescriptorStoreMax= of the unit a key belongs to; held sockets (key
/// "<unit>#sockets") are not limited by it.
pub fn limit(cfg: &crate::config::Config, key: &str) -> usize {
    if let Some(n) = key_limit(key) {
        return n;
    }
    crate::loader::Loader::new(cfg)
        .load(key)
        .get("Service", "FileDescriptorStoreMax")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn del(path: &Path, key: &str, name: Option<&str>) {
    if let Some(s) = connect(path) {
        let msg = match name {
            Some(n) => format!("DEL\0{key}\0{n}"),
            None => format!("DEL\0{key}"),
        };
        let _ = send(s.as_raw_fd(), msg.as_bytes(), None);
        let _ = recv(s.as_raw_fd());
    }
}

/// Drop stored fds whose other end hung up, as systemd does with FDSTORE=
/// fds (POLLHUP/POLLERR): journald stores one per stdout stream and never
/// removes it, so the key would grow until the store runs out of fds.
/// Held sockets and stdio ("#" keys) stay: a stdin pipe may be at EOF
/// before its service takes it.
fn prune_hung_up(key: &str, list: &mut Vec<(String, OwnedFd)>) {
    if key.contains('#') {
        return;
    }
    list.retain(|(_, fd)| {
        let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: 0, revents: 0 };
        let r = unsafe { libc::poll(&mut p, 1, 0) };
        !(r > 0 && p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0)
    });
}

/// Both fds refer to the same open file description (systemd's same_fd():
/// kcmp(), or the inode and flags without it).
fn same_file(a: RawFd, b: RawFd) -> bool {
    const KCMP_FILE: libc::c_int = 0;
    let pid = unsafe { libc::getpid() };
    let r = unsafe { libc::syscall(libc::SYS_kcmp, pid, pid, KCMP_FILE, a, b) };
    if r >= 0 {
        return r == 0;
    }
    let stat = |fd: RawFd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::fstat(fd, &mut st) } == 0).then_some((st.st_dev, st.st_ino))
    };
    let flags = |fd: RawFd| unsafe { libc::fcntl(fd, libc::F_GETFL) };
    stat(a).is_some() && stat(a) == stat(b) && flags(a) == flags(b)
}

/// The serving side.
#[derive(Clone, Default)]
pub struct Store {
    inner: Arc<Mutex<HashMap<String, Vec<(String, OwnedFd)>>>>,
}

impl Store {
    /// Serve requests in a dedicated thread. `limit(key)` returns the maximum
    /// number of fds for a key (FileDescriptorStoreMax=); 0 rejects.
    pub fn serve(&self, path: &Path, limit: impl Fn(&str) -> usize + Send + 'static) -> Result<(), String> {
        let p = path.display().to_string();
        if p.len() >= 100 {
            return Err(format!("{p}: path too long"));
        }
        let _ = std::fs::remove_file(path);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let lfd = unsafe {
            let fd = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(errno());
            }
            let mut addr: libc::sockaddr_un = std::mem::zeroed();
            addr.sun_family = libc::AF_UNIX as u16;
            for (i, b) in p.bytes().enumerate() {
                addr.sun_path[i] = b as libc::c_char;
            }
            let len = (std::mem::size_of::<libc::sa_family_t>() + p.len() + 1) as u32;
            if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) < 0 || libc::listen(fd, 64) < 0 {
                return Err(format!("{p}: {}", errno()));
            }
            libc::chmod(std::ffi::CString::new(p.clone()).unwrap().as_ptr(), 0o600);
            OwnedFd::from_raw_fd(fd)
        };
        let me = unsafe { libc::geteuid() };
        let store = self.clone();
        std::thread::spawn(move || loop {
            let c = unsafe { libc::accept4(lfd.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC) };
            if c < 0 {
                // EMFILE and the like stay until something is freed: don't spin.
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            let conn = unsafe { OwnedFd::from_raw_fd(c) };
            // Only root (or the daemon's own user, in tests) may use the store.
            let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::ucred>() as u32;
            unsafe {
                libc::getsockopt(c, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len);
            }
            if cred.uid != 0 && cred.uid != me {
                continue;
            }
            // One thread serves everyone: a client that connects and then
            // neither sends nor reads must not stop the store.
            let tv = libc::timeval { tv_sec: 2, tv_usec: 0 };
            for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
                unsafe {
                    libc::setsockopt(c, libc::SOL_SOCKET, opt, &tv as *const _ as *const libc::c_void, std::mem::size_of::<libc::timeval>() as u32);
                }
            }
            store.handle(&conn, &limit);
        });
        Ok(())
    }

    fn handle(&self, conn: &OwnedFd, limit: &dyn Fn(&str) -> usize) {
        let Ok((data, mut fds, _)) = recv(conn.as_raw_fd()) else { return };
        let parts: Vec<String> = data.split(|&b| b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        let reply = |m: &[u8]| {
            let _ = send(conn.as_raw_fd(), m, None);
        };
        match parts.first().map(String::as_str) {
            Some("PUT") if parts.len() == 3 || parts.len() == 4 => {
                let Some(fd) = fds.pop() else { return reply(b"ERR no fd") };
                let max = match parts.get(3).and_then(|m| m.parse().ok()) {
                    Some(m) => m,
                    None => limit(&parts[1]),
                };
                let mut map = self.inner.lock().unwrap();
                let list = map.entry(parts[1].clone()).or_default();
                prune_hung_up(&parts[1], list);
                // As systemd's service_add_fd_store(): the same open file
                // passed again is stored once. Not for "#stdio": one socket can
                // be both stdin and stdout (sd-bus unixexec: systemd-run --pipe),
                // and each name must get its fd. Keep in step with
                // standalone/sdcompat-fdstore/src/store.rs (no shared code).
                if !parts[1].ends_with("#stdio") && list.iter().any(|(_, f)| same_file(f.as_raw_fd(), fd.as_raw_fd())) {
                    return reply(b"OK");
                }
                if list.len() >= max {
                    return reply(b"ERR fd store full");
                }
                list.push((parts[2].clone(), fd));
                reply(b"OK");
            }
            Some("GET") if parts.len() == 2 => {
                let mut map = self.inner.lock().unwrap();
                if let Some(v) = map.get_mut(&parts[1]) {
                    prune_hung_up(&parts[1], v);
                }
                let dups: Vec<(String, RawFd)> = map
                    .get(&parts[1])
                    .map(|v| v.iter().map(|(n, f)| (n.clone(), f.as_raw_fd())).collect())
                    .unwrap_or_default();
                drop(map);
                for (n, fd) in dups {
                    let _ = send(conn.as_raw_fd(), format!("FD\0{n}").as_bytes(), Some(fd));
                }
                reply(b"END");
            }
            Some("DEL") if parts.len() >= 2 => {
                let mut map = self.inner.lock().unwrap();
                match parts.get(2) {
                    Some(n) => {
                        if let Some(v) = map.get_mut(&parts[1]) {
                            v.retain(|(name, _)| name != n);
                        }
                    }
                    None => {
                        map.remove(&parts[1]);
                    }
                }
                reply(b"OK");
            }
            _ => reply(b"ERR bad request"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_del() {
        let dir = std::env::temp_dir().join(format!("sdcompat-fdstore-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("s");
        // The server's own limit: 1 for "old.service", as a store that reads
        // unit files would; nothing for other keys.
        Store::default().serve(&sock, |k| if k == "old.service" { 1 } else { 0 }).unwrap();
        let files: Vec<std::fs::File> = (0..3).map(|_| std::fs::File::open("/dev/null").unwrap()).collect();
        let (fd, fd2, fd3) = (files[0].as_raw_fd(), files[1].as_raw_fd(), files[2].as_raw_fd());

        // The client's <max> wins over the server's limit.
        put(&sock, "a.service", "x", fd, 2).unwrap();
        put(&sock, "a.service", "y", fd2, 2).unwrap();
        // The same open file again is stored once (systemd's same_fd()).
        put(&sock, "a.service", "x", fd, 2).unwrap();
        assert_eq!(put(&sock, "a.service", "z", fd3, 2).unwrap_err(), "ERR fd store full");
        // Without <max> the server's limit applies.
        assert_eq!(put_msg(&sock, "PUT\0old.service\0x", fd), Ok(()));
        assert_eq!(put_msg(&sock, "PUT\0old.service\0y", fd2).unwrap_err(), "ERR fd store full");

        let names: Vec<String> = get(&sock, "a.service").into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["x", "y"]);
        del(&sock, "a.service", Some("x"));
        assert_eq!(get(&sock, "a.service").len(), 1);
        del(&sock, "a.service", None);
        assert!(get(&sock, "a.service").is_empty());
        // A NUL in the name would add a field (and the client's own <max>).
        assert!(put(&sock, "a.service", "x\099", fd, 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hung_up_fds_go() {
        let dir = std::env::temp_dir().join(format!("sdcompat-fdstore-hup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("s");
        Store::default().serve(&sock, |_| 0).unwrap();
        // A journald stream: once its other end is closed the stored fd
        // hangs up and goes, so the key has room again.
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        put(&sock, "j.service", "stored", a.as_raw_fd(), 1).unwrap();
        drop(b);
        let f = std::fs::File::open("/dev/null").unwrap();
        put(&sock, "j.service", "stored", f.as_raw_fd(), 1).unwrap();
        assert_eq!(get(&sock, "j.service").len(), 1);
        // Held sockets and stdio ("#" keys) stay.
        let (c, d) = std::os::unix::net::UnixStream::pair().unwrap();
        put(&sock, "j.service#stdio", "stdin", c.as_raw_fd(), 3).unwrap();
        drop(d);
        assert_eq!(get(&sock, "j.service#stdio").len(), 1);
        // One socket as both stdin and stdout keeps both names.
        let (e, _f) = std::os::unix::net::UnixStream::pair().unwrap();
        put(&sock, "k.service#stdio", "stdin", e.as_raw_fd(), 3).unwrap();
        put(&sock, "k.service#stdio", "stdout", e.as_raw_fd(), 3).unwrap();
        assert_eq!(get(&sock, "k.service#stdio").len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
