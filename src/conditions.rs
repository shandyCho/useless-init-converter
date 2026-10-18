//! Condition*= and Assert*= evaluation.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::loader::Unit;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub key: String,
    pub trigger: bool,
    pub negate: bool,
    pub param: String,
    /// 1 = passed, -1 = failed, 0 = not evaluated
    pub state: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    pub conditions_ok: bool,
    pub asserts_ok: bool,
    pub conditions: Vec<CheckResult>,
    pub asserts: Vec<CheckResult>,
    /// Human-readable reason for the first failure.
    pub failure: Option<String>,
}

pub fn evaluate(unit: &Unit) -> Evaluation {
    let mut ev = Evaluation { conditions_ok: true, asserts_ok: true, ..Default::default() };
    for (prefix, is_assert) in [("Condition", false), ("Assert", true)] {
        let mut results = Vec::new();
        for key in unit.file.keys_in("Unit") {
            let Some(kind) = key.strip_prefix(prefix) else { continue };
            for raw in unit.get_all("Unit", key) {
                let mut p = raw.as_str();
                let trigger = p.starts_with('|');
                if trigger {
                    p = p[1..].trim_start();
                }
                let negate = p.starts_with('!');
                if negate {
                    p = p[1..].trim_start();
                }
                let ok = check(kind, p) != negate;
                results.push(CheckResult {
                    key: key.to_string(),
                    trigger,
                    negate,
                    param: p.to_string(),
                    state: if ok { 1 } else { -1 },
                });
            }
        }
        let normal_ok = results.iter().filter(|r| !r.trigger).all(|r| r.state > 0);
        let triggers: Vec<_> = results.iter().filter(|r| r.trigger).collect();
        let trigger_ok = triggers.is_empty() || triggers.iter().any(|r| r.state > 0);
        let ok = normal_ok && trigger_ok;
        if !ok && ev.failure.is_none() {
            if let Some(r) = results.iter().find(|r| r.state < 0 && (!r.trigger || !trigger_ok)) {
                ev.failure = Some(format!(
                    "{}={}{} was not met",
                    r.key,
                    if r.negate { "!" } else { "" },
                    r.param
                ));
            }
        }
        if is_assert {
            ev.asserts_ok = ok;
            ev.asserts = results;
        } else {
            ev.conditions_ok = ok;
            ev.conditions = results;
        }
    }
    ev
}

fn check(kind: &str, p: &str) -> bool {
    let path = Path::new(p);
    match kind {
        "PathExists" => fs::symlink_metadata(path).is_ok() && path.exists(),
        "PathExistsGlob" => glob_any(p),
        "PathIsDirectory" => path.is_dir(),
        "PathIsSymbolicLink" => fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false),
        "PathIsMountPoint" => is_mount_point(path),
        "PathIsReadWrite" => access(p, libc::W_OK) && !is_ro_mount(path),
        "PathIsEncrypted" => false,
        "DirectoryNotEmpty" => fs::read_dir(path).map(|mut d| d.next().is_some()).unwrap_or(false),
        "FileNotEmpty" => fs::metadata(path).map(|m| m.is_file() && m.len() > 0).unwrap_or(false),
        "FileIsExecutable" => fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false),
        "KernelCommandLine" => kernel_cmdline_has(p),
        "Security" => security(p),
        "Virtualization" => virtualization(p),
        "Architecture" => p == crate::loader::systemd_arch() || p == "native",
        "Host" => p == crate::util::hostname() || p == crate::util::machine_id(),
        "FirstBoot" => crate::unit::parse_bool(p) == Some(false),
        "NeedsUpdate" => false,
        "ACPower" => true,
        "Capability" => true,
        "User" => p == "root" || p == "0" || p == "@system",
        "Group" => p == "root" || p == "0",
        "OSRelease" => true,
        "Environment" => std::env::var(p.split('=').next().unwrap_or(p)).is_ok(),
        // Unknown conditions are treated as passing.
        _ => true,
    }
}

fn access(p: &str, mode: i32) -> bool {
    let Ok(c) = std::ffi::CString::new(p) else { return false };
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

fn glob_any(pattern: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(pattern) else { return false };
    unsafe {
        let mut g: libc::glob_t = std::mem::zeroed();
        let r = libc::glob(c.as_ptr(), 0, None, &mut g);
        let found = r == 0 && g.gl_pathc > 0;
        libc::globfree(&mut g);
        found
    }
}

fn is_mount_point(p: &Path) -> bool {
    let (Ok(m), Some(parent)) = (fs::metadata(p), p.parent()) else { return false };
    if p == Path::new("/") {
        return true;
    }
    fs::metadata(parent).map(|pm| pm.dev() != m.dev()).unwrap_or(false)
}

fn is_ro_mount(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) else { return false };
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        libc::statvfs(c.as_ptr(), &mut st) == 0 && st.f_flag & libc::ST_RDONLY != 0
    }
}

fn kernel_cmdline_has(p: &str) -> bool {
    let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    cmdline.split_whitespace().any(|w| {
        if p.contains('=') {
            w == p
        } else {
            w == p || w.split_once('=').map(|(k, _)| k == p).unwrap_or(false)
        }
    })
}

fn security(p: &str) -> bool {
    match p {
        "selinux" => Path::new("/sys/fs/selinux/enforce").exists(),
        "apparmor" => Path::new("/sys/kernel/security/apparmor").exists(),
        "audit" => Path::new("/proc/self/loginuid").exists(),
        "uefi-secureboot" | "tpm2" | "cvm" | "measured-uki" => false,
        _ => false,
    }
}

pub fn detect_virtualization() -> &'static str {
    if Path::new("/run/.containerenv").exists() || Path::new("/.dockerenv").exists() {
        return "container-other";
    }
    if let Ok(c) = std::env::var("container") {
        if !c.is_empty() {
            return "container-other";
        }
    }
    let vendor = fs::read_to_string("/sys/class/dmi/id/sys_vendor").unwrap_or_default();
    let product = fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default();
    let v = format!("{vendor} {product}").to_lowercase();
    if v.contains("qemu") || v.contains("kvm") {
        return "kvm";
    }
    if v.contains("vmware") {
        return "vmware";
    }
    if v.contains("virtualbox") || v.contains("innotek") {
        return "oracle";
    }
    if v.contains("microsoft") {
        return "microsoft";
    }
    let cpu = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    if cpu.lines().any(|l| l.starts_with("flags") && l.contains(" hypervisor")) {
        return "vm-other";
    }
    "none"
}

fn virtualization(p: &str) -> bool {
    let v = detect_virtualization();
    let is_container = v.starts_with("container");
    match p {
        "vm" => v != "none" && !is_container,
        "container" => is_container,
        "private-users" => false,
        other => match crate::unit::parse_bool(other) {
            Some(b) => b == (v != "none"),
            None => other == v,
        },
    }
}
