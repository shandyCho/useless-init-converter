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
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::net::{self, sockopt};

pub fn send(sock: impl AsFd, data: &[u8], fd: Option<BorrowedFd<'_>>) -> std::io::Result<()> {
    let fds;
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = net::SendAncillaryBuffer::new(&mut space);
    if let Some(fd) = fd {
        fds = [fd];
        let pushed = control.push(net::SendAncillaryMessage::ScmRights(&fds));
        debug_assert!(pushed, "the buffer is sized for one fd");
    }
    net::sendmsg(
        sock,
        &[IoSlice::new(data)],
        &mut control,
        net::SendFlags::NOSIGNAL,
    )?;
    Ok(())
}

/// Receive one message; returns (data, fds, sender credentials). The
/// credentials are only present if the socket has SO_PASSCRED set.
///
/// On libc, not rustix: the kernel reports pid 0 for a sender it has no pid
/// for, and rustix's UCred holds a non-zero Pid, so reading that through
/// rustix would be undefined behaviour.
fn recv(sock: RawFd) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    recv_max(sock, 8192)
}

/// `recv` with a receive buffer of `cap` bytes; a longer datagram is cut.
fn recv_max(
    sock: RawFd,
    cap: usize,
) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    unsafe {
        let mut buf = vec![0u8; cap];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
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
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            } else if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_CREDENTIALS
            {
                cred = Some(std::ptr::read_unaligned(
                    libc::CMSG_DATA(c) as *const libc::ucred
                ));
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
        let mut p = [PollFd::new(fd, PollFlags::empty())];
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let ready = matches!(poll(&mut p, Some(&now)), Ok(n) if n > 0);
        !(ready
            && p[0]
                .revents()
                .intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL))
    });
}

/// Both fds refer to the same open file description (systemd's same_fd():
/// kcmp(), or the inode and flags without it).
fn same_file(a: BorrowedFd<'_>, b: BorrowedFd<'_>) -> bool {
    // rustix has no kcmp().
    const KCMP_FILE: libc::c_int = 0;
    let pid = unsafe { libc::getpid() };
    let r = unsafe {
        libc::syscall(
            libc::SYS_kcmp,
            pid,
            pid,
            KCMP_FILE,
            a.as_raw_fd(),
            b.as_raw_fd(),
        )
    };
    if r >= 0 {
        return r == 0;
    }
    same_inode_and_flags(a, b)
}

fn same_inode_and_flags(a: BorrowedFd<'_>, b: BorrowedFd<'_>) -> bool {
    let stat = |fd| rustix::fs::fstat(fd).ok().map(|st| (st.st_dev, st.st_ino));
    let flags = |fd| rustix::fs::fcntl_getfl(fd).ok();
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
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERSEC,
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    } < 0
    {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..len as usize])
        .trim_end_matches('\0')
        .to_string();
    (!s.is_empty()).then_some(s)
}

/// The peer's uid (SO_PEERCRED); None if the lookup fails. On libc for the
/// same reason as recv(): a peer outside our PID namespace has pid 0.
fn peer_uid(fd: BorrowedFd<'_>) -> Option<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as u32;
    let r = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    // A failed lookup leaves the zeroed ucred, which would read as root.
    (r == 0).then_some(cred.uid)
}

/// The serving side.
#[derive(Clone, Default)]
pub struct Store {
    inner: Arc<Mutex<HashMap<String, Vec<(String, OwnedFd)>>>>,
}

impl Store {
    /// Serve requests in a dedicated thread. `limit(key)` returns the maximum
    /// number of fds for a key (FileDescriptorStoreMax=); 0 rejects.
    pub fn serve(
        &self,
        path: &Path,
        limit: impl Fn(&str) -> usize + Send + 'static,
    ) -> Result<(), String> {
        let p = path.display().to_string();
        if path.as_os_str().len() >= 100 {
            return Err(format!("{p}: path too long"));
        }
        let _ = std::fs::remove_file(path);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let addr = net::SocketAddrUnix::new(path).map_err(|e| format!("{p}: {e}"))?;
        let lfd = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::SEQPACKET,
            net::SocketFlags::CLOEXEC,
            None,
        )
        .map_err(|e| e.to_string())?;
        net::bind(&lfd, &addr)
            .and_then(|()| net::listen(&lfd, 64))
            .map_err(|e| format!("{p}: {e}"))?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        let me = rustix::process::geteuid().as_raw();
        let own = own_label();
        let store = self.clone();
        std::thread::spawn(move || loop {
            let Ok(conn) = net::accept_with(&lfd, net::SocketFlags::CLOEXEC) else {
                // EMFILE and the like stay until something is freed: don't spin.
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            };
            // Only root (or the daemon's own user, in tests) may use the store.
            if !matches!(peer_uid(conn.as_fd()), Some(uid) if uid == 0 || uid == me) {
                continue;
            }
            if !same_domain(conn.as_raw_fd(), own.as_deref()) {
                continue;
            }
            // One thread serves everyone: a client that connects and then
            // neither sends nor reads must not stop the store.
            for kind in [sockopt::Timeout::Recv, sockopt::Timeout::Send] {
                let _ = sockopt::set_socket_timeout(
                    &conn,
                    kind,
                    Some(std::time::Duration::from_secs(2)),
                );
            }
            store.handle(&conn, &limit);
        });
        Ok(())
    }

    fn handle(&self, conn: &OwnedFd, limit: &dyn Fn(&str) -> usize) {
        let Ok((data, mut fds, _)) = recv(conn.as_raw_fd()) else {
            return;
        };
        let parts: Vec<String> = data
            .split(|&b| b == 0)
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        let reply = |m: &[u8]| {
            let _ = send(conn, m, None);
        };
        match parts.first().map(String::as_str) {
            Some("PUT") if parts.len() == 3 || parts.len() == 4 => {
                let Some(fd) = fds.pop() else {
                    return reply(b"ERR no fd");
                };
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
                if !parts[1].ends_with("#stdio")
                    && list.iter().any(|(_, f)| same_file(f.as_fd(), fd.as_fd()))
                {
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
                // The lock stays held: this thread is the only one that takes it.
                for (n, fd) in map.get(&parts[1]).into_iter().flatten() {
                    let _ = send(conn, format!("FD\0{n}").as_bytes(), Some(fd.as_fd()));
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
    fn a_passed_fd_arrives_usable_and_cloexec() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixDatagram, UnixStream};
        let (a, b) = UnixDatagram::pair().unwrap();
        let (mut payload, other) = UnixStream::pair().unwrap();
        send(&a, b"hello", Some(other.as_fd())).unwrap();
        drop(other);
        let (data, mut fds, cred) = recv(b.as_raw_fd()).unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(fds.len(), 1);
        assert!(cred.is_none(), "no credentials without SO_PASSCRED");
        let got = fds.pop().unwrap();
        let flags = unsafe { libc::fcntl(got.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        let mut got = std::fs::File::from(got);
        got.write_all(b"x").unwrap();
        let mut one = [0u8; 1];
        payload.read_exact(&mut one).unwrap();
        assert_eq!(&one, b"x");
        // No fd: just the data.
        send(&a, b"plain", None).unwrap();
        let (data, fds, _) = recv(b.as_raw_fd()).unwrap();
        assert_eq!((data.as_slice(), fds.len()), (&b"plain"[..], 0));
    }

    #[test]
    fn sending_to_a_closed_peer_is_an_error() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(b);
        let e = send(&a, b"x", None).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_long_message_is_cut_to_the_buffer() {
        let (a, b) = std::os::unix::net::UnixDatagram::pair().unwrap();
        send(&a, b"0123456789", None).unwrap();
        let (data, _, _) = recv_max(b.as_raw_fd(), 4).unwrap();
        assert_eq!(data, b"0123");
    }

    /// The kernel reports pid 0 for a sender it has no pid for: here a
    /// datagram sent before the receiver set SO_PASSCRED. This is why recv()
    /// stays on libc: rustix's UCred holds a non-zero Pid.
    #[test]
    fn credentials_with_pid_zero_are_read() {
        use std::os::unix::net::UnixDatagram;
        let (a, b) = UnixDatagram::pair().unwrap();
        a.send(b"early").unwrap();
        let on: libc::c_int = 1;
        let r = unsafe {
            libc::setsockopt(
                b.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                &on as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as u32,
            )
        };
        assert_eq!(r, 0);
        a.send(b"late").unwrap();
        let (data, _, cred) = recv(b.as_raw_fd()).unwrap();
        assert_eq!((data.as_slice(), cred.unwrap().pid), (&b"early"[..], 0));
        let (data, _, cred) = recv(b.as_raw_fd()).unwrap();
        assert_eq!(data, b"late");
        assert_eq!(cred.unwrap().pid, std::process::id() as i32);
    }

    /// same_file()'s fallback, for kernels without kcmp().
    #[test]
    fn same_inode_and_flags_as_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "uinitconverter-fdstore-same-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (one, two) = (dir.join("one"), dir.join("two"));
        std::fs::write(&one, "").unwrap();
        std::fs::write(&two, "").unwrap();
        let a = std::fs::File::open(&one).unwrap();
        let again = a.try_clone().unwrap();
        let other = std::fs::File::open(&two).unwrap();
        let writing = std::fs::OpenOptions::new().write(true).open(&one).unwrap();
        assert!(same_inode_and_flags(a.as_fd(), again.as_fd()));
        assert!(!same_inode_and_flags(a.as_fd(), other.as_fd()));
        assert!(!same_inode_and_flags(a.as_fd(), writing.as_fd()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serving_fails_cleanly() {
        let dir = std::env::temp_dir().join(format!(
            "uinitconverter-fdstore-fail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = dir.join("x".repeat(120));
        assert!(Store::default().serve(&long, |_| 0).is_err());
        // A directory is in the way of the socket.
        let blocked = dir.join("sub");
        std::fs::create_dir_all(&blocked).unwrap();
        assert!(Store::default().serve(&blocked, |_| 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_stores_own_domain() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        // Without SELinux everyone passes (root-only is checked before).
        assert!(same_domain(a.as_raw_fd(), None));
        assert!(!same_domain(
            a.as_raw_fd(),
            Some("system_u:system_r:other_t:s0")
        ));
        if let Some(own) = own_label() {
            assert!(same_domain(a.as_raw_fd(), Some(&own)));
        }
    }
}
