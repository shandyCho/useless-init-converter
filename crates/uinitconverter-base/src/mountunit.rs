//! .mount and .swap units: oneshots that call mount(8)/swapon(8). Their state
//! is read from the kernel (mountinfo, /proc/swaps), not from s6-rc.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::loader::Unit;
use crate::tools;
use crate::util;

/// Mount point of a .mount unit: Where=, or derived from the unit name.
pub fn where_of(unit: &Unit) -> String {
    if let Some(w) = unit.get("Mount", "Where") {
        return w;
    }
    let stem = unit.id.trim_end_matches(".mount");
    if stem == "-" {
        "/".into()
    } else {
        format!("/{}", util::unit_name_unescape(stem))
    }
}

/// Mount points currently mounted (octal escapes in mountinfo decoded).
pub fn mounted_points() -> Vec<String> {
    // Bytes: one mount point that isn't UTF-8 must not hide all the others.
    let text = String::from_utf8_lossy(&fs::read("/proc/self/mountinfo").unwrap_or_default()).into_owned();
    text.lines()
        .filter_map(|l| l.split_whitespace().nth(4))
        .map(decode_octal)
        .collect()
}

fn decode_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            // From the bytes: a str slice could split a character.
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 4]).ok().and_then(|o| u8::from_str_radix(o, 8).ok()) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn is_mounted(where_: &str) -> bool {
    let w = where_.trim_end_matches('/');
    let w = if w.is_empty() { "/" } else { w };
    mounted_points().iter().any(|m| m == w)
}

pub fn swap_active(what: &str) -> bool {
    let real = fs::canonicalize(what).map(|p| p.display().to_string()).unwrap_or_else(|_| what.to_string());
    fs::read_to_string("/proc/swaps")
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .any(|d| d == real || d == what)
}

fn run(cmd: &mut Command) -> i32 {
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("uinitconverter: {cmd:?}: {e}");
            1
        }
    }
}

pub fn mount_up(unit: &Unit) -> i32 {
    let where_ = where_of(unit);
    if is_mounted(&where_) {
        return 0;
    }
    let Some(what) = unit.get("Mount", "What") else {
        eprintln!("uinitconverter: {}: What= is missing", unit.id);
        return 1;
    };
    if !Path::new(&where_).exists() {
        let _ = fs::create_dir_all(&where_);
        if let Some(mode) = unit.get("Mount", "DirectoryMode").and_then(|m| u32::from_str_radix(&m, 8).ok()) {
            if let Ok(c) = std::ffi::CString::new(where_.as_str()) {
                unsafe { libc::chmod(c.as_ptr(), mode) };
            }
        }
    }
    let mut cmd = Command::new(tools::path("mount"));
    if let Some(t) = unit.get("Mount", "Type") {
        cmd.arg("-t").arg(t);
    }
    if unit.get_bool("Mount", "SloppyOptions", false) {
        cmd.arg("-s");
    }
    // x-systemd.* options are meaningless to mount(8).
    let opts: Vec<String> = unit
        .get("Mount", "Options")
        .unwrap_or_default()
        .split(',')
        .filter(|o| !o.is_empty() && !o.starts_with("x-systemd.") && *o != "nofail")
        .map(String::from)
        .collect();
    if !opts.is_empty() {
        cmd.arg("-o").arg(opts.join(","));
    }
    cmd.arg(&what).arg(&where_);
    run(&mut cmd)
}

pub fn mount_down(unit: &Unit) -> i32 {
    let where_ = where_of(unit);
    if !is_mounted(&where_) {
        return 0;
    }
    let mut cmd = Command::new(tools::path("umount"));
    if unit.get_bool("Mount", "LazyUnmount", false) {
        cmd.arg("-l");
    }
    if unit.get_bool("Mount", "ForceUnmount", false) {
        cmd.arg("-f");
    }
    cmd.arg(&where_);
    run(&mut cmd)
}

pub fn swap_up(unit: &Unit) -> i32 {
    let Some(what) = unit.get("Swap", "What") else { return 1 };
    if swap_active(&what) {
        return 0;
    }
    let mut cmd = Command::new(tools::path("swapon"));
    if let Some(p) = unit.get("Swap", "Priority") {
        cmd.arg("-p").arg(p);
    }
    if let Some(o) = unit.get("Swap", "Options") {
        cmd.arg("-o").arg(o);
    }
    cmd.arg(&what);
    run(&mut cmd)
}

pub fn swap_down(unit: &Unit) -> i32 {
    let Some(what) = unit.get("Swap", "What") else { return 0 };
    if !swap_active(&what) {
        return 0;
    }
    run(Command::new(tools::path("swapoff")).arg(&what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal() {
        assert_eq!(decode_octal("/mnt/a\\040b"), "/mnt/a b");
        assert_eq!(decode_octal("/mnt/\\é1"), "/mnt/\\é1");
    }
}
