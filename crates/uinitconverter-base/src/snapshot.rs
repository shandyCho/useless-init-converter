//! A unit as it was loaded at generation time (work_history parts 206-207).
//!
//! gen writes every unit it loaded into the s6-rc database it generates, so
//! the tools run the units of the database that is live, not whatever the
//! unit files on disk say now, and the units travel with their database (a
//! rollback boot, `s6 live install`, `s6-rc-update`):
//!
//! - a longrun carries its own unit in its service directory, `data/unit`
//!   (s6-rc-compile copies `data/` verbatim, s6-rc-update replaces it in a
//!   running service's directory without restarting it);
//! - every other unit gen loaded (oneshots, targets, the sockets a
//!   service's runner binds, `Accept=yes` templates) is in the holder
//!   longrun's `data/units/`, and the index in its `data/index`.
//!
//! This module is the one definition of the format: gen writes it, the
//! runner, the timers and the daemon read it.
//!
//! The file is line-based, fields separated by tabs, with `\\`, `\t` and
//! `\n` escaped in every field:
//!
//! ```text
//! uinitconverter-unit 1
//! id        NAME
//! name      NAME            (every name, the id first)
//! load      loaded|not-found|masked|bad-setting
//! error     TEXT            (a load error, when there is one)
//! fragment  PATH  SEC.NSEC  (the unit file and its mtime)
//! dropin    PATH  SEC.NSEC  (each drop-in, in load order)
//! dir       KIND  NAME      (an entry of NAME.KIND/, KIND in DIR_KINDS)
//! entry     SECTION  KEY  VALUE   (in the order they were read)
//! ```
//!
//! Values are as written in the files: specifiers are expanded when a tool
//! reads them, as with a unit loaded from disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::loader::{LoadState, Loader, Unit};
use crate::unit::UnitFile;

/// The first line, with the format's version.
pub const MAGIC: &str = "uinitconverter-unit 1";

/// A longrun's own unit, relative to its service directory.
pub const SERVICE_FILE: &str = "data/unit";

/// The longrun in the database that holds every other unit and the index.
/// It never needs to run: s6-rc gives every longrun of the live database a
/// service directory, up or not.
pub const HOLDER: &str = "uinitconverter-units";
/// The holder's units, one file per unit (named by util::path_component).
pub const HOLDER_UNITS: &str = "data/units";
/// The holder's copy of the database's index.
pub const HOLDER_INDEX: &str = "data/index";

/// The link directories (NAME.KIND/) a snapshot records.
pub const DIR_KINDS: &[&str] = &["wants", "requires", "upholds"];

/// A unit and what its loading read besides the unit files' entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub unit: Unit,
    /// KIND -> the names in NAME.KIND/ (Loader::dir_deps).
    pub dir_deps: BTreeMap<String, Vec<String>>,
    /// The fragment's and drop-ins' mtimes (seconds, nanoseconds), by
    /// path: what NeedDaemonReload compares with the files now.
    pub mtimes: BTreeMap<PathBuf, (i64, u32)>,
}

impl Snapshot {
    /// What `ld` would give for this unit now: its link directories and
    /// its files' mtimes are read here.
    pub fn take(ld: &Loader, unit: Unit) -> Snapshot {
        let dir_deps = DIR_KINDS
            .iter()
            .map(|k| (k.to_string(), ld.dir_deps(&unit, k)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let mtimes = unit
            .fragment
            .iter()
            .chain(unit.dropins.iter())
            .filter_map(|p| mtime(p).map(|t| (p.clone(), t)))
            .collect();
        Snapshot {
            unit,
            dir_deps,
            mtimes,
        }
    }

    /// The names in NAME.KIND/, as Loader::dir_deps gave them.
    pub fn dir_deps(&self, kind: &str) -> Vec<String> {
        self.dir_deps.get(kind).cloned().unwrap_or_default()
    }

    pub fn render(&self) -> String {
        let u = &self.unit;
        let mut out = String::new();
        let mut line = |fields: &[&str]| {
            let f: Vec<String> = fields.iter().map(|s| escape(s)).collect();
            out.push_str(&f.join("\t"));
            out.push('\n');
        };
        line(&[MAGIC]);
        line(&["id", &u.id]);
        for n in &u.names {
            line(&["name", n]);
        }
        line(&["load", u.load_state.as_str()]);
        if let Some(e) = &u.load_error {
            line(&["error", e]);
        }
        let stamp = |p: &Path| {
            self.mtimes
                .get(p)
                .map(|(s, n)| format!("{s}.{n:09}"))
                .unwrap_or_else(|| "-".into())
        };
        if let Some(f) = &u.fragment {
            line(&["fragment", &f.to_string_lossy(), &stamp(f)]);
        }
        for d in &u.dropins {
            line(&["dropin", &d.to_string_lossy(), &stamp(d)]);
        }
        for (kind, names) in &self.dir_deps {
            for n in names {
                line(&["dir", kind, n]);
            }
        }
        for (s, k, v) in &u.file.entries {
            line(&["entry", s, k, v]);
        }
        out
    }

    pub fn parse(text: &str) -> Result<Snapshot, String> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            return Err(format!("not a unit snapshot (no \"{MAGIC}\" line)"));
        }
        let mut s = Snapshot {
            unit: Unit {
                id: String::new(),
                names: Vec::new(),
                fragment: None,
                dropins: Vec::new(),
                load_state: LoadState::NotFound,
                load_error: None,
                file: UnitFile::default(),
            },
            dir_deps: BTreeMap::new(),
            mtimes: BTreeMap::new(),
        };
        for (n, l) in lines.enumerate() {
            let f: Vec<String> = l.split('\t').map(unescape).collect();
            let bad = || format!("line {}: {l:?}", n + 2);
            match f.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["id", v] => s.unit.id = v.to_string(),
                ["name", v] => s.unit.names.push(v.to_string()),
                ["load", v] => s.unit.load_state = LoadState::parse(v).ok_or_else(bad)?,
                ["error", v] => s.unit.load_error = Some(v.to_string()),
                ["fragment", p, t] => {
                    if let Some(t) = parse_stamp(t).ok_or_else(bad)? {
                        s.mtimes.insert(PathBuf::from(p), t);
                    }
                    s.unit.fragment = Some(PathBuf::from(p));
                }
                ["dropin", p, t] => {
                    if let Some(t) = parse_stamp(t).ok_or_else(bad)? {
                        s.mtimes.insert(PathBuf::from(p), t);
                    }
                    s.unit.dropins.push(PathBuf::from(p));
                }
                ["dir", k, v] => s
                    .dir_deps
                    .entry(k.to_string())
                    .or_default()
                    .push(v.to_string()),
                ["entry", sec, k, v] => {
                    s.unit
                        .file
                        .entries
                        .push((sec.to_string(), k.to_string(), v.to_string()))
                }
                _ => return Err(bad()),
            }
        }
        if s.unit.id.is_empty() {
            return Err("no id line".into());
        }
        Ok(s)
    }
}

/// "SEC.NSEC" or "-" (no mtime); None when it is neither.
fn parse_stamp(t: &str) -> Option<Option<(i64, u32)>> {
    if t == "-" {
        return Some(None);
    }
    let (sec, nsec) = t.split_once('.')?;
    Some(Some((sec.parse().ok()?, nsec.parse().ok()?)))
}

/// A file's mtime as (seconds, nanoseconds) since the epoch.
pub fn mtime(p: &Path) -> Option<(i64, u32)> {
    let d = std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?;
    Some((d.as_secs() as i64, d.subsec_nanos()))
}

/// The holder's file for unit `id`, relative to the holder's service
/// directory.
pub fn holder_file(id: &str) -> PathBuf {
    Path::new(HOLDER_UNITS).join(&*crate::util::path_component(id))
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some(c) => o.push(c),
            None => o.push('\\'),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("uinitconverter-snap-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Every unit in a unit path with a drop-in, an alias, link
    /// directories, a template instance, a masked and a missing unit:
    /// parse(render(take(load(u)))) is what was loaded.
    #[test]
    fn round_trip_equals_the_loaded_unit() {
        let d = tmp("rt");
        let units = d.join("units");
        fs::create_dir_all(units.join("a.service.d")).unwrap();
        fs::create_dir_all(units.join("a.service.wants")).unwrap();
        fs::create_dir_all(units.join("t@.service.requires")).unwrap();
        fs::write(
            units.join("a.service"),
            "[Unit]\nDescription=tab\there \\\n  and on\n[Service]\nExecStart=/bin/echo %n \\\\ x\nEnvironment=\"A=1\" B=2\n",
        )
        .unwrap();
        fs::write(
            units.join("a.service.d/10-x.conf"),
            "[Service]\nExecStart=\nExecStart=/bin/true\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("a.service", units.join("alias.service")).unwrap();
        fs::write(units.join("b.target"), "[Unit]\nDescription=B\n").unwrap();
        std::os::unix::fs::symlink("../b.target", units.join("a.service.wants/b.target")).unwrap();
        fs::write(
            units.join("t@.service"),
            "[Service]\nExecStart=/bin/echo %i\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("../b.target", units.join("t@.service.requires/b.target"))
            .unwrap();
        std::os::unix::fs::symlink("/dev/null", units.join("m.service")).unwrap();

        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units.clone()];
        let ld = Loader::new(&cfg);
        for name in [
            "a.service",
            "alias.service",
            "b.target",
            "t@x.service",
            "m.service",
            "gone.service",
        ] {
            let unit = ld.load(name);
            let snap = Snapshot::take(&ld, unit.clone());
            let back = Snapshot::parse(&snap.render()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back, snap, "{name}");
            assert_eq!(back.unit, unit, "{name}");
            for k in DIR_KINDS {
                assert_eq!(back.dir_deps(k), ld.dir_deps(&unit, k), "{name} {k}");
            }
        }
        let a = Snapshot::take(&ld, ld.load("a.service"));
        assert_eq!(a.dir_deps("wants"), vec!["b.target".to_string()]);
        assert_eq!(a.mtimes.len(), 2, "the fragment and the drop-in");
        let t = Snapshot::take(&ld, ld.load("t@x.service"));
        assert_eq!(t.dir_deps("requires"), vec!["b.target".to_string()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_what_is_not_a_snapshot() {
        assert!(Snapshot::parse("").is_err());
        assert!(Snapshot::parse("[Unit]\n").is_err());
        assert!(Snapshot::parse(&format!("{MAGIC}\nid\ta\nbogus\n")).is_err());
        assert!(Snapshot::parse(&format!("{MAGIC}\nload\tloaded\n")).is_err());
        assert!(Snapshot::parse(&format!("{MAGIC}\nid\ta\nfragment\t/x\t12\n")).is_err());
    }

    #[test]
    fn holder_file_stays_inside() {
        assert_eq!(holder_file("a.service"), Path::new("data/units/a.service"));
        assert_eq!(
            holder_file("../x.service").parent(),
            Some(Path::new(HOLDER_UNITS))
        );
    }
}
