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

// ---- other seccomp-based settings ----
//
// RestrictAddressFamilies=, RestrictNamespaces=, MemoryDenyWriteExecute=,
// LockPersonality=, RestrictRealtime=, RestrictSUIDSGID= and the syscall
// parts of ProtectClock=, ProtectKernelModules=, ProtectKernelLogs=,
// ProtectHostname= and PrivateDevices=. Built like systemd's seccomp-util.c:
// one allow-by-default filter per setting and architecture, with the same
// rules and errnos.

/// libseccomp's struct scmp_arg_cmp.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgCmp {
    arg: c_uint,
    op: c_int,
    a: u64,
    b: u64,
}

const SCMP_CMP_NE: c_int = 1;
const SCMP_CMP_GT: c_int = 6;
const SCMP_CMP_EQ: c_int = 4;
const SCMP_CMP_MASKED_EQ: c_int = 7;

fn arg_eq(arg: c_uint, v: u64) -> ArgCmp {
    ArgCmp { arg, op: SCMP_CMP_EQ, a: v, b: 0 }
}
fn arg_ne(arg: c_uint, v: u64) -> ArgCmp {
    ArgCmp { arg, op: SCMP_CMP_NE, a: v, b: 0 }
}
fn arg_gt(arg: c_uint, v: u64) -> ArgCmp {
    ArgCmp { arg, op: SCMP_CMP_GT, a: v, b: 0 }
}
/// (arg & bits) == bits.
fn arg_has(arg: c_uint, bits: u64) -> ArgCmp {
    ArgCmp { arg, op: SCMP_CMP_MASKED_EQ, a: bits, b: bits }
}

/// `syscall` fails with `errno` when all `args` comparisons match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub syscall: String,
    pub errno: i32,
    pub args: Vec<ArgCmp>,
}

fn rule(syscall: &str, errno: i32, args: Vec<ArgCmp>) -> Rule {
    Rule { syscall: syscall.to_string(), errno, args }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Restriction {
    /// The setting, for error messages.
    pub what: &'static str,
    pub rules: Vec<Rule>,
    /// Native architecture only. systemd skips the ABIs whose socket() is
    /// multiplexed through socketcall() (i386) for RestrictAddressFamilies=.
    pub native_only: bool,
}

const ADDRESS_FAMILIES: &[(&str, u64)] = &[
    ("AF_UNIX", 1), ("AF_LOCAL", 1), ("AF_INET", 2), ("AF_AX25", 3), ("AF_IPX", 4),
    ("AF_APPLETALK", 5), ("AF_NETROM", 6), ("AF_BRIDGE", 7), ("AF_ATMPVC", 8), ("AF_X25", 9),
    ("AF_INET6", 10), ("AF_ROSE", 11), ("AF_DECnet", 12), ("AF_NETBEUI", 13), ("AF_SECURITY", 14),
    ("AF_KEY", 15), ("AF_NETLINK", 16), ("AF_ROUTE", 16), ("AF_PACKET", 17), ("AF_ASH", 18),
    ("AF_ECONET", 19), ("AF_ATMSVC", 20), ("AF_RDS", 21), ("AF_SNA", 22), ("AF_IRDA", 23),
    ("AF_PPPOX", 24), ("AF_WANPIPE", 25), ("AF_LLC", 26), ("AF_IB", 27), ("AF_MPLS", 28),
    ("AF_CAN", 29), ("AF_TIPC", 30), ("AF_BLUETOOTH", 31), ("AF_IUCV", 32), ("AF_RXRPC", 33),
    ("AF_ISDN", 34), ("AF_PHONET", 35), ("AF_IEEE802154", 36), ("AF_CAIF", 37), ("AF_ALG", 38),
    ("AF_NFC", 39), ("AF_VSOCK", 40), ("AF_KCM", 41), ("AF_QIPCRTR", 42), ("AF_SMC", 43),
    ("AF_XDP", 44), ("AF_MCTP", 45),
];
const AF_MAX: u64 = 46;

/// RestrictAddressFamilies= lines, merged like SystemCallFilter=: the first
/// decides between allow list and deny list (`~`), later lines of the other
/// kind remove entries. "none" is an empty allow list. Unknown names are
/// ignored. None: no restriction.
pub fn parse_address_families(lines: &[String]) -> Option<(bool, std::collections::BTreeSet<u64>)> {
    let mut out: Option<(bool, std::collections::BTreeSet<u64>)> = None;
    for line in lines {
        let (invert, rest) = match line.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, line.as_str()),
        };
        let (allow, set) = out.get_or_insert_with(|| (!invert, Default::default()));
        let add = invert != *allow;
        for w in rest.split_whitespace() {
            if w == "none" {
                if !invert {
                    *allow = true;
                    set.clear();
                }
                continue;
            }
            if let Some(&(_, n)) = ADDRESS_FAMILIES.iter().find(|(name, _)| *name == w) {
                if add {
                    set.insert(n);
                } else {
                    set.remove(&n);
                }
            }
        }
    }
    out
}

const NAMESPACES: &[(&str, u64)] = &[
    ("cgroup", libc::CLONE_NEWCGROUP as u64),
    ("ipc", libc::CLONE_NEWIPC as u64),
    ("net", libc::CLONE_NEWNET as u64),
    ("mnt", libc::CLONE_NEWNS as u64),
    ("pid", libc::CLONE_NEWPID as u64),
    ("user", libc::CLONE_NEWUSER as u64),
    ("uts", libc::CLONE_NEWUTS as u64),
    ("time", 0x80), // CLONE_NEWTIME
];

fn all_namespaces() -> u64 {
    NAMESPACES.iter().fold(0, |a, (_, f)| a | f)
}

/// RestrictNamespaces=: the namespace flags that stay allowed. A boolean
/// (yes: none allowed), a list of allowed types or `~` and the denied ones.
/// None: not set, or nothing restricted.
pub fn parse_namespaces(lines: &[String]) -> Option<u64> {
    let all = all_namespaces();
    let mut retain: Option<u64> = None;
    for line in lines {
        if let Some(b) = crate::unit::parse_bool(line) {
            retain = Some(if b { 0 } else { all });
            continue;
        }
        let (invert, rest) = match line.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, line.as_str()),
        };
        let bits = rest
            .split_whitespace()
            .filter_map(|w| NAMESPACES.iter().find(|(n, _)| *n == w).map(|(_, f)| *f))
            .fold(0, |a, f| a | f);
        retain = Some(if invert { retain.unwrap_or(all) & !bits } else { retain.filter(|&r| r != all).unwrap_or(0) | bits });
    }
    retain.filter(|&r| r != all)
}

/// The restriction filters a unit asks for, in the order systemd loads them.
pub fn restrictions(unit: &Unit) -> Vec<Restriction> {
    let on = |key: &str| unit.get_bool("Service", key, false);
    let mut out = Vec::new();
    let groups = groups();
    let deny_group = |what: &'static str, group: &str| -> Restriction {
        let mut names = Vec::new();
        let _ = expand(group, &groups, &mut names, 0);
        Restriction { what, rules: names.iter().map(|n| rule(n, libc::EPERM, vec![])).collect(), native_only: false }
    };

    if let Some((allow, set)) = parse_address_families(&unit.get_all("Service", "RestrictAddressFamilies")) {
        let rules = if allow {
            (0..AF_MAX)
                .filter(|f| !set.contains(f))
                .map(|f| rule("socket", libc::EAFNOSUPPORT, vec![arg_eq(0, f)]))
                .chain(std::iter::once(rule("socket", libc::EAFNOSUPPORT, vec![arg_gt(0, AF_MAX - 1)])))
                .collect()
        } else {
            set.iter().map(|&f| rule("socket", libc::EAFNOSUPPORT, vec![arg_eq(0, f)])).collect()
        };
        out.push(Restriction { what: "RestrictAddressFamilies=", rules, native_only: true });
    }
    if let Some(retain) = parse_namespaces(&unit.get_all("Service", "RestrictNamespaces")) {
        let mut rules = vec![
            // Can't look into clone3()'s struct: make glibc fall back to clone().
            rule("clone3", libc::ENOSYS, vec![]),
            if retain == 0 { rule("setns", libc::EPERM, vec![]) } else { rule("setns", libc::EPERM, vec![arg_eq(1, 0)]) },
        ];
        let clone_arg = if cfg!(target_arch = "s390x") { 1 } else { 0 };
        for &(_, f) in NAMESPACES.iter().filter(|(_, f)| retain & f == 0) {
            rules.push(rule("unshare", libc::EPERM, vec![arg_has(0, f)]));
            rules.push(rule("clone", libc::EPERM, vec![arg_has(clone_arg, f)]));
            if retain != 0 {
                rules.push(rule("setns", libc::EPERM, vec![arg_has(1, f)]));
            }
        }
        out.push(Restriction { what: "RestrictNamespaces=", rules, native_only: false });
    }
    if on("MemoryDenyWriteExecute") {
        let wx = (libc::PROT_WRITE | libc::PROT_EXEC) as u64;
        let x = libc::PROT_EXEC as u64;
        out.push(Restriction {
            what: "MemoryDenyWriteExecute=",
            rules: vec![
                rule("mmap", libc::EPERM, vec![arg_has(2, wx)]),
                rule("mmap2", libc::EPERM, vec![arg_has(2, wx)]),
                rule("mprotect", libc::EPERM, vec![arg_has(2, x)]),
                rule("pkey_mprotect", libc::EPERM, vec![arg_has(2, x)]),
                rule("shmat", libc::EPERM, vec![arg_has(2, libc::SHM_EXEC as u64)]),
            ],
            native_only: false,
        });
    }
    if on("RestrictRealtime") {
        // Allowed: SCHED_OTHER (0), SCHED_BATCH (3), SCHED_IDLE (5).
        let mut rules: Vec<Rule> =
            [1u64, 2, 4].iter().map(|&p| rule("sched_setscheduler", libc::EPERM, vec![arg_eq(1, p)])).collect();
        rules.push(rule("sched_setscheduler", libc::EPERM, vec![arg_gt(1, 5)]));
        out.push(Restriction { what: "RestrictRealtime=", rules, native_only: false });
    }
    if on("LockPersonality") {
        let current = unsafe { libc::personality(0xffff_ffff) } as u64;
        out.push(Restriction {
            what: "LockPersonality=",
            rules: vec![rule("personality", libc::EPERM, vec![arg_ne(0, current)])],
            native_only: false,
        });
    }
    if on("RestrictSUIDSGID") {
        let mut rules = vec![rule("openat2", libc::ENOSYS, vec![])];
        for bit in [libc::S_ISUID as u64, libc::S_ISGID as u64] {
            for (sc, mode_arg) in [("chmod", 1), ("fchmod", 1), ("fchmodat", 2), ("fchmodat2", 2), ("creat", 1)] {
                rules.push(rule(sc, libc::EPERM, vec![arg_has(mode_arg, bit)]));
            }
            for flag in [libc::O_CREAT as u64, libc::O_TMPFILE as u64] {
                rules.push(rule("open", libc::EPERM, vec![arg_has(1, flag), arg_has(2, bit)]));
                rules.push(rule("openat", libc::EPERM, vec![arg_has(2, flag), arg_has(3, bit)]));
            }
        }
        out.push(Restriction { what: "RestrictSUIDSGID=", rules, native_only: false });
    }
    if on("ProtectClock") {
        out.push(deny_group("ProtectClock=", "@clock"));
    }
    if on("ProtectKernelModules") {
        out.push(deny_group("ProtectKernelModules=", "@module"));
    }
    if on("ProtectKernelLogs") {
        out.push(Restriction { what: "ProtectKernelLogs=", rules: vec![rule("syslog", libc::EPERM, vec![])], native_only: false });
    }
    // "private" allows changing the hostname inside the namespace.
    if on("ProtectHostname") {
        out.push(Restriction {
            what: "ProtectHostname=",
            rules: vec![rule("sethostname", libc::EPERM, vec![]), rule("setdomainname", libc::EPERM, vec![])],
            native_only: false,
        });
    }
    if on("PrivateDevices") {
        out.push(deny_group("PrivateDevices=", "@raw-io"));
    }
    out
}

/// PR_SET_MDWE (Linux 6.3): MemoryDenyWriteExecute= without seccomp, as
/// systemd uses it when the kernel has it. Inherited across execve().
pub fn set_mdwe() -> bool {
    const PR_SET_MDWE: libc::c_int = 65;
    const PR_MDWE_REFUSE_EXEC_GAIN: libc::c_ulong = 1;
    unsafe { libc::prctl(PR_SET_MDWE, PR_MDWE_REFUSE_EXEC_GAIN, 0, 0, 0) == 0 }
}

/// Load the unit's restriction filters into this process. Before
/// SystemCallFilter=, whose allow list may not include seccomp() itself.
pub fn apply_restrictions(unit: &Unit) -> Result<(), String> {
    let mut list = restrictions(unit);
    if list.iter().any(|r| r.what == "MemoryDenyWriteExecute=") && set_mdwe() {
        list.retain(|r| r.what != "MemoryDenyWriteExecute=");
    }
    if list.is_empty() {
        return Ok(());
    }
    let lib = match Lib::open() {
        Ok(l) => l,
        Err(e) => {
            let names: Vec<&str> = list.iter().map(|r| r.what).collect();
            eprintln!("sdcompat: {e}; {} skipped", names.join(" "));
            return Ok(());
        }
    };
    for r in &list {
        r.apply(&lib)?;
    }
    Ok(())
}

impl Restriction {
    fn apply(&self, lib: &Lib) -> Result<(), String> {
        let native = unsafe { (lib.arch_native)() };
        let mut archs = vec![native];
        if !self.native_only {
            let extra: &[&str] = match std::env::consts::ARCH {
                "x86_64" => &["x86", "x32"],
                "aarch64" => &["arm"],
                _ => &[],
            };
            archs.extend(extra.iter().map(|a| lib.arch(a)).filter(|&t| t != 0 && t != native));
        } else if std::env::consts::ARCH == "x86_64" {
            // x32 has socket() of its own.
            archs.extend([lib.arch("x32")].into_iter().filter(|&t| t != 0));
        }
        for arch in archs {
            unsafe {
                let ctx = (lib.init)(SCMP_ACT_ALLOW);
                if ctx.is_null() {
                    return Err(format!("{}: seccomp_init failed", self.what));
                }
                (lib.attr_set)(ctx, SCMP_FLTATR_CTL_NNP, u32::from(!have_sys_admin()));
                // Other ABIs are left to their own filter.
                (lib.attr_set)(ctx, SCMP_FLTATR_ACT_BADARCH, SCMP_ACT_ALLOW);
                if arch != native {
                    (lib.arch_add)(ctx, arch);
                    (lib.arch_remove)(ctx, native);
                }
                let mut added = 0;
                for r in &self.rules {
                    let c = CString::new(r.syscall.as_str()).unwrap_or_default();
                    let nr = (lib.resolve_name)(c.as_ptr());
                    if nr == NR_SCMP_ERROR {
                        continue;
                    }
                    let act = SCMP_ACT_ERRNO | (r.errno as u32 & 0xffff);
                    let args = if r.args.is_empty() { std::ptr::null() } else { r.args.as_ptr() as *const c_void };
                    // Like systemd: a syscall this ABI lacks is skipped.
                    if (lib.rule_add_array)(ctx, act, nr, r.args.len() as c_uint, args) == 0 {
                        added += 1;
                    }
                }
                let res = if added > 0 { (lib.load)(ctx) } else { 0 };
                (lib.release)(ctx);
                if res < 0 {
                    return Err(format!("{}: seccomp_load: {}", self.what, std::io::Error::from_raw_os_error(-res)));
                }
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
    fn address_families() {
        let (allow, set) = parse_address_families(&lines(&["AF_UNIX AF_INET AF_INET6"])).unwrap();
        assert!(allow);
        assert_eq!(set.into_iter().collect::<Vec<_>>(), vec![1, 2, 10]);
        let (allow, set) = parse_address_families(&lines(&["~AF_PACKET AF_NETLINK", "AF_NETLINK"])).unwrap();
        assert!(!allow);
        assert_eq!(set.into_iter().collect::<Vec<_>>(), vec![17]);
        let (allow, set) = parse_address_families(&lines(&["none"])).unwrap();
        assert!(allow && set.is_empty());
        assert_eq!(parse_address_families(&[]), None);
    }

    #[test]
    fn namespaces() {
        let all = all_namespaces();
        assert_eq!(parse_namespaces(&lines(&["yes"])), Some(0));
        assert_eq!(parse_namespaces(&lines(&["no"])), None);
        assert_eq!(parse_namespaces(&lines(&["net mnt"])), Some((libc::CLONE_NEWNET | libc::CLONE_NEWNS) as u64));
        assert_eq!(parse_namespaces(&lines(&["~user"])), Some(all & !(libc::CLONE_NEWUSER as u64)));
        assert_eq!(parse_namespaces(&lines(&["~cgroup user pid net uts mnt ipc time"])), Some(0));
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

    fn test_unit(text: &str) -> Unit {
        let mut file = crate::unit::UnitFile::default();
        file.parse_str(text);
        Unit {
            id: "t.service".into(),
            names: vec!["t.service".into()],
            fragment: None,
            dropins: Vec::new(),
            load_state: crate::loader::LoadState::Loaded,
            load_error: None,
            file,
        }
    }

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

    /// RestrictAddressFamilies=AF_UNIX and RestrictNamespaces=yes in a child.
    #[test]
    fn restrictions_take_effect() {
        if unsafe { libc::dlopen(c"libseccomp.so.2".as_ptr(), libc::RTLD_NOW) }.is_null() {
            eprintln!("libseccomp.so.2 not available, skipping");
            return;
        }
        let unit = test_unit("[Service]\nRestrictAddressFamilies=AF_UNIX\nRestrictNamespaces=yes\nRestrictRealtime=yes\n");
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let code = (|| {
                if apply_restrictions(&unit).is_err() {
                    return 2;
                }
                let errno = || std::io::Error::last_os_error().raw_os_error();
                let s = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
                if s >= 0 || errno() != Some(libc::EAFNOSUPPORT) {
                    return 3;
                }
                if unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) } < 0 {
                    return 4;
                }
                if unsafe { libc::unshare(libc::CLONE_NEWUSER) } == 0 || errno() != Some(libc::EPERM) {
                    return 5;
                }
                let p = libc::sched_param { sched_priority: 1 };
                if unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &p) } == 0 || errno() != Some(libc::EPERM) {
                    return 6;
                }
                0
            })();
            unsafe { libc::_exit(code) };
        }
        let mut st = 0;
        unsafe { libc::waitpid(pid, &mut st, 0) };
        assert!(libc::WIFEXITED(st), "child died: {st}");
        assert_eq!(libc::WEXITSTATUS(st), 0, "2 apply, 3 AF_INET, 4 AF_UNIX, 5 unshare, 6 SCHED_FIFO");
    }
}
