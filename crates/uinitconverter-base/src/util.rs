//! Small helpers shared by all subcommands.

use std::ffi::{CStr, CString};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn fnmatch(pattern: &str, s: &str) -> bool {
    let (Ok(p), Ok(s)) = (CString::new(pattern), CString::new(s)) else {
        return false;
    };
    unsafe { libc::fnmatch(p.as_ptr(), s.as_ptr(), 0) == 0 }
}

pub fn now_realtime_usec() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

pub fn now_monotonic_usec() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1000
}

/// Write a file atomically (tmp + rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
    }
    fs::rename(&tmp, path)
}

/// Whether another process holds a lock on `path`, as skalibs' fd_islocked()
/// asks (s6-rc's change lock, s6-supervise's supervise/lock). skalibs locks
/// with fcntl(); F_GETLK only looks.
pub fn lock_held(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = fs::File::open(path) else {
        return false;
    };
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_RDLCK as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETLK, &mut fl) };
    r == 0 && fl.l_type != libc::F_UNLCK as libc::c_short
}

/// A service's state as s6-supervise records it in `DIR/supervise/status`
/// (s6's s6_svstatus_t: two TAI64N stamps, the pid as 8 bytes big-endian
/// at 24, the wait status at 32, flags at 34: 2 finishing, 4 want up,
/// 8 ready), read as s6-svstat reads it, without starting a process (a
/// shutdown started ~100 s6-svstat a second, work_history part 186).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvStatus {
    /// s6-svstat's `up`: a pid, not finishing.
    pub up: bool,
    pub wanted_up: bool,
    /// s6-svstat's `ready`: a pid and the ready flag.
    pub ready: bool,
    pub pid: u32,
    /// Down with its finish script done: not up and the ready flag set
    /// again, what `s6-svc -wD` (s6_svlisten_loop) waits for.
    pub down_done: bool,
    /// The first TAI64N (bytes 0-12): when the state last changed. Only
    /// compared with an earlier read, to tell that s6-supervise acted.
    pub stamp: [u8; 12],
}

pub const S6_SVSTATUS_SIZE: usize = 35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvRead {
    /// No s6-supervise holds DIR/supervise/lock (s6-svstat fails then).
    NotSupervised,
    Status(SvStatus),
    /// A status file of another size (another s6 version): ask s6-svstat.
    Unknown,
}

pub fn svstatus(dir: &Path) -> SvRead {
    if !lock_held(&dir.join("supervise/lock")) {
        return SvRead::NotSupervised;
    }
    match fs::read(dir.join("supervise/status")) {
        Ok(b) if b.len() == S6_SVSTATUS_SIZE => SvRead::Status(svstatus_unpack(&b)),
        _ => SvRead::Unknown,
    }
}

pub fn svstatus_unpack(b: &[u8]) -> SvStatus {
    let pid = u64::from_be_bytes(b[24..32].try_into().unwrap());
    let flags = b[34];
    let pid = u32::try_from(pid).unwrap_or(0);
    let up = pid != 0 && flags & 2 == 0;
    SvStatus {
        up,
        wanted_up: flags & 4 != 0,
        ready: pid != 0 && flags & 8 != 0,
        pid,
        down_done: !up && flags & 8 != 0,
        stamp: b[0..12].try_into().unwrap(),
    }
}

/// What writing to an s6 control FIFO (`supervise/control`,
/// `.s6-svscan/control`) did, as s6's s6_svc_write reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtlWrite {
    Written,
    /// No one reads it: its s6-supervise or s6-svscan isn't running.
    NoReader,
    /// No FIFO there.
    Missing,
    Failed,
}

/// Write `cmd` to an s6 control FIFO, as s6-svc and s6-svscanctl do
/// (open without blocking, then a blocking write), without starting them.
pub fn s6_control(fifo: &Path, cmd: &[u8]) -> CtlWrite {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let f = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(fifo);
    let mut f = match f {
        Ok(f) => f,
        Err(e) => {
            return match e.raw_os_error() {
                Some(libc::ENXIO) => CtlWrite::NoReader,
                Some(libc::ENOENT | libc::ENOTDIR | libc::EISDIR) => CtlWrite::Missing,
                _ => CtlWrite::Failed,
            }
        }
    };
    use std::os::fd::AsRawFd;
    let fd = f.as_raw_fd();
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl < 0 || libc::fcntl(fd, libc::F_SETFL, fl & !libc::O_NONBLOCK) < 0 {
            return CtlWrite::Failed;
        }
    }
    match f.write_all(cmd) {
        Ok(()) => CtlWrite::Written,
        Err(_) => CtlWrite::Failed,
    }
}

pub fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub struct UserInfo {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

pub fn lookup_user(name: &str) -> Option<UserInfo> {
    if let Ok(uid) = name.parse::<u32>() {
        return lookup_uid(uid);
    }
    let c = CString::new(name).ok()?;
    unsafe { passwd_to_info(libc::getpwnam(c.as_ptr())) }
}

pub fn lookup_uid(uid: u32) -> Option<UserInfo> {
    unsafe { passwd_to_info(libc::getpwuid(uid)) }
}

unsafe fn passwd_to_info(pw: *mut libc::passwd) -> Option<UserInfo> {
    if pw.is_null() {
        return None;
    }
    let pw = &*pw;
    let s = |p: *const libc::c_char| {
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    Some(UserInfo {
        name: s(pw.pw_name),
        uid: pw.pw_uid,
        gid: pw.pw_gid,
        home: s(pw.pw_dir),
        shell: s(pw.pw_shell),
    })
}

pub fn group_name(gid: u32) -> Option<String> {
    unsafe {
        let gr = libc::getgrgid(gid);
        (!gr.is_null() && !(*gr).gr_name.is_null())
            .then(|| CStr::from_ptr((*gr).gr_name).to_string_lossy().into_owned())
    }
}

pub fn lookup_group(name: &str) -> Option<u32> {
    if let Ok(gid) = name.parse::<u32>() {
        return Some(gid);
    }
    let c = CString::new(name).ok()?;
    unsafe {
        let gr = libc::getgrnam(c.as_ptr());
        (!gr.is_null()).then(|| (*gr).gr_gid)
    }
}

pub fn hostname() -> String {
    read_trimmed(Path::new("/proc/sys/kernel/hostname")).unwrap_or_default()
}

pub fn machine_id() -> String {
    read_trimmed(Path::new("/etc/machine-id")).unwrap_or_default()
}

pub fn boot_id() -> String {
    read_trimmed(Path::new("/proc/sys/kernel/random/boot_id"))
        .unwrap_or_default()
        .replace('-', "")
}

/// systemd's bus_label_escape(): object path element for a unit name.
pub fn bus_label_escape(s: &str) -> String {
    if s.is_empty() {
        return "_".into();
    }
    let mut out = String::new();
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()) {
            out.push(b as char);
        } else {
            out.push_str(&format!("_{b:02x}"));
        }
    }
    out
}

/// `s` as one path component: unchanged if it is one, else bus-label
/// escaped (letters, digits and `_` only), so it can never be "..", an
/// absolute path or a subdirectory. Unit ids name directories (state,
/// cgroups); an invalid name from the bus must not leave them.
pub fn path_component(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_empty() || s == "." || s == ".." || s.contains('/') || s.contains('\0') {
        bus_label_escape(s).into()
    } else {
        s.into()
    }
}

pub fn bus_label_unescape(s: &str) -> String {
    if s == "_" {
        return String::new();
    }
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// systemd's unit_name_path_escape() (systemd-escape --path): "/" and
/// surrounding slashes dropped, "/" as "-", anything but [A-Za-z0-9:_.]
/// (and a leading ".") as \xNN.
pub fn unit_name_path_escape(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.is_empty() {
        return "-".into();
    }
    let mut out = String::new();
    for (i, b) in parts.join("/").bytes().enumerate() {
        match b {
            b'/' => out.push('-'),
            b'.' if i == 0 => out.push_str("\\x2e"),
            b if b.is_ascii_alphanumeric() || b":_.".contains(&b) => out.push(b as char),
            b => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

/// Unescape a unit name instance (systemd-escape --unescape).
pub fn unit_name_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'-' {
            out.push(b'/');
        } else if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
            out.push(b[i]);
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn signal_number(name: &str) -> Option<i32> {
    if let Ok(n) = name.parse::<i32>() {
        return Some(n);
    }
    let n = name.trim_start_matches("SIG");
    Some(match n {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "ILL" => libc::SIGILL,
        "TRAP" => libc::SIGTRAP,
        "ABRT" => libc::SIGABRT,
        "BUS" => libc::SIGBUS,
        "FPE" => libc::SIGFPE,
        "KILL" => libc::SIGKILL,
        "USR1" => libc::SIGUSR1,
        "SEGV" => libc::SIGSEGV,
        "USR2" => libc::SIGUSR2,
        "PIPE" => libc::SIGPIPE,
        "ALRM" => libc::SIGALRM,
        "TERM" => libc::SIGTERM,
        "CHLD" => libc::SIGCHLD,
        "CONT" => libc::SIGCONT,
        "STOP" => libc::SIGSTOP,
        "TSTP" => libc::SIGTSTP,
        "WINCH" => libc::SIGWINCH,
        "PWR" => libc::SIGPWR,
        _ => {
            if let Some(off) = n.strip_prefix("RTMIN+") {
                return off.parse::<i32>().ok().map(|o| libc::SIGRTMIN() + o);
            }
            return None;
        }
    })
}

pub fn signal_name(sig: i32) -> String {
    let names = [
        (libc::SIGHUP, "HUP"),
        (libc::SIGINT, "INT"),
        (libc::SIGQUIT, "QUIT"),
        (libc::SIGILL, "ILL"),
        (libc::SIGTRAP, "TRAP"),
        (libc::SIGABRT, "ABRT"),
        (libc::SIGBUS, "BUS"),
        (libc::SIGFPE, "FPE"),
        (libc::SIGKILL, "KILL"),
        (libc::SIGUSR1, "USR1"),
        (libc::SIGSEGV, "SEGV"),
        (libc::SIGUSR2, "USR2"),
        (libc::SIGPIPE, "PIPE"),
        (libc::SIGALRM, "ALRM"),
        (libc::SIGTERM, "TERM"),
        (libc::SIGCHLD, "CHLD"),
        (libc::SIGCONT, "CONT"),
        (libc::SIGSTOP, "STOP"),
        (libc::SIGTSTP, "TSTP"),
        (libc::SIGTTIN, "TTIN"),
        (libc::SIGTTOU, "TTOU"),
        (libc::SIGURG, "URG"),
        (libc::SIGXCPU, "XCPU"),
        (libc::SIGXFSZ, "XFSZ"),
        (libc::SIGVTALRM, "VTALRM"),
        (libc::SIGPROF, "PROF"),
        (libc::SIGWINCH, "WINCH"),
        (libc::SIGIO, "IO"),
        (libc::SIGPWR, "PWR"),
        (libc::SIGSYS, "SYS"),
    ];
    names
        .iter()
        .find(|(n, _)| *n == sig)
        .map(|(_, s)| s.to_string())
        .unwrap_or_else(|| sig.to_string())
}

/// Shell-quote a word for generated scripts.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Return the PIDs listed in a cgroup (recursively).
pub fn cgroup_pids(cgroup_dir: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    if let Ok(s) = fs::read_to_string(cgroup_dir.join("cgroup.procs")) {
        out.extend(s.lines().filter_map(|l| l.trim().parse::<u32>().ok()));
    }
    if let Ok(rd) = fs::read_dir(cgroup_dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.extend(cgroup_pids(&e.path()));
            }
        }
    }
    out
}

/// Direct children of a process (/proc/PID/task/*/children).
pub fn proc_children(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(format!("/proc/{pid}/task")) {
        for t in rd.flatten() {
            if let Ok(s) = fs::read_to_string(t.path().join("children")) {
                out.extend(s.split_whitespace().filter_map(|p| p.parse::<u32>().ok()));
            }
        }
    }
    out
}

/// Every descendant of a process, children first (not the process itself).
pub fn proc_descendants(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut todo = proc_children(pid);
    while let Some(p) = todo.pop() {
        if !out.contains(&p) {
            out.push(p);
            todo.extend(proc_children(p));
        }
    }
    out
}

pub fn proc_cmdline(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// The unified cgroup path (relative, starting with '/') of a process.
pub fn proc_cgroup(pid: u32) -> Option<String> {
    let s = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_escape() {
        assert_eq!(
            unit_name_path_escape("/sys/devices/pci0000:00/a-b c"),
            "sys-devices-pci0000:00-a\\x2db\\x20c"
        );
        assert_eq!(unit_name_path_escape("/"), "-");
        assert_eq!(unit_name_path_escape("/.hidden//x/"), "\\x2ehidden-x");
        assert_eq!(
            unit_name_unescape(&unit_name_path_escape("/dev/sda1")),
            "dev/sda1"
        );
    }

    #[test]
    fn label() {
        assert_eq!(bus_label_escape("sshd.service"), "sshd_2eservice");
        assert_eq!(bus_label_escape("1foo"), "_31foo");
        assert_eq!(bus_label_unescape("sshd_2eservice"), "sshd.service");
        assert_eq!(unit_name_unescape("dev-sda1"), "dev/sda1");
        assert_eq!(unit_name_unescape("a\\x2db"), "a-b");
    }

    #[test]
    fn one_component() {
        assert_eq!(path_component("user@0.service"), "user@0.service");
        assert_eq!(path_component("-.mount"), "-.mount");
        assert_eq!(path_component("...service"), "...service");
        for bad in ["../../etc/x.service", "/etc", "a/b.service", "..", ".", ""] {
            let c = path_component(bad);
            assert!(
                !c.contains('/') && c != ".." && c != "." && !c.is_empty(),
                "{bad} -> {c}"
            );
        }
        let st = crate::state::for_unit(Path::new("/run/x"), "../../../etc/y.service");
        assert_eq!(st.dir.parent(), Some(Path::new("/run/x/units")));
    }
}

/// One line to syslog (journald's /dev/log), as root only: the sandbox runs
/// unprivileged and would otherwise write into the host's journal. glibc
/// drops the message when /dev/log is not there (before journald is up).
pub fn syslog_line(prio: libc::c_int, line: &str) {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let Ok(c) = CString::new(line) else { return };
    unsafe { libc::syslog(libc::LOG_DAEMON | prio, c"%s".as_ptr(), c.as_ptr()) };
}

/// Copy everything written to stderr, ours and our children's, to syslog
/// too. stderr is s6's catch-all log, which lives in /run and is lost at
/// power-off, the very case where the messages are needed. Root only.
pub fn tee_stderr_to_syslog(ident: &'static CStr) {
    use std::io::BufRead;
    use std::os::fd::FromRawFd;
    let (r, orig) = unsafe {
        if libc::geteuid() != 0 {
            return;
        }
        let mut fds = [0i32; 2];
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) < 0 {
            return;
        }
        let orig = libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3);
        if orig < 0 || libc::dup2(fds[1], 2) < 0 {
            libc::close(fds[0]);
            libc::close(fds[1]);
            if orig >= 0 {
                libc::close(orig);
            }
            return;
        }
        libc::close(fds[1]);
        libc::openlog(ident.as_ptr(), libc::LOG_PID, libc::LOG_DAEMON);
        (fs::File::from_raw_fd(fds[0]), fs::File::from_raw_fd(orig))
    };
    std::thread::spawn(move || {
        let mut orig = orig;
        let mut r = io::BufReader::new(r);
        let mut buf = Vec::new();
        // Bytes, not lines(): a line that is not UTF-8 (a child's output)
        // must not end this thread, or fd 2 becomes a pipe nobody reads:
        // every eprintln! then panics and every child dies of SIGPIPE.
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            let _ = orig.write_all(&buf);
            let _ = orig.write_all(b"\n");
            let line = String::from_utf8_lossy(&buf).replace('\0', "");
            let Ok(c) = CString::new(line) else { continue };
            unsafe { libc::syslog(libc::LOG_NOTICE, c"%s".as_ptr(), c.as_ptr()) };
        }
    });
}

/// sd_notify(3): send `msg` to $NOTIFY_SOCKET (a path, or @ for the
/// abstract namespace). Nothing when it is unset; errors are ignored.
pub fn sd_notify(msg: &str) {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let path = path.to_string_lossy().into_owned();
    let addr = match path.strip_prefix('@') {
        Some(name) => SocketAddr::from_abstract_name(name.as_bytes()),
        None => SocketAddr::from_pathname(&path),
    };
    let (Ok(addr), Ok(sock)) = (addr, UnixDatagram::unbound()) else {
        return;
    };
    // Logged: a lost READY=1 leaves the unit "activating" until its start
    // timeout, with nothing else to show why.
    if let Err(e) = sock.send_to_addr(msg.as_bytes(), &addr) {
        let first = msg.lines().next().unwrap_or_default();
        eprintln!("sd_notify {first} to {path}: {e}");
    }
}

#[cfg(test)]
mod svstatus_tests {
    use super::*;

    fn pack(pid: u64, flags: u8) -> Vec<u8> {
        let mut b = vec![0u8; S6_SVSTATUS_SIZE];
        b[24..32].copy_from_slice(&pid.to_be_bytes());
        b[34] = flags;
        b
    }

    #[test]
    fn svstatus_unpack_matches_s6_svstat() {
        // up, want up, ready
        assert_eq!(
            svstatus_unpack(&pack(1234, 4 | 8)),
            SvStatus {
                up: true,
                wanted_up: true,
                ready: true,
                pid: 1234,
                down_done: false,
                stamp: [0; 12],
            }
        );
        // The stamp is the first 12 bytes, as s6 writes it.
        let mut b = pack(0, 0);
        b[..12].copy_from_slice(&[0x40, 0, 0, 0, 0x6a, 0x1b, 0x2c, 0x3d, 0, 1, 2, 3]);
        assert_eq!(
            svstatus_unpack(&b).stamp,
            [0x40, 0, 0, 0, 0x6a, 0x1b, 0x2c, 0x3d, 0, 1, 2, 3]
        );
        // finishing: not up (s6-svstat's isup), still a pid, not done
        let s = svstatus_unpack(&pack(99, 2));
        assert!(!s.up && !s.wanted_up && !s.ready && !s.down_done);
        // finish done: down, ready flag set again
        assert!(svstatus_unpack(&pack(0, 8)).down_done);
        // down, want up (a restart pending)
        let s = svstatus_unpack(&pack(0, 4 | 8));
        assert!(!s.up && s.wanted_up && !s.ready && s.pid == 0);
    }

    #[test]
    fn s6_control_reports_as_s6_svc_write() {
        let d = std::env::temp_dir().join(format!("s6ctl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let fifo = d.join("control");
        assert_eq!(s6_control(&fifo, b"d"), CtlWrite::Missing);
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(s6_control(&fifo, b"d"), CtlWrite::NoReader);
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let mut r = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        assert_eq!(s6_control(&fifo, b"an"), CtlWrite::Written);
        let mut got = String::new();
        r.read_to_string(&mut got).unwrap();
        assert_eq!(got, "an");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn svstatus_without_supervisor_or_with_another_format() {
        let d = std::env::temp_dir().join(format!("svstatus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("supervise")).unwrap();
        fs::write(d.join("supervise/status"), pack(1, 4)).unwrap();
        // No one holds the lock.
        assert_eq!(svstatus(&d), SvRead::NotSupervised);
        // Hold it as s6-supervise does (a write lock on supervise/lock).
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(d.join("supervise/lock"))
            .unwrap();
        use std::os::fd::AsRawFd;
        let mut fl: libc::flock = unsafe { std::mem::zeroed() };
        fl.l_type = libc::F_WRLCK as libc::c_short;
        fl.l_whence = libc::SEEK_SET as libc::c_short;
        assert_eq!(unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLK, &fl) }, 0);
        // The lock must be held by another process for F_GETLK to see it.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let r = match svstatus(&d) {
                SvRead::Status(s) if s.up && s.wanted_up && s.pid == 1 => {
                    fs::write(d.join("supervise/status"), [0u8; 20]).unwrap();
                    if svstatus(&d) == SvRead::Unknown {
                        0
                    } else {
                        2
                    }
                }
                _ => 1,
            };
            unsafe { libc::_exit(r) };
        }
        let mut st = 0;
        unsafe { libc::waitpid(pid, &mut st, 0) };
        assert_eq!(libc::WEXITSTATUS(st), 0);
        let _ = fs::remove_dir_all(&d);
    }
}
