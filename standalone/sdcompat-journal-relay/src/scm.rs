//! Receiving a datagram together with its fds (SCM_RIGHTS) and the
//! sender's credentials (SCM_CREDENTIALS).

use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// Receive one message into a buffer of `cap` bytes (a longer datagram is
/// cut); returns (data, fds, sender credentials if SO_PASSCRED is set).
pub fn recv_max(sock: RawFd, cap: usize) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>, Option<libc::ucred>)> {
    unsafe {
        let mut buf = vec![0u8; cap];
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
