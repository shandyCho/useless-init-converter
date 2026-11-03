//! initconvert-notify: a systemd-notify replacement. Sends one sd_notify
//! message to the socket in $NOTIFY_SOCKET (a path, or @name for the
//! abstract namespace). Works under any service manager that sets it.
//!
//! As systemd-notify (254 and later): the message is sent on behalf of the
//! calling process (its PID in SCM_CREDENTIALS, where the kernel allows
//! that), and then a BARRIER=1 waits until the manager has read it, unless
//! --no-block.

use std::ffi::CString;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::process::exit;

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use rustix::net;
use rustix::pipe::{pipe_with, PipeFlags};
use rustix::process::Pid;
use rustix::time::{clock_gettime, ClockId};

const USAGE: &str = "\
usage: initconvert-notify [OPTIONS] [VAR=VALUE...]
       initconvert-notify --exec [OPTIONS] [VAR=VALUE...] ; COMMAND [ARGS...]
       initconvert-notify --booted

  --ready              READY=1
  --reloading          RELOADING=1 (and MONOTONIC_USEC=)
  --stopping           STOPPING=1
  --status=TEXT        STATUS=TEXT
  --pid[=PID]          MAINPID=PID; PID may be auto or parent (the caller,
                       the default) or self
  --uid=USER           send as USER
  --fd=N               store fd N (FDSTORE=1); may be repeated
  --fdname=NAME        FDNAME=NAME for the stored fds
  --no-block           don't wait for the manager to read the message
  --exec               run COMMAND after sending, in place of this process
  --booted             exit 0 if the system runs a systemd-compatible
                       manager (/run/systemd/system exists), 1 otherwise
  --version            print the version
";

/// How long the barrier waits, as sd_notify_barrier()'s default.
const BARRIER_TIMEOUT_SECS: i64 = 5;

fn errno() -> std::io::Error {
    std::io::Error::last_os_error()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        exit(0);
    }
    if args.iter().any(|a| a == "--version") {
        println!("initconvert-notify {}", env!("CARGO_PKG_VERSION"));
        exit(0);
    }
    exit(run(&args))
}

#[derive(Debug, Default, PartialEq)]
struct Opts {
    lines: Vec<String>,
    /// MAINPID= and the credentials' PID.
    pid: Option<i32>,
    uid: Option<String>,
    fds: Vec<i32>,
    no_block: bool,
    booted: bool,
    exec: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let exec = args.iter().any(|a| a == "--exec");
    let mut it = args.iter();
    let mut fdname = None;
    for a in it.by_ref() {
        match a.as_str() {
            "--exec" => {}
            ";" if exec => break,
            "--ready" => o.lines.push("READY=1".into()),
            "--stopping" => o.lines.push("STOPPING=1".into()),
            "--reloading" => {
                o.lines.push("RELOADING=1".into());
                o.lines.push(format!("MONOTONIC_USEC={}", monotonic_usec()));
            }
            "--no-block" => o.no_block = true,
            "--booted" => o.booted = true,
            "--pid" => o.pid = Some(parent_pid()),
            s if s.starts_with("--pid=") => {
                o.pid = Some(match &s[6..] {
                    "" | "auto" | "parent" => parent_pid(),
                    "self" => std::process::id() as i32,
                    p => p
                        .parse()
                        .ok()
                        .filter(|&p: &i32| p > 0)
                        .ok_or_else(|| format!("bad PID {p}"))?,
                })
            }
            s if s.starts_with("--status=") => o.lines.push(format!("STATUS={}", &s[9..])),
            s if s.starts_with("--uid=") => o.uid = Some(s[6..].to_string()),
            s if s.starts_with("--fd=") => {
                let n: i32 = s[5..].parse().map_err(|_| format!("bad fd {}", &s[5..]))?;
                // A number from the command line: on libc, as rustix only
                // takes fds already known to be open.
                if unsafe { libc::fcntl(n, libc::F_GETFD) } < 0 {
                    return Err(format!("fd {n}: {}", errno()));
                }
                o.fds.push(n);
            }
            s if s.starts_with("--fdname=") => fdname = Some(s[9..].to_string()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if s.contains('=') => o.lines.push(s.to_string()),
            s => return Err(format!("unknown argument {s}")),
        }
    }
    if exec {
        o.exec = it.cloned().collect();
        if o.exec.is_empty() {
            return Err("--exec needs \";\" and a command".into());
        }
    }
    if let Some(p) = o.pid {
        o.lines.push(format!("MAINPID={p}"));
    }
    if !o.fds.is_empty() {
        o.lines.push("FDSTORE=1".into());
        if let Some(n) = fdname {
            o.lines.push(format!("FDNAME={n}"));
        }
    }
    Ok(o)
}

fn parent_pid() -> i32 {
    Pid::as_raw(rustix::process::getppid())
}

fn monotonic_usec() -> u64 {
    let ts = clock_gettime(ClockId::Monotonic);
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1000
}

fn run(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("initconvert-notify: {e}");
            return 1;
        }
    };
    if o.booted {
        return i32::from(!std::path::Path::new("/run/systemd/system").is_dir());
    }
    let Ok(sock) = std::env::var("NOTIFY_SOCKET") else {
        eprintln!("initconvert-notify: NOTIFY_SOCKET not set");
        return 1;
    };
    if let Some(u) = &o.uid {
        if let Err(e) = become_user(u) {
            eprintln!("initconvert-notify: --uid={u}: {e}");
            return 1;
        }
    }
    // On behalf of the caller, unless it is PID 1 (a manager does not notify
    // itself) or another PID was named.
    let source = o.pid.unwrap_or_else(parent_pid);
    let source = (source > 1).then_some(source);
    // SAFETY: parse() found each one open, and nothing here closes them.
    let fds: Vec<BorrowedFd<'_>> = o
        .fds
        .iter()
        .map(|&n| unsafe { BorrowedFd::borrow_raw(n) })
        .collect();
    if let Err(e) = send(&sock, o.lines.join("\n").as_bytes(), source, &fds) {
        eprintln!("initconvert-notify: {e}");
        return 1;
    }
    if !o.no_block {
        if let Err(e) = barrier(&sock, source) {
            // An old manager may not answer barriers: the message went out.
            eprintln!("initconvert-notify: barrier: {e}");
        }
    }
    if !o.exec.is_empty() {
        let c: Vec<CString> = o
            .exec
            .iter()
            .map(|a| CString::new(a.as_str()).unwrap_or_default())
            .collect();
        let mut p: Vec<*const libc::c_char> = c.iter().map(|a| a.as_ptr()).collect();
        p.push(std::ptr::null());
        unsafe { libc::execvp(p[0], p.as_ptr()) };
        eprintln!("initconvert-notify: exec {}: {}", o.exec[0], errno());
        return 1;
    }
    0
}

fn become_user(name: &str) -> Result<(), String> {
    let (uid, gid) = match name.parse::<u32>() {
        Ok(n) => (n, n),
        Err(_) => {
            let c = CString::new(name).map_err(|e| e.to_string())?;
            let pw = unsafe { libc::getpwnam(c.as_ptr()) };
            if pw.is_null() {
                return Err("no such user".into());
            }
            unsafe { ((*pw).pw_uid, (*pw).pw_gid) }
        }
    };
    unsafe {
        if libc::setgroups(0, std::ptr::null()) < 0 && libc::geteuid() == 0 {
            return Err(errno().to_string());
        }
        if libc::setresgid(gid, gid, gid) < 0 || libc::setresuid(uid, uid, uid) < 0 {
            return Err(errno().to_string());
        }
    }
    Ok(())
}

/// One datagram to $NOTIFY_SOCKET, with `fds` and, if `pid` is given,
/// credentials naming it. Naming another process needs privileges: without
/// them the message goes with our own (the kernel's default).
fn send(sock: &str, data: &[u8], pid: Option<i32>, fds: &[BorrowedFd<'_>]) -> std::io::Result<()> {
    let fd = net::socket_with(
        net::AddressFamily::UNIX,
        net::SocketType::DGRAM,
        net::SocketFlags::CLOEXEC,
        None,
    )?;
    let addr = socket_addr(sock)?;
    match pid {
        Some(p) => match sendmsg(&fd, &addr, data, Some(p), fds) {
            Err(e) if e.raw_os_error() == Some(Errno::PERM.raw_os_error()) => {
                sendmsg(&fd, &addr, data, None, fds)
            }
            r => r,
        },
        None => sendmsg(&fd, &addr, data, None, fds),
    }
}

/// A path, or @name for the abstract namespace.
fn socket_addr(sock: &str) -> std::io::Result<net::SocketAddrUnix> {
    // The size of sun_path.
    if sock.len() >= 108 {
        return Err(Errno::NAMETOOLONG.into());
    }
    Ok(match sock.strip_prefix('@') {
        Some(name) => net::SocketAddrUnix::new_abstract_name(name.as_bytes())?,
        None => net::SocketAddrUnix::new(sock)?,
    })
}

/// Control data for one message; a message that needs more is refused.
#[repr(align(8))]
struct ControlSpace([MaybeUninit<u8>; 512]);

fn sendmsg(
    fd: &OwnedFd,
    addr: &net::SocketAddrUnix,
    data: &[u8],
    pid: Option<i32>,
    fds: &[BorrowedFd<'_>],
) -> std::io::Result<()> {
    let mut space = ControlSpace([MaybeUninit::uninit(); 512]);
    let mut control = net::SendAncillaryBuffer::new(&mut space.0);
    if let Some(p) = pid {
        let cred = net::UCred {
            pid: Pid::from_raw(p).ok_or(Errno::INVAL)?,
            uid: rustix::process::geteuid(),
            gid: rustix::process::getegid(),
        };
        if !control.push(net::SendAncillaryMessage::ScmCredentials(cred)) {
            return Err(Errno::TOOBIG.into());
        }
    }
    if !fds.is_empty() && !control.push(net::SendAncillaryMessage::ScmRights(fds)) {
        return Err(Errno::TOOBIG.into());
    }
    net::sendmsg_addr(
        fd,
        addr,
        &[IoSlice::new(data)],
        &mut control,
        net::SendFlags::NOSIGNAL,
    )?;
    Ok(())
}

/// sd_notify_barrier(): send BARRIER=1 with the write end of a pipe, and
/// wait for the manager to close it, i.e. to have read what came before.
fn barrier(sock: &str, pid: Option<i32>) -> Result<(), String> {
    let (read, write) = pipe_with(PipeFlags::CLOEXEC).map_err(|e| e.to_string())?;
    let sent = send(sock, b"BARRIER=1", pid, &[write.as_fd()]);
    drop(write);
    sent.map_err(|e| e.to_string())?;
    let mut p = [PollFd::new(&read, PollFlags::empty())];
    let timeout = Timespec {
        tv_sec: BARRIER_TIMEOUT_SECS,
        tv_nsec: 0,
    };
    match poll(&mut p, Some(&timeout)) {
        Ok(0) => Err("timed out".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options() {
        let o = parse(&args(&["--ready", "--status=up", "--pid=self", "X=1"])).unwrap();
        assert_eq!(
            o.lines,
            [
                "READY=1",
                "STATUS=up",
                "X=1",
                &format!("MAINPID={}", std::process::id())
            ]
        );
        let o = parse(&args(&["--exec", "--ready", ";", "sleep", "1"])).unwrap();
        assert_eq!(o.exec, ["sleep", "1"]);
        assert!(parse(&args(&["--exec", "--ready"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
        assert_eq!(parse(&args(&["--pid"])).unwrap().pid, Some(parent_pid()));
    }

    /// One datagram with its credentials and fds, as a manager reads it.
    fn receive(
        rx: &std::os::unix::net::UnixDatagram,
    ) -> (Vec<u8>, Option<libc::ucred>, Vec<std::fs::File>) {
        use std::os::fd::{AsRawFd, FromRawFd};
        let mut buf = vec![0u8; 4096];
        let mut control = [0u64; 512];
        unsafe {
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = std::mem::size_of_val(&control) as _;
            let n = libc::recvmsg(rx.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC);
            assert!(n >= 0, "{}", errno());
            buf.truncate(n as usize);
            let (mut cred, mut fds) = (None, Vec::new());
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_type == libc::SCM_CREDENTIALS {
                    cred = Some(std::ptr::read_unaligned(
                        libc::CMSG_DATA(c) as *const libc::ucred
                    ));
                } else if (*c).cmsg_type == libc::SCM_RIGHTS {
                    let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / 4;
                    let d = libc::CMSG_DATA(c) as *const i32;
                    for i in 0..count {
                        fds.push(std::fs::File::from_raw_fd(std::ptr::read_unaligned(
                            d.add(i),
                        )));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
            (buf, cred, fds)
        }
    }

    /// A socket at a temporary path that asks for credentials.
    fn listening(name: &str) -> (std::os::unix::net::UnixDatagram, std::path::PathBuf) {
        use std::os::fd::AsRawFd;
        let path = std::env::temp_dir().join(format!(
            "uinitconverter-notify-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let rx = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let on: libc::c_int = 1;
        let r = unsafe {
            libc::setsockopt(
                rx.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                &on as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as u32,
            )
        };
        assert_eq!(r, 0);
        (rx, path)
    }

    #[test]
    fn credentials_and_fds_go_together() {
        use std::io::{Read, Write};
        let (rx, path) = listening("creds");
        let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let me = std::process::id() as i32;
        send(
            &path.display().to_string(),
            b"FDSTORE=1",
            Some(me),
            &[b.as_fd()],
        )
        .unwrap();
        let (data, cred, mut fds) = receive(&rx);
        assert_eq!(data, b"FDSTORE=1");
        let cred = cred.unwrap();
        assert_eq!(cred.pid, me);
        assert_eq!(cred.uid, unsafe { libc::geteuid() });
        assert_eq!(cred.gid, unsafe { libc::getegid() });
        assert_eq!(fds.len(), 1);
        fds[0].write_all(b"x").unwrap();
        let mut one = [0u8; 1];
        a.read_exact(&mut one).unwrap();
        assert_eq!(&one, b"x");
        let _ = std::fs::remove_file(&path);
    }

    /// Naming another process takes privileges; without them the message
    /// still goes, with our own credentials.
    #[test]
    fn naming_another_pid_falls_back_to_our_own() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (rx, path) = listening("other");
        send(&path.display().to_string(), b"READY=1", Some(1), &[]).unwrap();
        let (data, cred, _) = receive(&rx);
        assert_eq!(data, b"READY=1");
        assert_eq!(cred.unwrap().pid, std::process::id() as i32);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn abstract_socket() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("uinitconverter-notify-test-{}", std::process::id());
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
        let rx = std::os::unix::net::UnixDatagram::bind_addr(&addr).unwrap();
        send(&format!("@{name}"), b"READY=1", None, &[]).unwrap();
        let mut buf = [0u8; 64];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
    }

    #[test]
    fn sending_fails_with_the_right_error() {
        let long = format!("/tmp/{}", "x".repeat(120));
        let e = send(&long, b"x", None, &[]).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ENAMETOOLONG));
        let e = send("/nonexistent/socket", b"x", None, &[]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }

    /// 512 bytes of control data: 124 fds fit and the next one doesn't.
    #[test]
    fn too_many_fds_are_refused() {
        let (rx, path) = listening("many");
        let f = std::fs::File::open("/dev/null").unwrap();
        let sock = path.display().to_string();
        send(&sock, b"x", None, &[f.as_fd(); 124]).unwrap();
        let (_, _, fds) = receive(&rx);
        assert_eq!(fds.len(), 124);
        let e = send(&sock, b"x", None, &[f.as_fd(); 125]).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::E2BIG));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_barrier_times_out_if_nobody_closes_the_pipe() {
        let (rx, path) = listening("barrier");
        let start = std::time::Instant::now();
        let sock = path.display().to_string();
        let t = std::thread::spawn(move || {
            let held = receive(&rx);
            std::thread::sleep(std::time::Duration::from_millis(5500));
            drop(held);
        });
        assert_eq!(barrier(&sock, None).unwrap_err(), "timed out");
        assert!(start.elapsed() >= std::time::Duration::from_millis(4900));
        t.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bad_fds_are_refused() {
        assert!(parse(&args(&["--fd=-1"])).is_err());
        assert!(parse(&args(&["--fd=999999"])).is_err());
        assert!(parse(&args(&["--fd=x"])).is_err());
        assert_eq!(parse(&args(&["--fd=0"])).unwrap().fds, [0]);
    }

    #[test]
    fn monotonic_time_goes_on() {
        let a = monotonic_usec();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = monotonic_usec();
        assert!(a > 0 && b >= a + 2000, "{a} {b}");
        let uptime = std::fs::read_to_string("/proc/uptime").unwrap();
        let up: f64 = uptime.split_whitespace().next().unwrap().parse().unwrap();
        // CLOCK_MONOTONIC leaves out suspend and /proc/uptime doesn't, so
        // this is the larger one, or the same.
        assert!((b as f64 / 1e6) <= up + 5.0, "{b} {up}");
    }

    #[test]
    fn sends_and_waits_for_the_barrier() {
        use std::os::unix::net::UnixDatagram;
        let path =
            std::env::temp_dir().join(format!("uinitconverter-notify-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let rx = UnixDatagram::bind(&path).unwrap();
        let sock = path.display().to_string();
        send(&sock, b"READY=1", None, &[]).unwrap();
        let mut buf = [0u8; 64];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        // The receiver drops the pipe end it got: the barrier returns.
        let t = std::thread::spawn(move || {
            let mut b = [0u8; 64];
            rx.recv(&mut b).unwrap();
        });
        assert!(barrier(&sock, None).is_ok());
        t.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
