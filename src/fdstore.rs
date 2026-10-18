//! File descriptor store (systemd's FDSTORE=1 / FileDescriptorStoreMax=),
//! held by the daemon so descriptors survive service restarts. Also keeps a
//! service's listening sockets open across restarts (key "<unit>#sockets").
//!
//! Protocol: SOCK_SEQPACKET at RUN_DIR/fdstore, one fd per message (SCM_RIGHTS).
//!   PUT\0<key>\0<name>  +fd   -> "OK" | "ERR <msg>"
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
        let mut cbuf = [0u8; 64];
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

/// Receive one message; returns (data, fds).
pub fn recv(sock: RawFd) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    unsafe {
        let mut buf = vec![0u8; 8192];
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
        let mut cbuf = [0u8; 1024];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cbuf.len() as _;
        let n = libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC);
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        let mut fds = Vec::new();
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c) as *const RawFd;
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        Ok((buf, fds))
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

/// Client: store one fd.
pub fn put(path: &Path, key: &str, name: &str, fd: RawFd) -> Result<(), String> {
    let s = connect(path).ok_or("fd store not reachable")?;
    send(s.as_raw_fd(), format!("PUT\0{key}\0{name}").as_bytes(), Some(fd)).map_err(|e| e.to_string())?;
    let (reply, _) = recv(s.as_raw_fd()).map_err(|e| e.to_string())?;
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
        let Ok((data, mut fds)) = recv(s.as_raw_fd()) else { break };
        if data == b"END" || data.is_empty() {
            break;
        }
        if let (Some(name), Some(fd)) = (data.strip_prefix(b"FD\0"), fds.pop()) {
            out.push((String::from_utf8_lossy(name).into_owned(), fd));
        }
    }
    out
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

/// The daemon side.
#[derive(Clone, Default)]
pub struct Store {
    inner: Arc<Mutex<HashMap<String, Vec<(String, OwnedFd)>>>>,
}

impl Store {
    pub fn drop_key(&self, key: &str) {
        self.inner.lock().unwrap().remove(key);
    }

    pub fn count(&self, key: &str) -> usize {
        self.inner.lock().unwrap().get(key).map(|v| v.len()).unwrap_or(0)
    }

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
            store.handle(&conn, &limit);
        });
        Ok(())
    }

    fn handle(&self, conn: &OwnedFd, limit: &dyn Fn(&str) -> usize) {
        let Ok((data, mut fds)) = recv(conn.as_raw_fd()) else { return };
        let parts: Vec<String> = data.split(|&b| b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        let reply = |m: &[u8]| {
            let _ = send(conn.as_raw_fd(), m, None);
        };
        match parts.first().map(String::as_str) {
            Some("PUT") if parts.len() == 3 => {
                let Some(fd) = fds.pop() else { return reply(b"ERR no fd") };
                let max = limit(&parts[1]);
                let mut map = self.inner.lock().unwrap();
                let list = map.entry(parts[1].clone()).or_default();
                if list.len() >= max {
                    return reply(b"ERR fd store full");
                }
                list.push((parts[2].clone(), fd));
                reply(b"OK");
            }
            Some("GET") if parts.len() == 2 => {
                let dups: Vec<(String, RawFd)> = self
                    .inner
                    .lock()
                    .unwrap()
                    .get(&parts[1])
                    .map(|v| v.iter().map(|(n, f)| (n.clone(), f.as_raw_fd())).collect())
                    .unwrap_or_default();
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
