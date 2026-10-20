//! SystemCallFilter=, SystemCallErrorNumber= and SystemCallArchitectures=.
//! The filter is built like systemd builds it, with systemd's own syscall
//! groups (syscall-groups.txt) and libseccomp, loaded at run time so that
//! sdcompat has no build or install dependency on it.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

use crate::loader::Unit;

const GROUPS: &str = include_str!("syscall-groups.txt");

/// What a matching (or, for an allow list, non-matching) syscall gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Kill,
    Errno(i32),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Filter {
    /// true: only the listed syscalls are allowed; false: they are denied.
    pub allow_list: bool,
    /// Syscall name -> per-entry action (deny lists only; `name:EPERM`).
    pub syscalls: BTreeMap<String, Option<Action>>,
    /// SystemCallErrorNumber= (default: kill the process).
    pub action: Action,
    /// SystemCallArchitectures=, as libseccomp arch names; empty = all.
    pub archs: Vec<String>,
}

fn groups() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut cur: Option<&str> = None;
    for line in GROUPS.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            cur = Some(t);
            map.entry(t).or_default();
        } else if let Some(g) = cur {
            map.entry(g).or_default().push(t);
        }
    }
    map
}

/// Expand a name or @group into syscall names (groups nest).
fn expand(name: &str, groups: &BTreeMap<&str, Vec<&str>>, out: &mut Vec<String>, depth: u32) -> Result<(), String> {
    if !name.starts_with('@') {
        out.push(name.to_string());
        return Ok(());
    }
    let members = groups.get(name).ok_or_else(|| format!("unknown system call group {name}"))?;
    if depth > 8 {
        return Err(format!("system call group {name} nests too deeply"));
    }
    for m in members {
        expand(m, groups, out, depth + 1)?;
    }
    Ok(())
}

fn errno_by_name(name: &str) -> Option<i32> {
    extern "C" {
        fn strerrorname_np(errnum: c_int) -> *const c_char;
    }
    (1..4096).find(|&e| {
        let p = unsafe { strerrorname_np(e) };
        !p.is_null() && unsafe { CStr::from_ptr(p) }.to_bytes() == name.as_bytes()
    })
}

/// SystemCallErrorNumber= and the `:ERRNO` suffix: an errno name, a number or "kill".
pub fn parse_action(v: &str) -> Result<Action, String> {
    if v == "kill" {
        return Ok(Action::Kill);
    }
    let n = match v.parse::<i32>() {
        Ok(n) => n,
        Err(_) => errno_by_name(v).ok_or_else(|| format!("unknown errno {v}"))?,
    };
    if !(1..4096).contains(&n) {
        return Err(format!("errno {v} out of range"));
    }
    Ok(Action::Errno(n))
}

/// systemd architecture names (SystemCallArchitectures=) to libseccomp's.
fn seccomp_arch(name: &str) -> String {
    let native = crate::loader::systemd_arch();
    let n = if name == "native" { native } else { name };
    match n {
        "x86-64" => "x86_64",
        "arm64" => "aarch64",
        "mips64-le" => "mipsel64",
        "mips-le" => "mipsel",
        "ppc64-le" => "ppc64le",
        other => other,
    }
    .to_string()
}

impl Filter {
    /// Merge SystemCallFilter= lines the way systemd does: the first line
    /// decides between allow list (which then includes @default) and deny
    /// list (`~`); later lines of the other kind remove entries.
    pub fn parse(lines: &[String], errno: Option<&str>, archs: &[String]) -> Result<Option<Filter>, String> {
        let groups = groups();
        let action = match errno {
            Some(v) if !v.is_empty() => parse_action(v)?,
            _ => Action::Kill,
        };
        let mut filter: Option<Filter> = None;
        for line in lines {
            let (invert, rest) = match line.strip_prefix('~') {
                Some(r) => (true, r),
                None => (false, line.as_str()),
            };
            let first = filter.is_none();
            let f = filter.get_or_insert_with(|| Filter {
                allow_list: !invert,
                syscalls: BTreeMap::new(),
                action,
                archs: Vec::new(),
            });
            if first && f.allow_list {
                let mut names = Vec::new();
                expand("@default", &groups, &mut names, 0)?;
                for n in names {
                    f.syscalls.insert(n, None);
                }
            }
            let add = invert != f.allow_list;
            for word in rest.split_whitespace() {
                let (name, entry_action) = match word.split_once(':') {
                    Some((n, e)) => (n, Some(parse_action(e)?)),
                    None => (word, None),
                };
                let mut names = Vec::new();
                expand(name, &groups, &mut names, 0)?;
                for n in names {
                    if add {
                        f.syscalls.insert(n, if f.allow_list { None } else { entry_action });
                    } else {
                        f.syscalls.remove(&n);
                    }
                }
            }
        }
        let archs: Vec<String> = archs.iter().map(|a| seccomp_arch(a)).collect();
        if filter.is_none() && !archs.is_empty() {
            // Only an architecture restriction: an empty deny list.
            filter = Some(Filter { allow_list: false, syscalls: BTreeMap::new(), action, archs: Vec::new() });
        }
        if let Some(f) = filter.as_mut() {
            f.archs = archs;
        }
        Ok(filter)
    }

    pub fn from_unit(unit: &Unit) -> Result<Option<Filter>, String> {
        let lines = unit.get_all("Service", "SystemCallFilter");
        let archs = unit.get_words("Service", "SystemCallArchitectures");
        Filter::parse(&lines, unit.get("Service", "SystemCallErrorNumber").as_deref(), &archs)
    }

    /// Load the filter into this process (right before execve).
    pub fn apply(&self) -> Result<(), String> {
        // Like systemd without seccomp support: run the service unfiltered.
        let lib = match Lib::open() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("sdcompat: {e}; SystemCallFilter= skipped");
                return Ok(());
            }
        };
        let act = |a: Action| match a {
            Action::Kill => SCMP_ACT_KILL_PROCESS,
            Action::Errno(e) => SCMP_ACT_ERRNO | (e as u32 & 0xffff),
        };
        let default = if self.allow_list { act(self.action) } else { SCMP_ACT_ALLOW };
        let ctx = unsafe { (lib.init)(default) };
        if ctx.is_null() {
            return Err("seccomp_init failed".into());
        }
        let r = self.fill(&lib, ctx, act);
        unsafe { (lib.release)(ctx) };
        r
    }

    fn fill(&self, lib: &Lib, ctx: *mut c_void, act: impl Fn(Action) -> u32) -> Result<(), String> {
        unsafe {
            // Without CAP_SYS_ADMIN the kernel requires no_new_privs; systemd
            // likewise implies NoNewPrivileges= then.
            (lib.attr_set)(ctx, SCMP_FLTATR_CTL_NNP, u32::from(!have_sys_admin()));
            (lib.attr_set)(ctx, SCMP_FLTATR_ACT_BADARCH, SCMP_ACT_KILL_PROCESS);
            let native = (lib.arch_native)();
            let wanted: Vec<u32> = if self.archs.is_empty() {
                // Filter the secondary ABIs too, as systemd does.
                let extra: &[&str] = match std::env::consts::ARCH {
                    "x86_64" => &["x86", "x32"],
                    "aarch64" => &["arm"],
                    _ => &[],
                };
                extra.iter().map(|a| lib.arch(a)).filter(|&t| t != 0).collect()
            } else {
                self.archs.iter().map(|a| lib.arch(a)).filter(|&t| t != 0).collect()
            };
            for &t in &wanted {
                if t != native {
                    (lib.arch_add)(ctx, t);
                }
            }
            if !self.archs.is_empty() && !wanted.contains(&native) {
                (lib.arch_remove)(ctx, native);
            }
            for (name, entry) in &self.syscalls {
                let c = CString::new(name.as_str()).unwrap_or_default();
                let nr = (lib.resolve_name)(c.as_ptr());
                if nr == NR_SCMP_ERROR {
                    continue; // unknown to this libseccomp, as systemd does
                }
                let a = if self.allow_list { SCMP_ACT_ALLOW } else { act(entry.unwrap_or(self.action)) };
                (lib.rule_add_array)(ctx, a, nr, 0, std::ptr::null());
            }
            let r = (lib.load)(ctx);
            if r < 0 {
                return Err(format!("seccomp_load: {}", std::io::Error::from_raw_os_error(-r)));
            }
        }
        Ok(())
    }
}

fn have_sys_admin() -> bool {
    const CAP_SYS_ADMIN: u32 = 21;
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("CapEff:"))
                .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
        })
        .map(|caps| caps & (1 << CAP_SYS_ADMIN) != 0)
        .unwrap_or(false)
}

const SCMP_ACT_KILL_PROCESS: u32 = 0x8000_0000;
const SCMP_ACT_ERRNO: u32 = 0x0005_0000;
const SCMP_ACT_ALLOW: u32 = 0x7fff_0000;
const SCMP_FLTATR_ACT_BADARCH: c_int = 2;
const SCMP_FLTATR_CTL_NNP: c_int = 3;
const NR_SCMP_ERROR: c_int = -1;

/// The few libseccomp functions we need, from libseccomp.so.2.
struct Lib {
    init: unsafe extern "C" fn(u32) -> *mut c_void,
    release: unsafe extern "C" fn(*mut c_void),
    attr_set: unsafe extern "C" fn(*mut c_void, c_int, u32) -> c_int,
    arch_native: unsafe extern "C" fn() -> u32,
    arch_resolve_name: unsafe extern "C" fn(*const c_char) -> u32,
    arch_add: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    arch_remove: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
    resolve_name: unsafe extern "C" fn(*const c_char) -> c_int,
    rule_add_array: unsafe extern "C" fn(*mut c_void, u32, c_int, c_uint, *const c_void) -> c_int,
    load: unsafe extern "C" fn(*mut c_void) -> c_int,
}

impl Lib {
    fn open() -> Result<Lib, String> {
        let h = unsafe { libc::dlopen(c"libseccomp.so.2".as_ptr(), libc::RTLD_NOW) };
        if h.is_null() {
            return Err("SystemCallFilter= needs libseccomp.so.2, which could not be loaded".into());
        }
        let sym = |name: &CStr| -> Result<*mut c_void, String> {
            let p = unsafe { libc::dlsym(h, name.as_ptr()) };
            if p.is_null() {
                Err(format!("libseccomp: missing {}", name.to_string_lossy()))
            } else {
                Ok(p)
            }
        };
        // SAFETY: the signatures match libseccomp's <seccomp.h>.
        unsafe {
            Ok(Lib {
                init: std::mem::transmute(sym(c"seccomp_init")?),
                release: std::mem::transmute(sym(c"seccomp_release")?),
                attr_set: std::mem::transmute(sym(c"seccomp_attr_set")?),
                arch_native: std::mem::transmute(sym(c"seccomp_arch_native")?),
                arch_resolve_name: std::mem::transmute(sym(c"seccomp_arch_resolve_name")?),
                arch_add: std::mem::transmute(sym(c"seccomp_arch_add")?),
                arch_remove: std::mem::transmute(sym(c"seccomp_arch_remove")?),
                resolve_name: std::mem::transmute(sym(c"seccomp_syscall_resolve_name")?),
                rule_add_array: std::mem::transmute(sym(c"seccomp_rule_add_array")?),
                load: std::mem::transmute(sym(c"seccomp_load")?),
            })
        }
    }

    fn arch(&self, name: &str) -> u32 {
        let c = CString::new(name).unwrap_or_default();
        unsafe { (self.arch_resolve_name)(c.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allow_list_includes_default() {
        let f = Filter::parse(&lines(&["@basic-io ioctl"]), Some("EPERM"), &[]).unwrap().unwrap();
        assert!(f.allow_list);
        assert_eq!(f.action, Action::Errno(libc::EPERM));
        for s in ["read", "write", "ioctl", "execve", "exit_group"] {
            assert!(f.syscalls.contains_key(s), "{s}");
        }
        assert!(!f.syscalls.contains_key("mlock"));
    }

    #[test]
    fn fwupd_filter_blocks_mlock() {
        let l = lines(&["@basic-io @file-system @io-event @ipc @network-io @process @sync @signal @timer @chown ioctl uname fadvise64 sysinfo madvise mremap splice vmsplice copy_file_range"]);
        let f = Filter::parse(&l, Some("EPERM"), &lines(&["native"])).unwrap().unwrap();
        assert!(!f.syscalls.contains_key("mlock"));
        assert!(f.syscalls.contains_key("openat") && f.syscalls.contains_key("clone"));
        assert_eq!(f.archs, vec![seccomp_arch("native")]);
        if cfg!(target_arch = "x86_64") {
            assert_eq!(f.archs, vec!["x86_64".to_string()]);
        }
    }

    #[test]
    fn deny_list_and_removal() {
        let f = Filter::parse(&lines(&["~@clock kill:EACCES", "clock_settime"]), None, &[]).unwrap().unwrap();
        assert!(!f.allow_list);
        assert_eq!(f.action, Action::Kill);
        assert_eq!(f.syscalls.get("kill"), Some(&Some(Action::Errno(libc::EACCES))));
        assert!(f.syscalls.contains_key("settimeofday"));
        assert!(!f.syscalls.contains_key("clock_settime"));
    }

    #[test]
    fn allow_list_removal_and_reset() {
        let f = Filter::parse(&lines(&["@system-service", "~@resources"]), None, &[]).unwrap().unwrap();
        assert!(!f.syscalls.contains_key("setpriority"));
        assert!(f.syscalls.contains_key("read"));
        // get_all() already applies "SystemCallFilter=" resets; nothing left means no filter.
        assert_eq!(Filter::parse(&[], Some("EPERM"), &[]).unwrap(), None);
    }

    #[test]
    fn errors() {
        assert!(Filter::parse(&lines(&["@nope"]), None, &[]).is_err());
        assert!(parse_action("ENOTANERRNO").is_err());
        assert_eq!(parse_action("13").unwrap(), Action::Errno(13));
        assert_eq!(parse_action("kill").unwrap(), Action::Kill);
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    /// Load a deny list in a child and check that the syscall gets EPERM.
    #[test]
    fn deny_list_takes_effect() {
        if unsafe { libc::dlopen(c"libseccomp.so.2".as_ptr(), libc::RTLD_NOW) }.is_null() {
            eprintln!("libseccomp.so.2 not available, skipping");
            return;
        }
        let f = Filter::parse(&["~getppid".to_string()], Some("EPERM"), &[]).unwrap().unwrap();
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let code = match f.apply() {
                Err(_) => 2,
                Ok(()) => {
                    let r = unsafe { libc::syscall(libc::SYS_getppid) };
                    let e = std::io::Error::last_os_error().raw_os_error();
                    if r == -1 && e == Some(libc::EPERM) { 0 } else { 1 }
                }
            };
            unsafe { libc::_exit(code) };
        }
        let mut st = 0;
        unsafe { libc::waitpid(pid, &mut st, 0) };
        assert!(libc::WIFEXITED(st), "child died: {st}");
        assert_eq!(libc::WEXITSTATUS(st), 0, "1 = getppid not blocked, 2 = apply failed");
    }
}
