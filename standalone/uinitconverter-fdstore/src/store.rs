//! The store itself: a SOCK_SEQPACKET socket that holds fds by key.
//!
//! Protocol, one fd per message (SCM_RIGHTS):
//!   PUT\0<key>\0<name>\0<max>  +fd   -> "OK" | "ERR <msg>"
//!     <max>: how many fds the key may hold (the service's
//!     FileDescriptorStoreMax=). Without it only the "#sockets" and
//!     "#stdio" keys are accepted, with their fixed limits.
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
fn recv(sock: RawFd) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    recv_max(sock, 8192)
}

/// `recv` with a receive buffer of `cap` bytes; a longer datagram is cut.
fn recv_max(sock: RawFd, cap: usize) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
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

/// Under SELinux only peers in the store's own domain may use it. Its
/// clients are the service manager's own processes (uinitconverterd and the
/// runner, init_t on the s6 boot), and it holds every service's
/// descriptors; root alone is not enough, as the policy lets many confined
/// root daemons connect to init_t's sockets. Keep in step with
/// crates/uinitconverter-base/src/fdstore.rs (no shared code).
fn same_domain(fd: RawFd, own: Option<&str>) -> bool {
    let Some(own) = own else { return true };
    peer_label(fd).as_deref() == Some(own)
}

/// Our SELinux context; None without SELinux (no selinuxfs).
fn own_label() -> Option<String> {
    if !Path::new("/sys/fs/selinux/enforce").exists() {
        return None;
    }
    let s = std::fs::read_to_string("/proc/self/attr/current").ok()?;
    let s = s.trim_end_matches(['\0', '\n']).to_string();
    (!s.is_empty()).then_some(s)
}

/// The peer's SELinux context (SO_PEERSEC).
fn peer_label(fd: RawFd) -> Option<String> {
    let mut buf = [0u8; 4096];
    let mut len = buf.len() as libc::socklen_t;
    if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERSEC, buf.as_mut_ptr().cast(), &mut len) } < 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..len as usize]).trim_end_matches('\0').to_string();
    (!s.is_empty()).then_some(s)
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
        let own = own_label();
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
            // A failed lookup leaves the zeroed ucred, which would read as root.
            let r = unsafe {
                libc::getsockopt(c, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
            };
            if r < 0 || (cred.uid != 0 && cred.uid != me) {
                continue;
            }
            if !same_domain(c, own.as_deref()) {
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
                // crates/uinitconverter-base/src/fdstore.rs (no shared code).
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
    fn only_the_stores_own_domain() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        // Without SELinux everyone passes (root-only is checked before).
        assert!(same_domain(a.as_raw_fd(), None));
        assert!(!same_domain(a.as_raw_fd(), Some("system_u:system_r:other_t:s0")));
        if let Some(own) = own_label() {
            assert!(same_domain(a.as_raw_fd(), Some(&own)));
        }
    }
}
