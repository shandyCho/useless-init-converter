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
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
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
        (libc::SIGABRT, "ABRT"),
        (libc::SIGKILL, "KILL"),
        (libc::SIGSEGV, "SEGV"),
        (libc::SIGPIPE, "PIPE"),
        (libc::SIGTERM, "TERM"),
        (libc::SIGUSR1, "USR1"),
        (libc::SIGUSR2, "USR2"),
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
        && s
            .bytes()
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
    fn label() {
        assert_eq!(bus_label_escape("sshd.service"), "sshd_2eservice");
        assert_eq!(bus_label_escape("1foo"), "_31foo");
        assert_eq!(bus_label_unescape("sshd_2eservice"), "sshd.service");
        assert_eq!(unit_name_unescape("dev-sda1"), "dev/sda1");
        assert_eq!(unit_name_unescape("a\\x2db"), "a-b");
    }
}
