//! sdcompat-notify: a systemd-notify replacement. Sends one sd_notify
//! message to the socket in $NOTIFY_SOCKET (a path, or @name for the
//! abstract namespace). Works under any service manager that sets it.
//!
//! As systemd-notify (254 and later): the message is sent on behalf of the
//! calling process (its PID in SCM_CREDENTIALS, where the kernel allows
//! that), and then a BARRIER=1 waits until the manager has read it, unless
//! --no-block.

use std::ffi::CString;
use std::process::exit;

const USAGE: &str = "\
usage: sdcompat-notify [OPTIONS] [VAR=VALUE...]
       sdcompat-notify --exec [OPTIONS] [VAR=VALUE...] ; COMMAND [ARGS...]
       sdcompat-notify --booted

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
const BARRIER_TIMEOUT_MS: i32 = 5000;

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
        println!("sdcompat-notify {}", env!("CARGO_PKG_VERSION"));
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
                    p => p.parse().ok().filter(|&p: &i32| p > 0).ok_or_else(|| format!("bad PID {p}"))?,
                })
            }
            s if s.starts_with("--status=") => o.lines.push(format!("STATUS={}", &s[9..])),
            s if s.starts_with("--uid=") => o.uid = Some(s[6..].to_string()),
            s if s.starts_with("--fd=") => {
                let n: i32 = s[5..].parse().map_err(|_| format!("bad fd {}", &s[5..]))?;
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
    unsafe { libc::getppid() }
}

fn monotonic_usec() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1000
}

fn run(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sdcompat-notify: {e}");
            return 1;
        }
    };
    if o.booted {
        return i32::from(!std::path::Path::new("/run/systemd/system").is_dir());
    }
    let Ok(sock) = std::env::var("NOTIFY_SOCKET") else {
        eprintln!("sdcompat-notify: NOTIFY_SOCKET not set");
        return 1;
    };
    if let Some(u) = &o.uid {
        if let Err(e) = become_user(u) {
            eprintln!("sdcompat-notify: --uid={u}: {e}");
            return 1;
        }
    }
    // On behalf of the caller, unless it is PID 1 (a manager does not notify
    // itself) or another PID was named.
    let source = o.pid.unwrap_or_else(parent_pid);
    let source = (source > 1).then_some(source);
    if let Err(e) = send(&sock, o.lines.join("\n").as_bytes(), source, &o.fds) {
        eprintln!("sdcompat-notify: {e}");
        return 1;
    }
    if !o.no_block {
        if let Err(e) = barrier(&sock, source) {
            // An old manager may not answer barriers: the message went out.
            eprintln!("sdcompat-notify: barrier: {e}");
        }
    }
    if !o.exec.is_empty() {
        let c: Vec<CString> = o.exec.iter().map(|a| CString::new(a.as_str()).unwrap_or_default()).collect();
        let mut p: Vec<*const libc::c_char> = c.iter().map(|a| a.as_ptr()).collect();
        p.push(std::ptr::null());
        unsafe { libc::execvp(p[0], p.as_ptr()) };
        eprintln!("sdcompat-notify: exec {}: {}", o.exec[0], errno());
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
fn send(sock: &str, data: &[u8], pid: Option<i32>, fds: &[i32]) -> std::io::Result<()> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(errno());
    }
    let r = match pid {
        Some(p) => match sendmsg(fd, sock, data, Some(p), fds) {
            Err(e) if e.raw_os_error() == Some(libc::EPERM) => sendmsg(fd, sock, data, None, fds),
            r => r,
        },
        None => sendmsg(fd, sock, data, None, fds),
    };
    unsafe { libc::close(fd) };
    r
}

fn sendmsg(fd: i32, sock: &str, data: &[u8], pid: Option<i32>, fds: &[i32]) -> std::io::Result<()> {
    unsafe {
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as u16;
        if sock.len() >= addr.sun_path.len() {
            return Err(std::io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        }
        let abstract_ns = sock.starts_with('@');
        for (i, b) in sock.bytes().enumerate() {
            addr.sun_path[i] = if abstract_ns && i == 0 { 0 } else { b as libc::c_char };
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + sock.len() + usize::from(!abstract_ns)) as u32;
        let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut libc::c_void, iov_len: data.len() };
        // u64s: aligned for cmsghdr, as a byte array is not.
        let mut cbuf = [0u64; 64];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_name = &mut addr as *mut _ as *mut libc::c_void;
        msg.msg_namelen = len;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        let cred_space = libc::CMSG_SPACE(std::mem::size_of::<libc::ucred>() as u32) as usize;
        let fds_space = libc::CMSG_SPACE(std::mem::size_of_val(fds) as u32) as usize;
        let mut total = 0;
        if pid.is_some() {
            total += cred_space;
        }
        if !fds.is_empty() {
            total += fds_space;
        }
        if total > std::mem::size_of_val(&cbuf) {
            return Err(std::io::Error::from_raw_os_error(libc::E2BIG));
        }
        if total > 0 {
            msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = total as _;
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            if let Some(p) = pid {
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_CREDENTIALS;
                (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as u32) as _;
                let cred = libc::ucred { pid: p, uid: libc::geteuid(), gid: libc::getegid() };
                std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut libc::ucred, cred);
                c = libc::CMSG_NXTHDR(&msg, c);
            }
            if !fds.is_empty() {
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as _;
                let d = libc::CMSG_DATA(c) as *mut i32;
                for (i, f) in fds.iter().enumerate() {
                    std::ptr::write_unaligned(d.add(i), *f);
                }
            }
        }
        if libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL) < 0 {
            return Err(errno());
        }
    }
    Ok(())
}

/// sd_notify_barrier(): send BARRIER=1 with the write end of a pipe, and
/// wait for the manager to close it, i.e. to have read what came before.
fn barrier(sock: &str, pid: Option<i32>) -> Result<(), String> {
    let mut p = [0i32; 2];
    if unsafe { libc::pipe2(p.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(errno().to_string());
    }
    let r = send(sock, b"BARRIER=1", pid, &[p[1]]);
    unsafe { libc::close(p[1]) };
    let out = r.map_err(|e| e.to_string()).and_then(|()| {
        let mut pfd = libc::pollfd { fd: p[0], events: 0, revents: 0 };
        match unsafe { libc::poll(&mut pfd, 1, BARRIER_TIMEOUT_MS) } {
            n if n > 0 => Ok(()),
            0 => Err("timed out".into()),
            _ => Err(errno().to_string()),
        }
    });
    unsafe { libc::close(p[0]) };
    out
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
        assert_eq!(o.lines, ["READY=1", "STATUS=up", "X=1", &format!("MAINPID={}", std::process::id())]);
        let o = parse(&args(&["--exec", "--ready", ";", "sleep", "1"])).unwrap();
        assert_eq!(o.exec, ["sleep", "1"]);
        assert!(parse(&args(&["--exec", "--ready"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
        assert_eq!(parse(&args(&["--pid"])).unwrap().pid, Some(parent_pid()));
    }

    #[test]
    fn sends_and_waits_for_the_barrier() {
        use std::os::unix::net::UnixDatagram;
        let path = std::env::temp_dir().join(format!("sdcompat-notify-test-{}", std::process::id()));
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
