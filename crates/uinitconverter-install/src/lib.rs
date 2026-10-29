//! [Install] handling: enable, disable, mask, presets, unit file state.
//! Produces the same (type, path, source) change records as systemd.


use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use uinitconverter_base::config::Config;
use uinitconverter_base::loader::{self, LoadState, Loader, Unit};

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// "symlink" or "unlink"
    pub kind: String,
    pub path: String,
    pub source: String,
}

fn change(kind: &str, path: &Path, source: &str) -> Change {
    Change { kind: kind.into(), path: path.display().to_string(), source: source.into() }
}

fn config_dirs(cfg: &Config) -> Vec<PathBuf> {
    let mut v = vec![cfg.config_unit_dir().to_path_buf()];
    if !v.contains(&cfg.runtime_unit_dir) {
        v.push(cfg.runtime_unit_dir.clone());
    }
    v
}

fn install_info(unit: &Unit) -> bool {
    ["WantedBy", "RequiredBy", "UpheldBy", "Alias", "Also"]
        .iter()
        .any(|k| !unit.get_words("Install", k).is_empty())
}

fn make_link(link: &Path, target: &Path, force: bool, changes: &mut Vec<Change>) -> Result<(), String> {
    if let Ok(existing) = fs::read_link(link) {
        if existing == target {
            return Ok(());
        }
        if !force {
            return Err(format!(
                "File {} already exists and is a symlink to {}.",
                link.display(),
                existing.display()
            ));
        }
        fs::remove_file(link).map_err(|e| e.to_string())?;
    } else if link.exists() {
        return Err(format!("File {} already exists.", link.display()));
    }
    if let Some(p) = link.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    symlink(target, link).map_err(|e| format!("{}: {e}", link.display()))?;
    changes.push(change("symlink", link, &target.display().to_string()));
    Ok(())
}

/// Name to enable for a unit: templates use DefaultInstance=.
fn install_name(unit: &Unit, requested: &str) -> Option<String> {
    let parsed = loader::parse_name(requested)?;
    if parsed.is_template {
        let inst = unit.get("Install", "DefaultInstance")?;
        Some(format!("{}@{}.{}", parsed.prefix, inst, parsed.suffix))
    } else {
        Some(requested.to_string())
    }
}

pub fn enable(
    cfg: &Config,
    names: &[String],
    runtime: bool,
    force: bool,
) -> Result<(bool, Vec<Change>), String> {
    let ld = Loader::new(cfg);
    let dir = if runtime { cfg.runtime_unit_dir.clone() } else { cfg.config_unit_dir().to_path_buf() };
    let mut changes = Vec::new();
    let mut carries_info = false;
    let mut todo: Vec<String> = names.iter().map(|n| basename(n)).collect();
    let mut done = BTreeSet::new();

    while let Some(name) = todo.pop() {
        if !done.insert(name.clone()) {
            continue;
        }
        let unit = ld.load(&name);
        match unit.load_state {
            LoadState::Masked => return Err(format!("Unit file {name} is masked.")),
            LoadState::NotFound => return Err(format!("Unit file {name} does not exist.")),
            LoadState::BadSetting => return Err(unit.load_error.unwrap_or_default()),
            LoadState::Loaded => {}
        }
        if !install_info(&unit) {
            continue;
        }
        carries_info = true;
        let fragment = unit.fragment.clone().unwrap();
        let Some(inst_name) = install_name(&unit, &name) else {
            // Template without DefaultInstance: only Also= is processed.
            todo.extend(unit.get_words("Install", "Also"));
            continue;
        };
        for (key, kind) in [("WantedBy", "wants"), ("RequiredBy", "requires"), ("UpheldBy", "upholds")] {
            for t in unit.get_words("Install", key) {
                let link = dir.join(format!("{t}.{kind}")).join(&inst_name);
                make_link(&link, &fragment, force, &mut changes)?;
            }
        }
        for alias in unit.get_words("Install", "Alias") {
            make_link(&dir.join(&alias), &fragment, force, &mut changes)?;
        }
        todo.extend(unit.get_words("Install", "Also"));
    }
    Ok((carries_info, changes))
}

fn basename(n: &str) -> String {
    Path::new(n)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| n.to_string())
}

/// Remove every enablement symlink for the given units (and their Also=).
pub fn disable(cfg: &Config, names: &[String], runtime: bool) -> Result<Vec<Change>, String> {
    let ld = Loader::new(cfg);
    let mut targets: BTreeSet<String> = BTreeSet::new();
    let mut todo: Vec<String> = names.iter().map(|n| basename(n)).collect();
    while let Some(n) = todo.pop() {
        if !targets.insert(n.clone()) {
            continue;
        }
        let unit = ld.load(&n);
        if unit.is_loaded() {
            targets.extend(unit.names.iter().cloned());
            if let Some(f) = &unit.fragment {
                targets.insert(basename(&f.display().to_string()));
            }
            todo.extend(unit.get_words("Install", "Also"));
            targets.extend(unit.get_words("Install", "Alias"));
        }
    }

    let dirs = if runtime { vec![cfg.runtime_unit_dir.clone()] } else { config_dirs(cfg) };
    let mut changes = Vec::new();
    for dir in dirs {
        remove_links(&dir, &targets, &mut changes, 0);
    }
    Ok(changes)
}

fn link_matches(link: &Path, targets: &BTreeSet<String>) -> bool {
    let name = basename(&link.display().to_string());
    let Ok(dest) = fs::read_link(link) else { return false };
    if dest == Path::new("/dev/null") {
        return false;
    }
    let dest_name = basename(&dest.display().to_string());
    let tmpl = loader::template_of(&name);
    targets.contains(&name)
        || targets.contains(&dest_name)
        || tmpl.map(|t| targets.contains(&t)).unwrap_or(false)
}

fn remove_links(dir: &Path, targets: &BTreeSet<String>, changes: &mut Vec<Change>, depth: u32) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            if link_matches(&p, targets) && fs::remove_file(&p).is_ok() {
                changes.push(change("unlink", &p, ""));
            }
        } else if meta.is_dir() && depth == 0 {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".wants") || n.ends_with(".requires") || n.ends_with(".upholds") {
                remove_links(&p, targets, changes, depth + 1);
                let _ = fs::remove_dir(&p);
            }
        }
    }
}

pub fn mask(cfg: &Config, names: &[String], runtime: bool, force: bool) -> Result<Vec<Change>, String> {
    let dir = if runtime { cfg.runtime_unit_dir.clone() } else { cfg.config_unit_dir().to_path_buf() };
    let mut changes = Vec::new();
    for n in names {
        let n = basename(n);
        if loader::parse_name(&n).is_none() {
            return Err(format!("Invalid unit name {n}"));
        }
        let link = dir.join(&n);
        if fs::symlink_metadata(&link).map(|m| m.is_file()).unwrap_or(false) && !force {
            return Err(format!("File {} already exists.", link.display()));
        }
        make_link(&link, Path::new("/dev/null"), true, &mut changes)?;
    }
    Ok(changes)
}

pub fn unmask(cfg: &Config, names: &[String], runtime: bool) -> Result<Vec<Change>, String> {
    let dirs = if runtime { vec![cfg.runtime_unit_dir.clone()] } else { config_dirs(cfg) };
    let mut changes = Vec::new();
    for n in names {
        let n = basename(n);
        for d in &dirs {
            let link = d.join(&n);
            if fs::read_link(&link).map(|t| t == Path::new("/dev/null")).unwrap_or(false)
                && fs::remove_file(&link).is_ok()
            {
                changes.push(change("unlink", &link, ""));
            }
        }
    }
    Ok(changes)
}

/// `aliases`: also count an Alias= link at the top of `dir` (for gdm.service,
/// display-manager.service -> gdm.service is all that enabling it creates).
/// Not in vendor directories, where such links are the package's own names.
fn any_link_to(dir: &Path, targets: &BTreeSet<String>, depth: u32, aliases: bool) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            // At the top, a link under the unit's own name is "linked", not
            // an enablement.
            let alias = aliases && depth == 0 && !targets.contains(&e.file_name().to_string_lossy().into_owned());
            if (depth > 0 || alias) && link_matches(&p, targets) {
                return true;
            }
        } else if meta.is_dir() && depth == 0 {
            let n = e.file_name().to_string_lossy().into_owned();
            if (n.ends_with(".wants") || n.ends_with(".requires") || n.ends_with(".upholds"))
                && any_link_to(&p, targets, 1, false)
            {
                return true;
            }
        }
    }
    false
}

/// systemd's UnitFileState: enabled, enabled-runtime, disabled, static,
/// masked, alias, indirect, linked, bad.
pub fn unit_file_state(cfg: &Config, name: &str) -> Result<String, String> {
    state_with(cfg, name, |dir, targets, aliases| any_link_to(dir, targets, 0, aliases))
}

/// unit_file_state() with `has_link(dir, names, aliases)` in place of
/// any_link_to(), so list_unit_files() can answer from one reading.
fn state_with(cfg: &Config, name: &str, has_link: impl Fn(&Path, &BTreeSet<String>, bool) -> bool) -> Result<String, String> {
    let any_link_to = |d: &Path, t: &BTreeSet<String>, _depth: u32, a: bool| has_link(d, t, a);
    let ld = Loader::new(cfg);
    let unit = ld.load(name);
    match unit.load_state {
        LoadState::Masked => {
            let rt = unit.fragment.as_ref().map(|f| f.starts_with(&cfg.runtime_unit_dir)).unwrap_or(false);
            return Ok(if rt { "masked-runtime" } else { "masked" }.into());
        }
        LoadState::NotFound => return Err(format!("No such file or directory: {name}")),
        LoadState::BadSetting => return Ok("bad".into()),
        LoadState::Loaded => {}
    }
    if unit.id != name && !name.contains("@.") {
        return Ok("alias".into());
    }
    let mut targets: BTreeSet<String> = unit.names.iter().cloned().collect();
    targets.insert(name.to_string());
    if any_link_to(cfg.config_unit_dir(), &targets, 0, true) {
        return Ok("enabled".into());
    }
    if any_link_to(&cfg.runtime_unit_dir, &targets, 0, true) {
        return Ok("enabled-runtime".into());
    }
    // Enabled from a vendor directory (e.g. /usr/lib/systemd/system/*.wants).
    for d in cfg.unit_path.iter().skip(1) {
        if any_link_to(d, &targets, 0, false) {
            return Ok("static".into());
        }
    }
    if !install_info(&unit) {
        return Ok("static".into());
    }
    let has_wanted = ["WantedBy", "RequiredBy", "UpheldBy", "Alias"]
        .iter()
        .any(|k| !unit.get_words("Install", k).is_empty());
    if !has_wanted || (loader::parse_name(name).map(|p| p.is_template).unwrap_or(false)
        && unit.get("Install", "DefaultInstance").is_none())
    {
        return Ok("indirect".into());
    }
    Ok("disabled".into())
}

/// The prescription the s6 set gives a service (always, active, usable,
/// masked), from the set's rx directories; None when the set doesn't list
/// it (no repository, a template, a unit uinitconverter doesn't convert).
pub fn set_rx(cfg: &Config, name: &str) -> Option<&'static str> {
    let set = cfg.repo_dir.join("sources").join(&cfg.set_name);
    ["always", "active", "usable", "masked"]
        .into_iter()
        .find(|rx| fs::symlink_metadata(set.join(rx).join(name)).is_ok())
}

/// A unit file `state` as the s6 set has it, for what systemctl shows.
/// After a service's first prescription the set decides what boots, not
/// the [Install] links (`systemctl disable` leaves the set alone), so for a
/// unit the set lists, enabled and disabled come from the set. static,
/// indirect, alias and the others are about the unit file and stay. A
/// service AUTO_MASK= masked is disabled.
pub fn set_file_state(cfg: &Config, name: &str, state: String) -> String {
    if !matches!(state.as_str(), "enabled" | "enabled-runtime" | "disabled") {
        return state;
    }
    match set_rx(cfg, name) {
        Some("always" | "active") if state == "disabled" => "enabled".into(),
        Some("usable") => "disabled".into(),
        // Masked by AUTO_MASK=: still started on demand, as a disabled unit.
        Some("masked") if uinitconverter_base::automask::is_masked(cfg, name) => "disabled".into(),
        Some("masked") => "masked".into(),
        _ => state,
    }
}

pub fn is_enabled(cfg: &Config, name: &str) -> bool {
    matches!(unit_file_state(cfg, name).as_deref(), Ok("enabled") | Ok("enabled-runtime") | Ok("static"))
}

// ---- presets ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresetAction {
    Enable,
    Disable,
    Ignore,
}

pub fn preset_for(cfg: &Config, name: &str) -> PresetAction {
    let mut files: std::collections::BTreeMap<String, PathBuf> = Default::default();
    for dir in cfg.preset_path.iter().rev() {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".preset") {
                    files.insert(n, e.path());
                }
            }
        }
    }
    for path in files.values() {
        let Ok(text) = fs::read_to_string(path) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let mut words = line.split_whitespace();
            let (Some(verb), Some(pattern)) = (words.next(), words.next()) else { continue };
            if uinitconverter_base::util::fnmatch(pattern, name) {
                return match verb {
                    "enable" => PresetAction::Enable,
                    "disable" => PresetAction::Disable,
                    _ => PresetAction::Ignore,
                };
            }
        }
    }
    PresetAction::Enable
}

/// mode: "full", "enable-only" or "disable-only"
///
/// Like systemd, all disables run before all enables: disabling a unit also
/// disables its Also= units, so `preset cups.path cups.socket cups.service`
/// (cups.service is preset-disabled with Also=cups.socket cups.path) must not
/// undo the enables of the other two.
pub fn preset(
    cfg: &Config,
    names: &[String],
    mode: &str,
    runtime: bool,
    force: bool,
) -> Result<(bool, Vec<Change>), String> {
    let (mut plus, mut minus) = (Vec::new(), Vec::new());
    for n in names {
        let n = basename(n);
        match preset_for(cfg, &n) {
            PresetAction::Enable if mode != "disable-only" => plus.push(n),
            PresetAction::Disable if mode != "enable-only" => minus.push(n),
            _ => {}
        }
    }
    let mut changes = Vec::new();
    if !minus.is_empty() {
        changes.extend(disable(cfg, &minus, runtime)?);
    }
    let mut info = false;
    for n in plus {
        let (i, c) = enable(cfg, &[n], runtime, force)?;
        info |= i;
        changes.extend(c);
    }
    Ok((info, changes))
}

pub fn preset_state(cfg: &Config, name: &str) -> &'static str {
    match preset_for(cfg, name) {
        PresetAction::Enable => "enabled",
        PresetAction::Disable => "disabled",
        PresetAction::Ignore => "ignored",
    }
}

/// All unit files with their state, for ListUnitFiles.
pub fn list_unit_files(cfg: &Config) -> Vec<(String, String)> {
    let links = Links::read(cfg);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in &cfg.unit_path {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if loader::parse_name(&n).is_none() || e.path().is_dir() || !seen.insert(n.clone()) {
                continue;
            }
            let state = state_with(cfg, &n, |d, t, a| links.any(d, t, a)).unwrap_or_else(|_| "bad".into());
            out.push((e.path().display().to_string(), state));
        }
    }
    out
}

/// The links of every unit directory (enablement and aliases), read once:
/// checking each unit file against all of them again made list-unit-files
/// O(files x links). Keyed by the link's name, its target's name and its
/// template, the three names link_matches() compares.
struct Links {
    by_name: std::collections::HashMap<String, Vec<(PathBuf, bool, String)>>,
}

impl Links {
    fn read(cfg: &Config) -> Links {
        let mut l = Links { by_name: Default::default() };
        let mut dirs = cfg.unit_path.clone();
        if !dirs.contains(&cfg.runtime_unit_dir) {
            dirs.push(cfg.runtime_unit_dir.clone());
        }
        for d in &dirs {
            l.scan(d, d, 0);
        }
        l
    }

    fn scan(&mut self, base: &Path, dir: &Path, depth: u32) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = fs::symlink_metadata(&p) else { continue };
            let n = e.file_name().to_string_lossy().into_owned();
            if meta.file_type().is_symlink() {
                let Ok(dest) = fs::read_link(&p) else { continue };
                if dest == Path::new("/dev/null") {
                    continue;
                }
                let mut keys = vec![n.clone(), basename(&dest.display().to_string())];
                keys.extend(loader::template_of(&n));
                keys.dedup();
                for k in keys {
                    self.by_name.entry(k).or_default().push((base.to_path_buf(), depth == 0, n.clone()));
                }
            } else if meta.is_dir() && depth == 0 && (n.ends_with(".wants") || n.ends_with(".requires") || n.ends_with(".upholds")) {
                self.scan(base, &p, 1);
            }
        }
    }

    /// any_link_to(dir, targets, 0, aliases) from the index.
    fn any(&self, dir: &Path, targets: &BTreeSet<String>, aliases: bool) -> bool {
        targets.iter().filter_map(|t| self.by_name.get(t)).flatten().any(|(d, top, name)| {
            d == dir && (!*top || (aliases && !targets.contains(name)))
        })
    }
}

pub fn get_default_target(cfg: &Config) -> Option<String> {
    let u = Loader::new(cfg).load("default.target");
    u.is_loaded().then_some(u.id)
}

pub fn set_default_target(cfg: &Config, name: &str) -> Result<Vec<Change>, String> {
    let unit = Loader::new(cfg).load(name);
    if !unit.is_loaded() || unit.utype() != "target" {
        return Err(format!("Unit {name} is not a loadable target."));
    }
    let mut changes = Vec::new();
    make_link(
        &cfg.config_unit_dir().join("default.target"),
        unit.fragment.as_ref().unwrap(),
        true,
        &mut changes,
    )?;
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_disable_before_enable() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-preset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (etc, lib, presets) = (tmp.join("etc"), tmp.join("lib"), tmp.join("preset"));
        for d in [&etc, &lib, &presets] {
            fs::create_dir_all(d).unwrap();
        }
        let write = |p: PathBuf, s: &str| fs::write(p, s).unwrap();
        write(lib.join("cups.service"), "[Service]\nExecStart=/bin/true\n[Install]\nAlso=cups.socket cups.path\nWantedBy=multi-user.target\n");
        write(lib.join("cups.socket"), "[Socket]\nListenStream=/run/cups.sock\n[Install]\nWantedBy=sockets.target\n");
        write(lib.join("cups.path"), "[Path]\nPathExists=/tmp\n[Install]\nWantedBy=multi-user.target\n");
        write(presets.join("90-default.preset"), "enable cups.socket\nenable cups.path\ndisable *\n");
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![etc.clone(), lib];
        cfg.runtime_unit_dir = tmp.join("run");
        cfg.preset_path = vec![presets];

        let names: Vec<String> = ["cups.path", "cups.socket", "cups.service"].iter().map(|s| s.to_string()).collect();
        preset(&cfg, &names, "full", false, false).unwrap();
        assert!(etc.join("sockets.target.wants/cups.socket").is_symlink());
        assert!(etc.join("multi-user.target.wants/cups.path").is_symlink());
        assert!(!etc.join("multi-user.target.wants/cups.service").exists());
        assert_eq!(unit_file_state(&cfg, "cups.service").as_deref(), Ok("disabled"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_state_follows_the_set() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-setstate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (etc, lib) = (tmp.join("etc"), tmp.join("lib"));
        let set = tmp.join("repo/sources/current");
        for d in [&etc, &lib, &set.join("active"), &set.join("usable"), &set.join("masked")] {
            fs::create_dir_all(d).unwrap();
        }
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![etc, lib];
        cfg.runtime_unit_dir = tmp.join("run");
        cfg.repo_dir = tmp.join("repo");
        symlink("x", set.join("active/a.service")).unwrap();
        symlink("x", set.join("usable/u.service")).unwrap();
        symlink("x", set.join("masked/m.service")).unwrap();
        let st = |n: &str, s: &str| set_file_state(&cfg, n, s.to_string());
        assert_eq!(st("a.service", "disabled"), "enabled");
        assert_eq!(st("a.service", "enabled-runtime"), "enabled-runtime");
        assert_eq!(st("u.service", "enabled"), "disabled");
        assert_eq!(st("m.service", "enabled"), "masked");
        // Not in the set, or not an enablement state: unchanged.
        assert_eq!(st("other.service", "enabled"), "enabled");
        assert_eq!(st("u.service", "static"), "static");
        assert_eq!(st("a.service", "indirect"), "indirect");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn enabled_through_alias() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-alias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (etc, lib) = (tmp.join("etc"), tmp.join("lib"));
        for d in [&etc, &lib] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lib.join("gdm.service"), "[Service]\nExecStart=/bin/true\n[Install]\nAlias=display-manager.service\n").unwrap();
        fs::write(lib.join("mc.service"), "[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target\n").unwrap();
        // A vendor alias (dnf5-makecache.service) does not enable anything.
        symlink("mc.service", lib.join("mc5.service")).unwrap();
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![etc.clone(), lib.clone()];
        cfg.runtime_unit_dir = tmp.join("run");
        assert_eq!(unit_file_state(&cfg, "gdm.service").as_deref(), Ok("disabled"));
        assert_eq!(unit_file_state(&cfg, "mc.service").as_deref(), Ok("disabled"));
        // `systemctl enable gdm.service` creates only the alias link.
        enable(&cfg, &["gdm.service".to_string()], false, false).unwrap();
        assert!(etc.join("display-manager.service").is_symlink());
        assert_eq!(unit_file_state(&cfg, "gdm.service").as_deref(), Ok("enabled"));
        assert_eq!(unit_file_state(&cfg, "display-manager.service").as_deref(), Ok("alias"));
        // list-unit-files answers from one reading of the links, the same.
        enable(&cfg, &["mc.service".to_string()], false, false).unwrap();
        let listed = list_unit_files(&cfg);
        assert!(listed.len() >= 4);
        for (path, state) in listed {
            assert_eq!(Ok(state), unit_file_state(&cfg, &basename(&path)), "{path}");
        }
        let _ = fs::remove_dir_all(&tmp);
    }
}
