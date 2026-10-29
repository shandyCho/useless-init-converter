//! Services the generator masked in the s6 set on its own (AUTO_MASK=yes):
//! translated units nothing boots or depends on, left out of the database
//! so they have no idle s6-supervise, and started by uinitconverterd as
//! dynamic instances when something asks for them. STATE_DIR/auto-masked
//! lists them; STATE_DIR/auto-masked.keep lists the ones the admin moved
//! out of `masked` afterwards, which are never masked again. A service
//! the admin masked is in neither and stays refused, as before.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::config::Config;

pub fn list_path(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("auto-masked")
}

pub fn keep_path(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("auto-masked.keep")
}

fn read(path: PathBuf) -> BTreeSet<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

fn write(path: PathBuf, names: &BTreeSet<String>) -> Result<(), String> {
    let mut text = String::new();
    for n in names {
        text.push_str(n);
        text.push('\n');
    }
    crate::util::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The services masked by the generator.
pub fn masked(cfg: &Config) -> BTreeSet<String> {
    read(list_path(cfg))
}

/// Whether the generator masked `name`. Asked once per unit (UnitFileState
/// of every unit file, each job): the list is read again only when the
/// file changes.
pub fn is_masked(cfg: &Config, name: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    // save() replaces the file (write_atomic): a new inode, whatever the
    // clock's granularity.
    type Cached = (PathBuf, Option<(SystemTime, u64, u64)>, BTreeSet<String>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
    let path = list_path(cfg);
    let key = fs::metadata(&path).ok().and_then(|m| Some((m.modified().ok()?, m.ino(), m.len())));
    let mut c = CACHE.lock().unwrap();
    if !matches!(&*c, Some((p, k, _)) if *p == path && *k == key && key.is_some()) {
        *c = Some((path.clone(), key, read(path)));
    }
    c.as_ref().is_some_and(|(_, _, set)| set.contains(name))
}

/// Whether `name` runs as uinitconverterd's dynamic instance: a longrun's
/// service directory (RUN_DIR/dyn/NAME) or an active oneshot's marker
/// (RUN_DIR/dyn-oneshot/NAME).
pub fn running_dynamic(cfg: &Config, name: &str) -> bool {
    ["dyn", "dyn-oneshot"].iter().any(|d| cfg.run_dir.join(d).join(name).exists())
}

/// The services the admin took back.
pub fn kept(cfg: &Config) -> BTreeSet<String> {
    read(keep_path(cfg))
}

pub fn save(cfg: &Config, masked: &BTreeSet<String>, kept: &BTreeSet<String>) -> Result<(), String> {
    write(list_path(cfg), masked)?;
    write(keep_path(cfg), kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("uinitconverter-automask-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::defaults();
        cfg.state_dir = dir.clone();
        assert!(masked(&cfg).is_empty() && kept(&cfg).is_empty());
        let m: BTreeSet<String> = ["a.service", "b.timer"].iter().map(|s| s.to_string()).collect();
        let k: BTreeSet<String> = ["c.service"].iter().map(|s| s.to_string()).collect();
        save(&cfg, &m, &k).unwrap();
        assert_eq!(masked(&cfg), m);
        assert_eq!(kept(&cfg), k);
        assert!(is_masked(&cfg, "a.service") && !is_masked(&cfg, "c.service"));
        let m2: BTreeSet<String> = ["c.service"].iter().map(|s| s.to_string()).collect();
        // A new file (write_atomic renames) is read again.
        save(&cfg, &m2, &k).unwrap();
        assert!(!is_masked(&cfg, "a.service") && is_masked(&cfg, "c.service"));
        let _ = fs::remove_dir_all(&dir);
    }
}
