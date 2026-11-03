//! initconvert-fdstore: holds file descriptors for services (systemd's
//! FDSTORE=1 and the listening sockets of socket-activated services), so
//! they survive a restart of the service and of the service manager.
//!
//! Standalone: reads no configuration and no unit files. The client says
//! how many fds a key may hold. Only root and the store's own user may use
//! it.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::process::exit;

mod store;

const USAGE: &str = "\
usage: initconvert-fdstore [--socket PATH] [--ready-fd N]

  --socket PATH   where to listen (default /run/uinitconverter/fdstore)
  --ready-fd N    write a newline to fd N once listening (s6 readiness)
";

fn die(msg: &str) -> ! {
    eprintln!("initconvert-fdstore: {msg}");
    exit(1)
}

fn main() {
    let mut socket = PathBuf::from("/run/uinitconverter/fdstore");
    let mut ready_fd: Option<RawFd> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .unwrap_or_else(|| die(&format!("{a} needs a value")))
        };
        match a.as_str() {
            "--socket" => socket = PathBuf::from(val()),
            "--ready-fd" => {
                ready_fd = Some(
                    val()
                        .parse()
                        .unwrap_or_else(|_| die("--ready-fd needs a number")),
                )
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                exit(0);
            }
            o => die(&format!("unknown option {o}")),
        }
    }
    let st = store::Store::default();
    if let Err(e) = st.serve(&socket, |key| store::key_limit(key).unwrap_or(0)) {
        die(&e);
    }
    if let Some(fd) = ready_fd {
        unsafe {
            libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(fd);
        }
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::store::{self, Store};
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::path::Path;

    fn connect(path: &Path) -> OwnedFd {
        use rustix::net;
        let fd = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::SEQPACKET,
            net::SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        net::connect(&fd, &net::SocketAddrUnix::new(path).unwrap()).unwrap();
        fd
    }

    fn request(path: &Path, msg: &str, fd: Option<BorrowedFd<'_>>) -> Vec<Vec<u8>> {
        let s = connect(path);
        store::send(&s, msg.as_bytes(), fd).unwrap();
        let mut out = Vec::new();
        loop {
            let mut buf = [0u8; 256];
            let n = rustix::io::read(&s, &mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            let data = buf[..n].to_vec();
            let more = data.starts_with(b"FD\0");
            out.push(data);
            if !more {
                break;
            }
        }
        out
    }

    #[test]
    fn the_socket_is_private_and_a_silent_client_doesnt_stop_the_store() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "uinitconverter-fdstore-silent-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("s");
        Store::default().serve(&sock, |_| 0).unwrap();
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let silent = connect(&sock);
        let start = std::time::Instant::now();
        assert_eq!(request(&sock, "GET\0nothing.service", None), [b"END"]);
        let waited = start.elapsed();
        assert!(waited < std::time::Duration::from_secs(5), "{waited:?}");
        drop(silent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_get_respects_client_max() {
        let dir =
            std::env::temp_dir().join(format!("uinitconverter-fdstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("s");
        Store::default()
            .serve(&sock, |k| store::key_limit(k).unwrap_or(0))
            .unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let fd = Some(file.as_fd());

        // A unit's key without <max>: the store knows no unit files, so 0.
        assert_eq!(
            request(&sock, "PUT\0a.service\0x", fd)[0],
            b"ERR fd store full"
        );
        // With <max> from the client.
        assert_eq!(request(&sock, "PUT\0a.service\0x\x001", fd)[0], b"OK");
        // The same open file again is stored once (systemd's same_fd()).
        assert_eq!(request(&sock, "PUT\0a.service\0x\x001", fd)[0], b"OK");
        let other = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(
            request(&sock, "PUT\0a.service\0y\x001", Some(other.as_fd()))[0],
            b"ERR fd store full"
        );
        // Held sockets have their own limit.
        assert_eq!(request(&sock, "PUT\0a.service#sockets\0s", fd)[0], b"OK");

        let got = request(&sock, "GET\0a.service", None);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], b"FD\0x");
        assert_eq!(got[1], b"END");
        assert_eq!(request(&sock, "DEL\0a.service", None)[0], b"OK");
        assert_eq!(request(&sock, "GET\0a.service", None)[0], b"END");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hung_up_fds_go() {
        let dir =
            std::env::temp_dir().join(format!("uinitconverter-fdstore-hup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("s");
        Store::default()
            .serve(&sock, |k| store::key_limit(k).unwrap_or(0))
            .unwrap();
        // A journald stream whose other end is closed: dropped, as systemd
        // drops FDSTORE= fds on POLLHUP, so the key has room again.
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(
            request(&sock, "PUT\0j.service\0stored\x001", Some(a.as_fd()))[0],
            b"OK"
        );
        drop(b);
        let file = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(
            request(&sock, "PUT\0j.service\0stored\x001", Some(file.as_fd()))[0],
            b"OK"
        );
        assert_eq!(request(&sock, "GET\0j.service", None).len(), 2);
        // Held sockets and stdio ("#" keys) stay.
        let (c, d) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(
            request(&sock, "PUT\0j.service#stdio\0stdin", Some(c.as_fd()))[0],
            b"OK"
        );
        drop(d);
        assert_eq!(request(&sock, "GET\0j.service#stdio", None).len(), 2);
        // One socket as both stdin and stdout keeps both names.
        let (e, _f) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(
            request(&sock, "PUT\0k.service#stdio\0stdin", Some(e.as_fd()))[0],
            b"OK"
        );
        assert_eq!(
            request(&sock, "PUT\0k.service#stdio\0stdout", Some(e.as_fd()))[0],
            b"OK"
        );
        assert_eq!(request(&sock, "GET\0k.service#stdio", None).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
