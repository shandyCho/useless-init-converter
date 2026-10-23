//! Unit lookup: search path, aliases, templates, masking, drop-ins, specifiers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::unit::UnitFile;
use crate::util;

pub const UNIT_SUFFIXES: &[&str] = &[
    "service", "socket", "target", "timer", "path", "mount", "automount", "swap", "slice",
    "scope", "device",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    Loaded,
    NotFound,
    Masked,
    BadSetting,
}

impl LoadState {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoadState::Loaded => "loaded",
            LoadState::NotFound => "not-found",
            LoadState::Masked => "masked",
            LoadState::BadSetting => "bad-setting",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Unit {
    /// Canonical name (alias symlinks resolved, instance applied).
    pub id: String,
    /// All names this unit is known by (id first).
    pub names: Vec<String>,
    pub fragment: Option<PathBuf>,
    pub dropins: Vec<PathBuf>,
    pub load_state: LoadState,
    pub load_error: Option<String>,
    pub file: UnitFile,
}

/// Parsed pieces of a unit name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitName<'a> {
    pub prefix: &'a str,
    pub instance: Option<&'a str>,
    pub suffix: &'a str,
    pub is_template: bool,
}

pub fn parse_name(name: &str) -> Option<UnitName<'_>> {
    let (stem, suffix) = name.rsplit_once('.')?;
    if !UNIT_SUFFIXES.contains(&suffix) || stem.is_empty() || name.contains('/') {
        return None;
    }
    match stem.split_once('@') {
        Some((prefix, inst)) if !prefix.is_empty() => Some(UnitName {
            prefix,
            instance: (!inst.is_empty()).then_some(inst),
            suffix,
            is_template: inst.is_empty(),
        }),
        Some(_) => None,
        None => Some(UnitName { prefix: stem, instance: None, suffix, is_template: false }),
    }
}

pub fn unit_type(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, s)| s).unwrap_or("")
}

/// "foo@bar.service" -> "foo@.service"
pub fn template_of(name: &str) -> Option<String> {
    let n = parse_name(name)?;
    n.instance.map(|_| format!("{}@.{}", n.prefix, n.suffix))
}

fn is_dev_null(p: &Path) -> bool {
    fs::read_link(p).map(|t| t == Path::new("/dev/null")).unwrap_or(false)
}

/// Find the file for `name` in the search path; returns (path, masked).
fn find_fragment(cfg: &Config, name: &str) -> Option<(PathBuf, bool)> {
    for dir in &cfg.unit_path {
        let p = dir.join(name);
        if fs::symlink_metadata(&p).is_ok() {
            if is_dev_null(&p) {
                return Some((p, true));
            }
            if p.is_file() {
                return Some((p, false));
            }
        }
    }
    None
}

pub struct Loader<'a> {
    pub cfg: &'a Config,
}

impl<'a> Loader<'a> {
    pub fn new(cfg: &'a Config) -> Self {
        Loader { cfg }
    }

    pub fn load(&self, name: &str) -> Unit {
        let mut unit = Unit {
            id: name.to_string(),
            names: vec![name.to_string()],
            fragment: None,
            dropins: Vec::new(),
            load_state: LoadState::NotFound,
            load_error: None,
            file: UnitFile::default(),
        };
        let Some(parsed) = parse_name(name) else {
            unit.load_state = LoadState::BadSetting;
            unit.load_error = Some(format!("Invalid unit name \"{name}\""));
            return unit;
        };

        let mut lookup = find_fragment(self.cfg, name);
        if lookup.is_none() {
            if let Some(t) = template_of(name) {
                lookup = find_fragment(self.cfg, &t);
            }
        }
        let Some((path, masked)) = lookup else {
            return unit;
        };
        if masked {
            unit.load_state = LoadState::Masked;
            unit.fragment = Some(path);
            return unit;
        }

        // Alias resolution: if the file we found is a symlink to a unit file
        // with a different name, that name is canonical.
        let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if let Some(target_name) = real.file_name().and_then(|s| s.to_str()) {
            let looked_up = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if target_name != looked_up && parse_name(target_name).is_some() {
                let canonical = match (parse_name(target_name), parsed.instance) {
                    (Some(t), Some(inst)) if t.is_template => {
                        format!("{}@{}.{}", t.prefix, inst, t.suffix)
                    }
                    _ => target_name.to_string(),
                };
                unit.id = canonical.clone();
                unit.names = vec![canonical, name.to_string()];
            }
        }
        unit.fragment = Some(real.clone());

        if let Err(e) = unit.file.parse_file(&real) {
            unit.load_state = LoadState::BadSetting;
            unit.load_error = Some(format!("{}: {e}", real.display()));
            return unit;
        }
        unit.dropins = self.dropins_for(&unit);
        for d in unit.dropins.clone() {
            if let Err(e) = unit.file.parse_file(&d) {
                eprintln!("sdcompat: {}: {e}", d.display());
            }
        }
        unit.load_state = LoadState::Loaded;
        unit
    }

    fn dropins_for(&self, unit: &Unit) -> Vec<PathBuf> {
        let mut dirs: Vec<String> = Vec::new();
        let ty = unit_type(&unit.id);
        // Type-wide drop-ins (service.d), then dash prefixes (user-.slice.d
        // for user-1000.slice, shortest first), then template, then each name.
        dirs.push(format!("{ty}.d"));
        let stem = unit.id.split(['@', '.']).next().unwrap_or("");
        for (i, _) in stem.match_indices('-') {
            dirs.push(format!("{}-.{ty}.d", &stem[..i]));
        }
        if let Some(t) = template_of(&unit.id) {
            dirs.push(format!("{t}.d"));
        }
        for n in &unit.names {
            dirs.push(format!("{n}.d"));
        }

        // Same basename in a higher-priority directory wins.
        let mut by_name: BTreeMap<String, PathBuf> = BTreeMap::new();
        for d in &dirs {
            for base in self.cfg.unit_path.iter().rev() {
                let Ok(rd) = fs::read_dir(base.join(d)) else { continue };
                for e in rd.flatten() {
                    let fname = e.file_name().to_string_lossy().into_owned();
                    if fname.ends_with(".conf") {
                        by_name.insert(fname, e.path());
                    }
                }
            }
        }
        by_name
            .into_values()
            .filter(|p| !is_dev_null(p))
            .collect()
    }

    /// Every unit file name in the search path (templates included, aliases resolved later).
    pub fn list_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for dir in &self.cfg.unit_path {
            let Ok(rd) = fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if parse_name(&n).is_some() && !e.path().is_dir() {
                    out.insert(n);
                }
            }
        }
        out
    }

    /// Entries of `<unit>.wants/` (or `.requires/`, `.upholds/`) across the search path.
    pub fn dir_deps(&self, unit: &Unit, kind: &str) -> Vec<String> {
        let mut out = BTreeSet::new();
        let mut names = unit.names.clone();
        if let Some(t) = template_of(&unit.id) {
            names.push(t);
        }
        for n in &names {
            for dir in &self.cfg.unit_path {
                let Ok(rd) = fs::read_dir(dir.join(format!("{n}.{kind}"))) else { continue };
                for e in rd.flatten() {
                    let dep = e.file_name().to_string_lossy().into_owned();
                    if parse_name(&dep).is_some() && !is_dev_null(&e.path()) {
                        out.insert(dep);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Context needed to expand specifiers like %n, %i, %t.
pub fn expand_specifiers(s: &str, unit: &Unit) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let name = &unit.id;
    let parsed = parse_name(name);
    let stem = name.rsplit_once('.').map(|(a, _)| a).unwrap_or(name);
    let prefix = parsed.as_ref().map(|p| p.prefix).unwrap_or(stem);
    let instance = parsed.as_ref().and_then(|p| p.instance).unwrap_or("");
    let fragment = unit
        .fragment
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'n' => out.push_str(name),
            'N' => out.push_str(stem),
            'p' => out.push_str(prefix),
            'P' => out.push_str(&util::unit_name_unescape(prefix)),
            'i' => out.push_str(instance),
            'I' => out.push_str(&util::unit_name_unescape(instance)),
            'j' => out.push_str(prefix.rsplit('-').next().unwrap_or(prefix)),
            'J' => out.push_str(&util::unit_name_unescape(
                prefix.rsplit('-').next().unwrap_or(prefix),
            )),
            'f' => {
                let src = if instance.is_empty() { prefix } else { instance };
                let un = util::unit_name_unescape(src);
                if !un.starts_with('/') {
                    out.push('/');
                }
                out.push_str(&un);
            }
            't' => out.push_str("/run"),
            'S' => out.push_str("/var/lib"),
            'C' => out.push_str("/var/cache"),
            'L' => out.push_str("/var/log"),
            'E' => out.push_str("/etc"),
            'T' => out.push_str("/tmp"),
            'V' => out.push_str("/var/tmp"),
            'h' => out.push_str("/root"),
            's' => out.push_str("/bin/sh"),
            'u' | 'g' => out.push_str("root"),
            'U' | 'G' => out.push('0'),
            'H' => out.push_str(&util::hostname()),
            'l' => out.push_str(util::hostname().split('.').next().unwrap_or("")),
            'm' => out.push_str(&util::machine_id()),
            'b' => out.push_str(&util::boot_id()),
            'v' => out.push_str(
                &util::read_trimmed(Path::new("/proc/sys/kernel/osrelease")).unwrap_or_default(),
            ),
            'a' => out.push_str(systemd_arch()),
            'd' => out.push_str(&format!("/run/credentials/{name}")),
            'y' => out.push_str(&fragment),
            'Y' => out.push_str(
                &Path::new(&fragment)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            'o' | 'w' | 'B' | 'W' | 'A' | 'M' => {
                let key = match spec {
                    'o' => "ID",
                    'w' => "VERSION_ID",
                    'B' => "BUILD_ID",
                    'W' => "VARIANT_ID",
                    'A' => "IMAGE_VERSION",
                    _ => "IMAGE_ID",
                };
                let v = crate::envfile::load(Path::new("/etc/os-release"))
                    .ok()
                    .and_then(|vars| vars.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
                    .unwrap_or_default();
                out.push_str(&v);
            }
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

pub fn systemd_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86-64",
        "x86" => "x86",
        "aarch64" => "arm64",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        other => other,
    }
}

impl Unit {
    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        self.file.get(section, key).map(|v| expand_specifiers(v, self))
    }

    pub fn get_all(&self, section: &str, key: &str) -> Vec<String> {
        self.file
            .get_all(section, key)
            .into_iter()
            .map(|v| expand_specifiers(v, self))
            .collect()
    }

    pub fn get_words(&self, section: &str, key: &str) -> Vec<String> {
        self.get_all(section, key)
            .iter()
            .flat_map(|v| crate::unit::split_words(v).unwrap_or_default())
            .collect()
    }

    pub fn get_bool(&self, section: &str, key: &str, default: bool) -> bool {
        self.file.get_bool(section, key).unwrap_or(default)
    }

    pub fn description(&self) -> String {
        self.get("Unit", "Description").unwrap_or_else(|| self.id.clone())
    }

    pub fn utype(&self) -> &str {
        unit_type(&self.id)
    }

    /// Section holding type-specific settings ("Service", "Socket", ...).
    pub fn type_section(&self) -> String {
        let t = self.utype();
        let mut s = t[..1].to_ascii_uppercase();
        s.push_str(&t[1..]);
        s
    }

    pub fn is_loaded(&self) -> bool {
        self.load_state == LoadState::Loaded
    }

    /// TimeoutStartSec= (or TimeoutSec=) in usec; u64::MAX means none. As
    /// systemd: 0 means none, and a Type=oneshot service has none unless it
    /// sets one; otherwise DefaultTimeoutStartSec= (90 s).
    pub fn start_timeout_usec(&self) -> u64 {
        let set = self.get("Service", "TimeoutStartSec").or_else(|| self.get("Service", "TimeoutSec"));
        if let Some(u) = set.and_then(|v| crate::unit::parse_timeout_usec(&v)) {
            return u;
        }
        let oneshot = self.utype() == "service"
            && match self.get("Service", "Type") {
                Some(t) => t == "oneshot",
                None => self.file.get("Service", "ExecStart").is_none(),
            };
        if oneshot {
            u64::MAX
        } else {
            90_000_000
        }
    }

    /// TimeoutStopSec= (or TimeoutSec=) in usec, DefaultTimeoutStopSec= (90 s)
    /// if unset; u64::MAX means none.
    pub fn stop_timeout_usec(&self) -> u64 {
        self.get("Service", "TimeoutStopSec")
            .or_else(|| self.get("Service", "TimeoutSec"))
            .and_then(|v| crate::unit::parse_timeout_usec(&v))
            .unwrap_or(90_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_timeouts() {
        let unit = |text: &str| {
            let mut file = crate::unit::UnitFile::default();
            file.parse_str(text);
            Unit {
                id: "t.service".into(),
                names: vec!["t.service".into()],
                fragment: None,
                dropins: Vec::new(),
                load_state: LoadState::Loaded,
                load_error: None,
                file,
            }
        };
        assert_eq!(unit("[Service]\nExecStart=/bin/true\n").start_timeout_usec(), 90_000_000);
        // Type=oneshot: no start timeout unless set (fstrim, man-db-cache-update).
        assert_eq!(unit("[Service]\nType=oneshot\nExecStart=/bin/true\n").start_timeout_usec(), u64::MAX);
        assert_eq!(unit("[Service]\nType=oneshot\nTimeoutSec=5\n").start_timeout_usec(), 5_000_000);
        // 0 is "no timeout" (docker.service).
        assert_eq!(unit("[Service]\nType=notify\nTimeoutStartSec=0\n").start_timeout_usec(), u64::MAX);
    }

    #[test]
    fn names() {
        let n = parse_name("getty@tty1.service").unwrap();
        assert_eq!((n.prefix, n.instance, n.suffix), ("getty", Some("tty1"), "service"));
        assert!(parse_name("getty@.service").unwrap().is_template);
        assert!(parse_name("foo.bar").is_none());
        assert_eq!(template_of("a@b.service").as_deref(), Some("a@.service"));
    }
}
