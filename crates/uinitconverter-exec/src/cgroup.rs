//! cgroup v2 layout compatible with systemd: /sys/fs/cgroup/system.slice/<unit>.
//!
//! Using the same paths means sd_pid_get_unit(), `systemctl status` process
//! trees and resource-control settings work as on systemd.

use std::fs;
use std::path::{Path, PathBuf};

use crate::loader::Unit;

pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

pub fn available() -> bool {
    Path::new(CGROUP_ROOT).join("cgroup.controllers").exists()
}

/// "foo-bar.slice" -> "foo.slice/foo-bar.slice"
pub fn slice_path(slice: &str) -> PathBuf {
    let stem = slice.trim_end_matches(".slice");
    if stem == "-" || stem.is_empty() {
        return PathBuf::new();
    }
    let mut p = PathBuf::new();
    let mut acc = String::new();
    for part in stem.split('-') {
        if !acc.is_empty() {
            acc.push('-');
        }
        acc.push_str(part);
        p.push(format!("{acc}.slice"));
    }
    p
}

/// Parent slice of a slice: "user-1000.slice" -> "user.slice", "user.slice" -> "-.slice".
pub fn parent_slice(slice: &str) -> String {
    let stem = slice.trim_end_matches(".slice");
    match stem.rsplit_once('-') {
        Some((p, _)) if !p.is_empty() => format!("{p}.slice"),
        _ => "-.slice".into(),
    }
}

pub fn slice_of(unit: &Unit) -> String {
    unit.get("Service", "Slice")
        .or_else(|| unit.get("Scope", "Slice"))
        .unwrap_or_else(|| "system.slice".into())
}

/// Relative cgroup path as systemd reports it in ControlGroup (leading '/').
pub fn unit_cgroup_rel(unit_id: &str, slice: &str) -> String {
    format!("/{}", slice_path(slice).join(unit_id).display())
}

pub fn unit_cgroup_dir(unit_id: &str, slice: &str) -> PathBuf {
    Path::new(CGROUP_ROOT).join(slice_path(slice)).join(&*crate::util::path_component(unit_id))
}

/// Enable the controllers we may need along the path from the root.
fn enable_controllers(dir: &Path) {
    let root = Path::new(CGROUP_ROOT);
    let Ok(rel) = dir.strip_prefix(root) else { return };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        let avail = fs::read_to_string(cur.join("cgroup.controllers")).unwrap_or_default();
        for c in ["cpu", "memory", "pids", "io"] {
            if avail.split_whitespace().any(|a| a == c) {
                let _ = fs::write(cur.join("cgroup.subtree_control"), format!("+{c}"));
            }
        }
        cur.push(comp);
    }
}

fn parse_bytes(v: &str) -> Option<String> {
    let v = v.trim();
    if v == "infinity" {
        return Some("max".into());
    }
    if let Some(pct) = v.strip_suffix('%') {
        let pct: f64 = pct.parse().ok()?;
        let total = fs::read_to_string("/proc/meminfo")
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))
            .and_then(|l| l.trim().trim_end_matches("kB").trim().parse::<u64>().ok())?
            * 1024;
        return Some(((total as f64) * pct / 100.0) as u64).map(|b| b.to_string());
    }
    let (num, mult) = match v.chars().last()? {
        'K' => (&v[..v.len() - 1], 1u64 << 10),
        'M' => (&v[..v.len() - 1], 1 << 20),
        'G' => (&v[..v.len() - 1], 1 << 30),
        'T' => (&v[..v.len() - 1], 1 << 40),
        _ => (v, 1),
    };
    let n: f64 = num.parse().ok()?;
    Some(((n * mult as f64) as u64).to_string())
}

fn chown_delegated(dir: &Path, uid: u32, gid: u32) {
    // cgroup v2 delegation: the directory and these three files.
    for p in [dir.to_path_buf(), dir.join("cgroup.procs"), dir.join("cgroup.threads"), dir.join("cgroup.subtree_control")] {
        if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
            unsafe { libc::chown(c.as_ptr(), uid, gid) };
        }
    }
}

/// Create the unit's cgroup, apply resource settings and move `pid` into it.
/// With Delegate=, the cgroup (and DelegateSubgroup=, which receives the
/// process) is handed to `owner`.
pub fn enter(unit: &Unit, pid: u32, owner: Option<(u32, u32)>) -> Result<PathBuf, String> {
    if !available() {
        return Err("cgroup2 not mounted".into());
    }
    let dir = unit_cgroup_dir(&unit.id, &slice_of(unit));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    enable_controllers(&dir);

    let sec = unit.type_section();
    let set = |file: &str, val: String| {
        if let Err(e) = fs::write(dir.join(file), &val) {
            eprintln!("uinitconverter: {}: cannot set {file}={val}: {e}", unit.id);
        }
    };
    if let Some(v) = unit.get(&sec, "MemoryMax").or_else(|| unit.get(&sec, "MemoryLimit")) {
        if let Some(b) = parse_bytes(&v) {
            set("memory.max", b);
        }
    }
    if let Some(b) = unit.get(&sec, "MemoryHigh").and_then(|v| parse_bytes(&v)) {
        set("memory.high", b);
    }
    if let Some(v) = unit.get(&sec, "CPUQuota") {
        if let Ok(pct) = v.trim_end_matches('%').parse::<f64>() {
            set("cpu.max", format!("{} 100000", (pct * 1000.0) as u64));
        }
    }
    if let Some(v) = unit.get(&sec, "CPUWeight") {
        set("cpu.weight", v.replace("idle", "1"));
    }
    if let Some(v) = unit.get(&sec, "IOWeight") {
        set("io.weight", format!("default {v}"));
    }
    if let Some(v) = unit.get(&sec, "TasksMax") {
        let val = if v == "infinity" {
            "max".to_string()
        } else if let Some(p) = v.strip_suffix('%') {
            let max: u64 = fs::read_to_string("/proc/sys/kernel/pid_max")
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(4194304);
            let pct: f64 = p.parse().unwrap_or(100.0);
            ((max as f64 * pct / 100.0) as u64).to_string()
        } else {
            v
        };
        set("pids.max", val);
    }

    let delegate = unit
        .get(&sec, "Delegate")
        .map(|v| crate::unit::parse_bool(&v) != Some(false))
        .unwrap_or(false);
    let mut target = dir.clone();
    if delegate {
        if let Some(sub) = unit.get(&sec, "DelegateSubgroup") {
            target = dir.join(sub);
            fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        if let Some((uid, gid)) = owner {
            chown_delegated(&dir, uid, gid);
            if target != dir {
                chown_delegated(&target, uid, gid);
            }
        }
    }
    fs::write(target.join("cgroup.procs"), pid.to_string())
        .map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(dir)
}

/// MEMORY_PRESSURE_WATCH= and MEMORY_PRESSURE_WRITE= for the unit's
/// processes, as systemd's exec_invoke() sets them (MemoryPressureWatch=,
/// MemoryPressureThresholdSec=). Without them sd-event's
/// sd_event_add_memory_pressure() looks for the file itself and first checks
/// /proc/pressure, which the service's SELinux domain may not search
/// (systemd_hostnamed_t and proc_psi_t on Fedora). `dir` is the unit's
/// cgroup, None if it has none; "off" still gives /dev/null then.
pub fn memory_pressure_env(unit: &Unit, dir: Option<&Path>) -> Vec<(String, String)> {
    let sec = unit.type_section();
    pressure_env(
        unit.get(&sec, "MemoryPressureWatch").as_deref(),
        unit.get(&sec, "MemoryPressureThresholdSec").as_deref(),
        dir.map(|d| d.join("memory.pressure")).as_deref(),
    )
}

fn pressure_env(watch: Option<&str>, threshold: Option<&str>, file: Option<&Path>) -> Vec<(String, String)> {
    let watch = watch.unwrap_or("auto");
    if watch == "skip" {
        return Vec::new();
    }
    if crate::unit::parse_bool(watch) == Some(false) {
        return vec![("MEMORY_PRESSURE_WATCH".into(), "/dev/null".into())];
    }
    // No file: no cgroup, no memory controller or no PSI; systemd sets nothing.
    let Some(file) = file.filter(|f| f.exists()) else {
        return Vec::new();
    };
    const WINDOW_USEC: u64 = 2_000_000;
    let usec = match threshold.and_then(|v| crate::unit::parse_timespan_usec(v, 1_000_000)) {
        None | Some(u64::MAX) => 200_000,
        Some(u) => u.clamp(1, WINDOW_USEC),
    };
    // The trigger line is encoded with its trailing NUL, as systemd does.
    let write = base64(format!("some {usec} {WINDOW_USEC}\0").as_bytes());
    vec![
        ("MEMORY_PRESSURE_WATCH".into(), file.display().to_string()),
        ("MEMORY_PRESSURE_WRITE".into(), write),
    ]
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Signal every process in the unit's cgroup except `except`.
pub fn kill_all(dir: &Path, sig: i32, except: u32) -> usize {
    if sig == libc::SIGKILL && except == 0 && dir.join("cgroup.kill").exists() {
        let n = crate::util::cgroup_pids(dir).len();
        let _ = fs::write(dir.join("cgroup.kill"), "1");
        return n;
    }
    let mut n = 0;
    for pid in crate::util::cgroup_pids(dir) {
        if pid != except && pid != 0 {
            unsafe { libc::kill(pid as i32, sig) };
            n += 1;
        }
    }
    n
}

pub fn remove(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                remove(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

pub fn read_u64(dir: &Path, file: &str) -> Option<u64> {
    let s = fs::read_to_string(dir.join(file)).ok()?;
    s.split_whitespace().next()?.parse().ok()
}

pub fn cpu_usage_nsec(dir: &Path) -> Option<u64> {
    let s = fs::read_to_string(dir.join("cpu.stat")).ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|u| u * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices() {
        assert_eq!(slice_path("system.slice"), PathBuf::from("system.slice"));
        assert_eq!(
            slice_path("system-getty.slice"),
            PathBuf::from("system.slice/system-getty.slice")
        );
    }

    #[test]
    fn memory_pressure() {
        let dir = std::env::temp_dir().join(format!("uinitconverter-psi-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("memory.pressure");
        assert!(pressure_env(None, None, Some(&file)).is_empty());
        assert!(pressure_env(None, None, None).is_empty());
        assert_eq!(pressure_env(Some("no"), None, None)[0].1, "/dev/null");
        assert_eq!(pressure_env(Some("off"), None, Some(&file)), vec![("MEMORY_PRESSURE_WATCH".into(), "/dev/null".into())]);
        fs::write(&file, "").unwrap();
        assert!(pressure_env(Some("skip"), None, Some(&file)).is_empty());
        let env = pressure_env(None, None, Some(&file));
        assert_eq!(env[0].1, file.display().to_string());
        // base64("some 200000 2000000\0"), the value systemd writes.
        assert_eq!(env[1].1, "c29tZSAyMDAwMDAgMjAwMDAwMAA=");
        assert_eq!(pressure_env(Some("on"), Some("10s"), Some(&file))[1].1, base64(b"some 2000000 2000000\0"));
        assert_eq!(base64(b"ab"), "YWI=");
        fs::remove_dir_all(&dir).unwrap();
    }
}
