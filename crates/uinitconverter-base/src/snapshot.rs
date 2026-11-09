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

use crate::config::Config;
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

    /// Whether the unit files now differ from what was loaded: systemd's
    /// NeedDaemonReload, for which `systemctl status` warns "changed on
    /// disk". A fragment or drop-in was changed or removed, or the unit now
    /// has another fragment, other drop-ins, or another load state.
    pub fn changed_on_disk(&self, ld: &Loader) -> bool {
        if !self.unit.is_loaded() {
            return false;
        }
        if self.mtimes.iter().any(|(p, t)| mtime(p) != Some(*t)) {
            return true;
        }
        let now = ld.load(&self.unit.id);
        now.load_state != self.unit.load_state
            || now.fragment != self.unit.fragment
            || now.dropins != self.unit.dropins
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

/// The live database's holder service directory, when the live database
/// has one (one generated before the holder existed has none).
pub fn live_holder(cfg: &Config) -> Option<PathBuf> {
    let h = cfg.live_dir.join("servicedirs").join(HOLDER);
    h.join(HOLDER_INDEX).is_file().then_some(h)
}

/// Where the daemon writes the snapshot of a unit it loads at run time
/// (a dynamic instance, a transient unit, a socket started by hand): in
/// the unit's state directory, RUN_DIR/units/ID/snapshot.
pub fn runtime_file(cfg: &Config, id: &str) -> PathBuf {
    cfg.unit_run_dir(id).join("snapshot")
}

/// The daemon's side of runtime_file().
pub fn write_runtime(cfg: &Config, snap: &Snapshot) -> std::io::Result<()> {
    let p = runtime_file(cfg, &snap.unit.id);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::util::write_atomic(&p, snap.render().as_bytes())
}

/// The units the tools run: the live database's (each longrun's
/// `data/unit`, the holder's `data/units/`), then those the daemon loaded
/// at run time (runtime_file). A unit in none of them is not found: the
/// unit files on disk are not read behind the database's back. Without a
/// live database with a holder (no s6 boot, `initconvert plan`, or a
/// database generated before the holder existed) the unit files are read,
/// as before.
pub struct Units<'a> {
    cfg: &'a Config,
    holder: Option<PathBuf>,
    index: crate::index::Index,
}

impl<'a> Units<'a> {
    pub fn new(cfg: &'a Config) -> Units<'a> {
        Units {
            cfg,
            holder: live_holder(cfg),
            index: crate::index::Index::load(cfg),
        }
    }

    /// Whether the units come from a live database (else from disk).
    pub fn from_database(&self) -> bool {
        self.holder.is_some()
    }

    pub fn index(&self) -> &crate::index::Index {
        &self.index
    }

    /// A unit's canonical id: through the index's aliases, or as loaded
    /// from disk.
    pub fn id(&self, name: &str) -> String {
        if self.holder.is_some() {
            self.index.resolve(name).to_string()
        } else {
            Loader::new(self.cfg).load(name).id
        }
    }

    /// A unit and its link directories' entries.
    pub fn get(&self, name: &str) -> Snapshot {
        let Some(holder) = &self.holder else {
            let ld = Loader::new(self.cfg);
            return Snapshot::take(&ld, ld.load(name));
        };
        let id = self.index.resolve(name).to_string();
        if let Some(s) = find_in(self.cfg, holder, &id) {
            return s;
        }
        Snapshot {
            unit: Unit {
                id: id.clone(),
                names: vec![id],
                fragment: None,
                dropins: Vec::new(),
                load_state: LoadState::NotFound,
                load_error: Some("not in the live database (run `systemctl daemon-reload`)".into()),
                file: UnitFile::default(),
            },
            dir_deps: BTreeMap::new(),
            mtimes: BTreeMap::new(),
        }
    }

    /// The socket units whose Service= names one of `names`: the
    /// database's, and those the daemon loaded at run time (a socket
    /// started by hand). From disk: every socket in the unit path.
    pub fn sockets_naming(&self, names: &[String]) -> Vec<Snapshot> {
        let names_it = |s: &Snapshot| {
            s.unit
                .get("Socket", "Service")
                .is_some_and(|v| names.contains(&v))
        };
        let Some(holder) = &self.holder else {
            let ld = Loader::new(self.cfg);
            let mut out = Vec::new();
            for dir in &self.cfg.unit_path {
                let Ok(rd) = std::fs::read_dir(dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if !n.ends_with(".socket") {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(e.path()) else {
                        continue;
                    };
                    if !text.contains("Service=") {
                        continue;
                    }
                    let s = Snapshot::take(&ld, ld.load(&n));
                    if names_it(&s) && !out.iter().any(|o: &Snapshot| o.unit.id == s.unit.id) {
                        out.push(s);
                    }
                }
            }
            return out;
        };
        let mut ids: Vec<String> = Vec::new();
        for dir in [holder.join(HOLDER_UNITS), self.cfg.run_dir.join("units")] {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".socket") && !ids.contains(&n) {
                    ids.push(n);
                }
            }
        }
        ids.iter()
            .map(|n| self.get(n))
            .filter(|s| s.unit.is_loaded() && names_it(s))
            .collect()
    }
}

/// Unit `id` (canonical) in the live database or the daemon's runtime
/// files, None when the live database has no holder or doesn't carry it.
/// The daemon's lookup; the tools go through Units.
pub fn find_live(cfg: &Config, id: &str) -> Option<Snapshot> {
    find_in(cfg, &live_holder(cfg)?, id)
}

fn find_in(cfg: &Config, holder: &Path, id: &str) -> Option<Snapshot> {
    if let Some(s) = find_file(cfg, holder, id) {
        return Some(s);
    }
    // An instance of a template the database carries (an Accept=yes
    // connection's echo@N-PID-UID.service): the template's unit under the
    // instance's name, as the loader would make it.
    let t = crate::loader::template_of(id)?;
    let mut s = find_file(cfg, holder, &t).filter(|s| s.unit.is_loaded())?;
    s.unit.id = id.to_string();
    s.unit.names = vec![id.to_string()];
    Some(s)
}

fn find_file(cfg: &Config, holder: &Path, id: &str) -> Option<Snapshot> {
    let comp = crate::util::path_component(id);
    let live = cfg.live_dir.join("servicedirs");
    [
        live.join(&*comp).join(SERVICE_FILE),
        holder.join(holder_file(id)),
        runtime_file(cfg, id),
    ]
    .into_iter()
    .find_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        Some(Snapshot::parse(&text).unwrap_or_else(|e| {
            let mut s = Snapshot::parse(&format!("{MAGIC}\nid\t{}\n", escape(id))).unwrap();
            s.unit.load_state = LoadState::BadSetting;
            s.unit.load_error = Some(format!("{}: {e}", p.display()));
            s
        }))
    })
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

    /// With a live database: a longrun's own data/unit, the holder's
    /// units, the daemon's runtime file, an instance of a template the
    /// holder carries; anything else is not found, even with a unit file on
    /// disk. The index is the live database's, not STATE_DIR/current's.
    #[test]
    fn units_come_from_the_live_database() {
        let d = tmp("units");
        let units = d.join("units");
        fs::create_dir_all(&units).unwrap();
        for n in [
            "a.service",
            "b.target",
            "c.service",
            "t@.service",
            "disk.service",
        ] {
            fs::write(units.join(n), format!("[Unit]\nDescription={n}\n")).unwrap();
        }
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units.clone()];
        cfg.live_dir = d.join("live");
        cfg.run_dir = d.join("run");
        cfg.state_dir = d.join("state");
        cfg.repo_dir = d.join("repo");
        let ld = Loader::new(&cfg);
        let snap = |n: &str| Snapshot::take(&ld, ld.load(n)).render();
        let sd = cfg.live_dir.join("servicedirs");
        fs::create_dir_all(sd.join("a.service/data")).unwrap();
        fs::write(sd.join("a.service").join(SERVICE_FILE), snap("a.service")).unwrap();
        let h = sd.join(HOLDER);
        fs::create_dir_all(h.join(HOLDER_UNITS)).unwrap();
        fs::write(h.join(holder_file("b.target")), snap("b.target")).unwrap();
        fs::write(h.join(holder_file("t@.service")), snap("t@.service")).unwrap();
        fs::write(
            h.join(HOLDER_INDEX),
            "unit\ta.service\tlongrun\t-\nalias\tal.service\ta.service\n",
        )
        .unwrap();
        fs::create_dir_all(cfg.state_dir.join("current")).unwrap();
        fs::write(
            cfg.state_dir.join("current/index"),
            "unit\tnewer.service\tlongrun\t-\n",
        )
        .unwrap();
        write_runtime(&cfg, &Snapshot::take(&ld, ld.load("c.service"))).unwrap();

        let u = Units::new(&cfg);
        assert!(u.from_database());
        assert!(u.index().units.contains_key("a.service"));
        assert!(!u.index().units.contains_key("newer.service"));
        assert_eq!(u.get("a.service").unit, ld.load("a.service"));
        assert_eq!(u.get("al.service").unit.id, "a.service");
        assert_eq!(u.get("b.target").unit, ld.load("b.target"));
        assert_eq!(u.get("c.service").unit, ld.load("c.service"));
        let i = u.get("t@x.service").unit;
        assert!(i.is_loaded());
        assert_eq!(i.id, "t@x.service");
        assert_eq!(i.get("Unit", "Description").as_deref(), Some("t@.service"));
        let gone = u.get("disk.service").unit;
        assert_eq!(gone.load_state, LoadState::NotFound);
        assert!(gone.load_error.unwrap().contains("daemon-reload"));

        // No holder (no live database, or one older than the holder):
        // the unit files and STATE_DIR/current's index.
        fs::remove_dir_all(&h).unwrap();
        let u = Units::new(&cfg);
        assert!(!u.from_database());
        assert!(u.index().units.contains_key("newer.service"));
        assert!(u.get("disk.service").unit.is_loaded());
        let _ = fs::remove_dir_all(&d);
    }

    /// changed_on_disk: nothing changed; the fragment edited; a drop-in
    /// added; the unit masked.
    #[test]
    fn changed_on_disk_as_need_daemon_reload() {
        let d = tmp("ndr");
        let units = d.join("units");
        fs::create_dir_all(&units).unwrap();
        let f = units.join("a.service");
        fs::write(&f, "[Service]\nExecStart=/bin/true\n").unwrap();
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units.clone()];
        let ld = Loader::new(&cfg);
        let take = || Snapshot::take(&ld, ld.load("a.service"));
        let s = take();
        assert!(!s.changed_on_disk(&ld));
        // A different mtime (a second later than written).
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(s.changed_on_disk(&ld));
        let s = take();
        assert!(!s.changed_on_disk(&ld));
        fs::create_dir_all(units.join("a.service.d")).unwrap();
        fs::write(units.join("a.service.d/x.conf"), "[Service]\nNice=1\n").unwrap();
        assert!(s.changed_on_disk(&ld));
        let s = take();
        assert!(!s.changed_on_disk(&ld));
        fs::remove_file(&f).unwrap();
        std::os::unix::fs::symlink("/dev/null", &f).unwrap();
        assert!(s.changed_on_disk(&ld));
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
