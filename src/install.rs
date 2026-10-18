//! [Install] handling: enable, disable, mask, presets, unit file state.
//! Produces the same (type, path, source) change records as systemd.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::loader::{self, LoadState, Loader, Unit};

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

fn any_link_to(dir: &Path, targets: &BTreeSet<String>, depth: u32) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            if depth > 0 && link_matches(&p, targets) {
                return true;
            }
        } else if meta.is_dir() && depth == 0 {
            let n = e.file_name().to_string_lossy().into_owned();
            if (n.ends_with(".wants") || n.ends_with(".requires") || n.ends_with(".upholds"))
                && any_link_to(&p, targets, 1)
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
    if any_link_to(cfg.config_unit_dir(), &targets, 0) {
        return Ok("enabled".into());
    }
    if any_link_to(&cfg.runtime_unit_dir, &targets, 0) {
        return Ok("enabled-runtime".into());
    }
    // Enabled from a vendor directory (e.g. /usr/lib/systemd/system/*.wants).
    for d in cfg.unit_path.iter().skip(1) {
        if any_link_to(d, &targets, 0) {
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
            if crate::util::fnmatch(pattern, name) {
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
pub fn preset(
    cfg: &Config,
    names: &[String],
    mode: &str,
    runtime: bool,
    force: bool,
) -> Result<(bool, Vec<Change>), String> {
    let mut changes = Vec::new();
    let mut info = false;
    for n in names {
        let n = basename(n);
        match preset_for(cfg, &n) {
            PresetAction::Enable if mode != "disable-only" => {
                let (i, c) = enable(cfg, &[n], runtime, force)?;
                info |= i;
                changes.extend(c);
            }
            PresetAction::Disable if mode != "enable-only" => {
                changes.extend(disable(cfg, &[n], runtime)?);
            }
            _ => {}
        }
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
    let ld = Loader::new(cfg);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in &cfg.unit_path {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if loader::parse_name(&n).is_none() || e.path().is_dir() || !seen.insert(n.clone()) {
                continue;
            }
            let state = unit_file_state(cfg, &n).unwrap_or_else(|_| "bad".into());
            out.push((e.path().display().to_string(), state));
        }
    }
    let _ = ld;
    out
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
