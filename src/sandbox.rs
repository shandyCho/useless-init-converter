//! Best-effort subset of systemd's sandboxing: PrivateTmp, ProtectSystem,
//! ProtectHome, ReadOnly/ReadWrite/InaccessiblePaths, the ProtectKernel*,
//! ProtectControlGroups, ProtectProc/ProcSubset, PrivateDevices,
//! PrivateNetwork, PrivateIPC, ProtectHostname and PrivateMounts namespaces
//! and mounts, NoNewPrivileges and capability sets. The syscall filters of
//! these settings are in seccomp.rs. Everything here runs in the forked child
//! (or right before the final exec), as root.

use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::loader::Unit;

fn cstr(p: &str) -> CString {
    CString::new(p).unwrap_or_default()
}

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn mount(src: &str, dst: &str, fstype: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> Result<(), String> {
    let s = cstr(src);
    let d = cstr(dst);
    let t = fstype.map(cstr);
    let o = data.map(cstr);
    let r = unsafe {
        libc::mount(
            s.as_ptr(),
            d.as_ptr(),
            t.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null()),
            flags,
            o.as_ref().map(|c| c.as_ptr() as *const libc::c_void).unwrap_or(std::ptr::null()),
        )
    };
    if r < 0 {
        Err(format!("mount {src} on {dst}: {}", errno()))
    } else {
        Ok(())
    }
}

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}
const MOUNT_ATTR_RDONLY: u64 = 0x1;
const AT_RECURSIVE: i32 = 0x8000;

/// Recursively set or clear read-only on the mount at `path`.
fn set_readonly(path: &str, ro: bool) -> Result<(), String> {
    let attr = MountAttr {
        attr_set: if ro { MOUNT_ATTR_RDONLY } else { 0 },
        attr_clr: if ro { 0 } else { MOUNT_ATTR_RDONLY },
        propagation: 0,
        userns_fd: 0,
    };
    let p = cstr(path);
    let r = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            libc::AT_FDCWD,
            p.as_ptr(),
            AT_RECURSIVE as libc::c_uint,
            &attr as *const MountAttr,
            std::mem::size_of::<MountAttr>(),
        )
    };
    if r == 0 {
        return Ok(());
    }
    // Fallback for old kernels: non-recursive remount.
    let flags = libc::MS_BIND | libc::MS_REMOUNT | if ro { libc::MS_RDONLY } else { 0 };
    mount("none", path, None, flags, None)
}

fn bind(src: &str, dst: &str) -> Result<(), String> {
    mount(src, dst, None, libc::MS_BIND | libc::MS_REC, None)
}

fn bind_ro(path: &str) -> Result<(), String> {
    bind(path, path)?;
    set_readonly(path, true)
}

fn bind_rw(path: &str) -> Result<(), String> {
    bind(path, path)?;
    set_readonly(path, false)
}

/// Split "-/path" (ignore missing) and "+/path" prefixes.
fn path_entries(unit: &Unit, key: &str) -> Vec<(String, bool)> {
    unit.get_words("Service", key)
        .into_iter()
        .map(|w| {
            let mut s = w.as_str();
            let mut optional = false;
            loop {
                if let Some(r) = s.strip_prefix('-') {
                    optional = true;
                    s = r;
                } else if let Some(r) = s.strip_prefix('+') {
                    s = r;
                } else {
                    break;
                }
            }
            (s.to_string(), optional)
        })
        .collect()
}

pub struct Sandbox {
    pub private_tmp: Option<(PathBuf, PathBuf)>,
    /// Mount point for building the PrivateDevices= /dev (unit state dir).
    pub private_dev: Option<PathBuf>,
    pub private_network: bool,
    pub private_ipc: bool,
    pub private_mounts: bool,
    pub protect_hostname: bool,
    pub protect_kernel_tunables: bool,
    pub protect_kernel_modules: bool,
    pub protect_kernel_logs: bool,
    pub protect_control_groups: bool,
    /// ProtectProc= as a hidepid= value ("" = default).
    pub protect_proc: String,
    /// ProcSubset=pid.
    pub proc_subset_pid: bool,
    pub protect_system: String,
    pub protect_home: String,
    pub read_write: Vec<(String, bool)>,
    pub read_only: Vec<(String, bool)>,
    pub inaccessible: Vec<(String, bool)>,
    /// Service directories that must stay writable (StateDirectory= etc).
    pub writable_dirs: Vec<String>,
}

impl Sandbox {
    pub fn from_unit(unit: &Unit, run_dir: &Path, writable_dirs: Vec<String>) -> Sandbox {
        let private_tmp = unit.get_bool("Service", "PrivateTmp", false).then(|| {
            let base = run_dir.join("private");
            (base.join("tmp"), base.join("var-tmp"))
        });
        let ps = unit.get("Service", "ProtectSystem").unwrap_or_default();
        let ph = unit.get("Service", "ProtectHome").unwrap_or_default();
        let norm = |v: String| match crate::unit::parse_bool(&v) {
            Some(true) => "yes".to_string(),
            Some(false) => String::new(),
            None => v,
        };
        let on = |key: &str| unit.get_bool("Service", key, false);
        // "private" (systemd 256): own namespace, hostname may change in it.
        let hostname = unit.get("Service", "ProtectHostname").unwrap_or_default();
        // "private"/"strict" also want a cgroup namespace; read-only is the
        // part both share with "yes".
        let cgroups = unit.get("Service", "ProtectControlGroups").unwrap_or_default();
        let protect_proc = match unit.get("Service", "ProtectProc").as_deref() {
            Some(v @ ("noaccess" | "invisible" | "ptraceable")) => v.to_string(),
            _ => String::new(),
        };
        Sandbox {
            private_tmp,
            private_dev: on("PrivateDevices").then(|| run_dir.join("private-dev")),
            private_network: on("PrivateNetwork"),
            private_ipc: on("PrivateIPC"),
            private_mounts: on("PrivateMounts"),
            protect_hostname: crate::unit::parse_bool(&hostname).unwrap_or(hostname == "private"),
            protect_kernel_tunables: on("ProtectKernelTunables"),
            protect_kernel_modules: on("ProtectKernelModules"),
            protect_kernel_logs: on("ProtectKernelLogs"),
            protect_control_groups: crate::unit::parse_bool(&cgroups).unwrap_or(matches!(cgroups.as_str(), "private" | "strict")),
            protect_proc,
            proc_subset_pid: unit.get("Service", "ProcSubset").as_deref() == Some("pid"),
            protect_system: norm(ps),
            protect_home: norm(ph),
            read_write: path_entries(unit, "ReadWritePaths"),
            read_only: path_entries(unit, "ReadOnlyPaths"),
            inaccessible: path_entries(unit, "InaccessiblePaths"),
            writable_dirs,
        }
    }

    /// A mount namespace is needed (the other namespaces are separate).
    pub fn needed(&self) -> bool {
        self.private_tmp.is_some()
            || self.private_dev.is_some()
            || self.private_ipc
            || self.private_mounts
            || self.protect_kernel_tunables
            || self.protect_kernel_modules
            || self.protect_kernel_logs
            || self.protect_control_groups
            || !self.protect_proc.is_empty()
            || self.proc_subset_pid
            || !self.protect_system.is_empty()
            || !self.protect_home.is_empty()
            || !self.read_only.is_empty()
            || !self.inaccessible.is_empty()
    }

    /// Prepare persistent per-unit directories (PrivateTmp is shared by all
    /// processes of one unit invocation).
    pub fn prepare(&self) -> Result<(), String> {
        if let Some((tmp, vtmp)) = &self.private_tmp {
            for d in [tmp, vtmp] {
                fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                let c = cstr(&d.display().to_string());
                unsafe { libc::chmod(c.as_ptr(), 0o1777) };
            }
        }
        if let Some(d) = &self.private_dev {
            fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        Ok(())
    }

    /// Enter the namespaces and apply the settings. Must run as root.
    pub fn apply(&self) -> Result<(), String> {
        let mut flags = 0;
        if self.private_network {
            flags |= libc::CLONE_NEWNET;
        }
        if self.private_ipc {
            flags |= libc::CLONE_NEWIPC;
        }
        if self.protect_hostname {
            flags |= libc::CLONE_NEWUTS;
        }
        // Each process of the unit gets its own: systemd shares one per unit.
        if flags != 0 && unsafe { libc::unshare(flags) } < 0 {
            return Err(format!("unshare(PrivateNetwork/PrivateIPC/ProtectHostname): {}", errno()));
        }
        if self.private_network {
            loopback_up()?;
        }
        if !self.needed() {
            return Ok(());
        }
        if unsafe { libc::unshare(libc::CLONE_NEWNS) } < 0 {
            return Err(format!("unshare(CLONE_NEWNS): {}", errno()));
        }
        mount("none", "/", None, libc::MS_REC | libc::MS_SLAVE, None)?;

        let mut rw: BTreeSet<String> = BTreeSet::new();
        match self.protect_system.as_str() {
            "" => {}
            "yes" | "full" => {
                let mut dirs = vec!["/usr", "/boot", "/efi"];
                if self.protect_system == "full" {
                    dirs.push("/etc");
                }
                for d in dirs {
                    if Path::new(d).is_dir() {
                        bind_ro(d)?;
                    }
                }
            }
            "strict" => {
                bind_ro("/")?;
                for d in ["/dev", "/proc", "/sys"] {
                    rw.insert(d.to_string());
                }
            }
            other => return Err(format!("unsupported ProtectSystem={other}")),
        }

        match self.protect_home.as_str() {
            "" => {}
            mode @ ("yes" | "read-only" | "tmpfs") => {
                for d in ["/home", "/root", "/run/user"] {
                    if !Path::new(d).is_dir() {
                        continue;
                    }
                    if mode == "read-only" {
                        bind_ro(d)?;
                    } else {
                        let opts = if mode == "yes" { "mode=0000" } else { "mode=0755" };
                        mount("tmpfs", d, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NODEV, Some(opts))?;
                    }
                }
            }
            other => return Err(format!("unsupported ProtectHome={other}")),
        }

        for (p, optional) in &self.read_write {
            if Path::new(p).exists() {
                rw.insert(p.clone());
            } else if !optional {
                return Err(format!("ReadWritePaths: {p} does not exist"));
            }
        }
        if self.protect_system == "strict" {
            for d in &self.writable_dirs {
                rw.insert(d.clone());
            }
        }
        for p in &rw {
            bind_rw(p)?;
        }

        if let Some((tmp, vtmp)) = &self.private_tmp {
            bind(&tmp.display().to_string(), "/tmp")?;
            if Path::new("/var/tmp").is_dir() {
                bind(&vtmp.display().to_string(), "/var/tmp")?;
            }
        }

        // A new procfs instance first: it would hide the binds below.
        if !self.protect_proc.is_empty() || self.proc_subset_pid {
            let mut opts = Vec::new();
            if !self.protect_proc.is_empty() {
                opts.push(format!("hidepid={}", self.protect_proc));
            }
            if self.proc_subset_pid {
                opts.push("subset=pid".to_string());
            }
            let fl = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
            // Older kernels lack per-instance options: go on without, as systemd.
            if let Err(e) = mount("proc", "/proc", Some("proc"), fl, Some(&opts.join(","))) {
                eprintln!("sdcompat: ProtectProc=/ProcSubset= skipped: {e}");
            }
        }
        if let Some(staging) = &self.private_dev {
            private_dev(&staging.display().to_string())?;
        }
        if self.private_ipc && Path::new("/dev/mqueue").is_dir() {
            // The mqueue mount shows the namespace it was mounted in.
            let _ = mount("mqueue", "/dev/mqueue", Some("mqueue"), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None);
        }
        if self.protect_kernel_tunables {
            for (p, inaccessible) in [
                ("/proc/acpi", false), ("/proc/apm", false), ("/proc/asound", false), ("/proc/bus", false),
                ("/proc/fs", false), ("/proc/irq", false), ("/proc/kallsyms", true), ("/proc/kcore", true),
                ("/proc/latency_stats", false), ("/proc/mtrr", false), ("/proc/scsi", false),
                ("/proc/sys", false), ("/proc/sysrq-trigger", false), ("/proc/timer_list", false),
                ("/sys", false),
            ] {
                if !Path::new(p).exists() {
                    continue;
                }
                if inaccessible {
                    make_inaccessible(p)?;
                } else {
                    bind_ro(p)?;
                }
            }
            // /sys went read-only with all its submounts: these stay as they were.
            for p in ["/sys/fs/cgroup", "/sys/fs/selinux"] {
                if Path::new(p).is_dir() && !(p == "/sys/fs/cgroup" && self.protect_control_groups) {
                    bind_rw(p)?;
                }
            }
        }
        if self.protect_control_groups && Path::new("/sys/fs/cgroup").is_dir() {
            bind_ro("/sys/fs/cgroup")?;
        }
        if self.protect_kernel_modules {
            for p in ["/usr/lib/modules", "/lib/modules"] {
                if Path::new(p).is_dir() {
                    make_inaccessible(p)?;
                }
            }
        }
        if self.protect_kernel_logs {
            for p in ["/proc/kmsg", "/dev/kmsg"] {
                if Path::new(p).exists() {
                    make_inaccessible(p)?;
                }
            }
        }
        // ReadWritePaths= below a path made read-only above (efivars under /sys).
        if self.protect_kernel_tunables || self.protect_control_groups {
            for (p, _) in &self.read_write {
                if (p.starts_with("/sys/") || p.starts_with("/proc/")) && Path::new(p).exists() {
                    bind_rw(p)?;
                }
            }
        }

        for (p, optional) in &self.read_only {
            if Path::new(p).exists() {
                bind_ro(p)?;
            } else if !optional {
                return Err(format!("ReadOnlyPaths: {p} does not exist"));
            }
        }
        for (p, optional) in &self.inaccessible {
            if Path::new(p).exists() {
                make_inaccessible(p)?;
            } else if !optional {
                return Err(format!("InaccessiblePaths: {p} does not exist"));
            }
        }
        Ok(())
    }
}

/// Hide a path: an empty read-only directory, or /dev/null over a file.
fn make_inaccessible(p: &str) -> Result<(), String> {
    if Path::new(p).is_dir() {
        mount("tmpfs", p, Some("tmpfs"), libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, Some("mode=0000"))
    } else {
        bind("/dev/null", p)
    }
}

/// PrivateDevices=: a /dev with only the pseudo devices, like systemd's. The
/// nodes are bind mounts of the real ones (keeps their SELinux labels).
fn private_dev(staging: &str) -> Result<(), String> {
    mount("tmpfs", staging, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_STRICTATIME, Some("mode=0755"))?;
    let at = |n: &str| format!("{staging}/{n}");
    for n in ["null", "zero", "full", "random", "urandom", "tty"] {
        let src = format!("/dev/{n}");
        if !Path::new(&src).exists() {
            continue;
        }
        fs::File::create(at(n)).map_err(|e| format!("{}: {e}", at(n)))?;
        bind(&src, &at(n))?;
    }
    let _ = fs::create_dir(at("pts"));
    let tty_gid = crate::util::lookup_group("tty").unwrap_or(5);
    let pts_flags = libc::MS_NOSUID | libc::MS_NOEXEC;
    let opts = "newinstance,ptmxmode=0666,mode=0620";
    // gid= fails where the tty group isn't mapped (a user namespace).
    mount("devpts", &at("pts"), Some("devpts"), pts_flags, Some(&format!("{opts},gid={tty_gid}")))
        .or_else(|_| mount("devpts", &at("pts"), Some("devpts"), pts_flags, Some(opts)))?;
    let _ = std::os::unix::fs::symlink("pts/ptmx", at("ptmx"));
    for d in ["shm", "mqueue", "hugepages"] {
        let src = format!("/dev/{d}");
        if Path::new(&src).is_dir() {
            let _ = fs::create_dir(at(d));
            bind(&src, &at(d))?;
        }
    }
    for (n, target) in [("fd", "/proc/self/fd"), ("stdin", "/proc/self/fd/0"), ("stdout", "/proc/self/fd/1"), ("stderr", "/proc/self/fd/2")] {
        let _ = std::os::unix::fs::symlink(target, at(n));
    }
    // /dev/log: journald's syslog socket, a symlink on Fedora.
    if let Ok(target) = fs::read_link("/dev/log") {
        let _ = std::os::unix::fs::symlink(target, at("log"));
    } else if Path::new("/dev/log").exists() {
        let _ = fs::File::create(at("log"));
        let _ = bind("/dev/log", &at("log"));
    }
    mount(staging, "/dev", None, libc::MS_MOVE, None)
}

/// PrivateNetwork=: bring up "lo" in the new network namespace.
fn loopback_up() -> Result<(), String> {
    #[repr(C)]
    struct IfReq {
        name: [u8; 16],
        flags: libc::c_short,
        pad: [u8; 22],
    }
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("PrivateNetwork: socket: {}", errno()));
    }
    let mut ifr = IfReq { name: [0; 16], flags: 0, pad: [0; 22] };
    ifr.name[..2].copy_from_slice(b"lo");
    let r = unsafe {
        if libc::ioctl(fd, libc::SIOCGIFFLAGS, &mut ifr) < 0 {
            -1
        } else {
            ifr.flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            libc::ioctl(fd, libc::SIOCSIFFLAGS, &ifr)
        }
    };
    let e = errno();
    unsafe { libc::close(fd) };
    if r < 0 {
        return Err(format!("PrivateNetwork: bringing up lo: {e}"));
    }
    Ok(())
}

// ---- capabilities ----

const CAP_NAMES: &[&str] = &[
    "CHOWN", "DAC_OVERRIDE", "DAC_READ_SEARCH", "FOWNER", "FSETID", "KILL", "SETGID", "SETUID",
    "SETPCAP", "LINUX_IMMUTABLE", "NET_BIND_SERVICE", "NET_BROADCAST", "NET_ADMIN", "NET_RAW",
    "IPC_LOCK", "IPC_OWNER", "SYS_MODULE", "SYS_RAWIO", "SYS_CHROOT", "SYS_PTRACE", "SYS_PACCT",
    "SYS_ADMIN", "SYS_BOOT", "SYS_NICE", "SYS_RESOURCE", "SYS_TIME", "SYS_TTY_CONFIG", "MKNOD",
    "LEASE", "AUDIT_WRITE", "AUDIT_CONTROL", "SETFCAP", "MAC_OVERRIDE", "MAC_ADMIN", "SYSLOG",
    "WAKE_ALARM", "BLOCK_SUSPEND", "AUDIT_READ", "PERFMON", "BPF", "CHECKPOINT_RESTORE",
];

pub fn last_cap() -> u32 {
    fs::read_to_string("/proc/sys/kernel/cap_last_cap")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(40)
}

fn cap_number(name: &str) -> Option<u32> {
    let n = name.trim().trim_start_matches("CAP_").to_ascii_uppercase();
    CAP_NAMES.iter().position(|c| *c == n).map(|p| p as u32)
}

/// Parse a capability-set setting; None if never set.
pub fn parse_capset(unit: &Unit, key: &str) -> Option<u64> {
    let values = unit.get_all("Service", key);
    if values.is_empty() {
        return None;
    }
    let all: u64 = if last_cap() >= 63 { u64::MAX } else { (1u64 << (last_cap() + 1)) - 1 };
    let mut set: Option<u64> = None;
    for v in values {
        let (invert, list) = match v.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, v.as_str()),
        };
        let mut bits = 0u64;
        for w in list.split_whitespace() {
            if let Some(c) = cap_number(w) {
                bits |= 1 << c;
            }
        }
        set = Some(if invert {
            set.unwrap_or(all) & !bits
        } else {
            set.unwrap_or(0) | bits
        });
    }
    set
}

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// CapabilityBoundingSet= minus what the sandboxing settings imply, as
/// systemd's unit_patch_contexts(). None: leave the bounding set alone.
pub fn bounding_set(unit: &Unit) -> Option<u64> {
    let on = |key: &str| unit.get_bool("Service", key, false);
    let mut drop = 0u64;
    let cap = |n: &str| 1u64 << cap_number(n).unwrap_or(63);
    if on("PrivateDevices") {
        drop |= cap("MKNOD") | cap("SYS_RAWIO");
    }
    if on("ProtectKernelModules") {
        drop |= cap("SYS_MODULE");
    }
    if on("ProtectKernelLogs") {
        drop |= cap("SYSLOG");
    }
    if on("ProtectClock") {
        drop |= cap("SYS_TIME") | cap("WAKE_ALARM");
    }
    let set = parse_capset(unit, "CapabilityBoundingSet");
    if drop == 0 {
        return set;
    }
    let all: u64 = if last_cap() >= 63 { u64::MAX } else { (1u64 << (last_cap() + 1)) - 1 };
    Some(set.unwrap_or(all) & !drop)
}

/// NoNewPrivileges= as systemd's context_has_no_new_privileges(): set, or
/// implied by a sandboxing setting for a service without CAP_SYS_ADMIN (not
/// root, or dropped from its bounding set).
pub fn no_new_privileges(unit: &Unit, runs_as_root: bool) -> bool {
    if unit.get_bool("Service", "NoNewPrivileges", false) {
        return true;
    }
    let sys_admin = 1u64 << cap_number("SYS_ADMIN").unwrap_or(21);
    let has_sys_admin = runs_as_root && bounding_set(unit).map(|b| b & sys_admin != 0).unwrap_or(true);
    if has_sys_admin {
        return false;
    }
    let on = |key: &str| unit.get_bool("Service", key, false);
    [
        "LockPersonality", "MemoryDenyWriteExecute", "PrivateDevices", "ProtectClock", "ProtectHostname",
        "ProtectKernelTunables", "ProtectKernelModules", "ProtectKernelLogs", "RestrictRealtime", "RestrictSUIDSGID",
    ]
    .iter()
    .any(|k| on(k))
        || !unit.get_all("Service", "SystemCallFilter").is_empty()
        || !unit.get_all("Service", "SystemCallArchitectures").is_empty()
        || !unit.get_all("Service", "RestrictAddressFamilies").is_empty()
        || crate::seccomp::parse_namespaces(&unit.get_all("Service", "RestrictNamespaces")).is_some()
}

pub fn drop_bounding_set(keep: u64) -> Result<(), String> {
    for c in 0..=last_cap() {
        if keep & (1 << c) == 0 && unsafe { libc::prctl(libc::PR_CAPBSET_DROP, c as libc::c_ulong, 0, 0, 0) } < 0 {
            return Err(format!("PR_CAPBSET_DROP {c}: {}", errno()));
        }
    }
    Ok(())
}

/// After setuid(): set permitted/effective/inheritable to `caps` and raise them as ambient.
pub fn raise_ambient(caps: u64) -> Result<(), String> {
    let hdr = CapHeader { version: 0x2008_0522, pid: 0 };
    let lo = caps as u32;
    let hi = (caps >> 32) as u32;
    let data = [
        CapData { effective: lo, permitted: lo, inheritable: lo },
        CapData { effective: hi, permitted: hi, inheritable: hi },
    ];
    if unsafe { libc::syscall(libc::SYS_capset, &hdr as *const CapHeader, data.as_ptr()) } < 0 {
        return Err(format!("capset: {}", errno()));
    }
    for c in 0..=last_cap() {
        if caps & (1 << c) != 0
            && unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong, c as libc::c_ulong, 0, 0) } < 0
        {
            return Err(format!("PR_CAP_AMBIENT_RAISE {c}: {}", errno()));
        }
    }
    Ok(())
}
