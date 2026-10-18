//! Best-effort subset of systemd's sandboxing: PrivateTmp, ProtectSystem,
//! ProtectHome, ReadOnly/ReadWrite/InaccessiblePaths, NoNewPrivileges and
//! capability sets. Everything here runs in the forked child (or right before
//! the final exec), as root.

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
        Sandbox {
            private_tmp,
            protect_system: norm(ps),
            protect_home: norm(ph),
            read_write: path_entries(unit, "ReadWritePaths"),
            read_only: path_entries(unit, "ReadOnlyPaths"),
            inaccessible: path_entries(unit, "InaccessiblePaths"),
            writable_dirs,
        }
    }

    pub fn needed(&self) -> bool {
        self.private_tmp.is_some()
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
        Ok(())
    }

    /// Enter a new mount namespace and apply the settings. Must run as root.
    pub fn apply(&self) -> Result<(), String> {
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

        for (p, optional) in &self.read_only {
            if Path::new(p).exists() {
                bind_ro(p)?;
            } else if !optional {
                return Err(format!("ReadOnlyPaths: {p} does not exist"));
            }
        }
        for (p, optional) in &self.inaccessible {
            let path = Path::new(p);
            if path.is_dir() {
                mount("tmpfs", p, Some("tmpfs"), libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, Some("mode=0000"))?;
            } else if path.exists() {
                bind("/dev/null", p)?;
            } else if !optional {
                return Err(format!("InaccessiblePaths: {p} does not exist"));
            }
        }
        Ok(())
    }
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
