//! The index of a generated s6-rc database: which s6-rc service each unit
//! became. initconvert-gen writes it (STATE_DIR/current/index); the runner and
//! the daemon read it.

use std::collections::BTreeMap;
use std::fs;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Longrun,
    Oneshot,
    Bundle,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Longrun => "longrun",
            Kind::Oneshot => "oneshot",
            Kind::Bundle => "bundle",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "longrun" => Some(Kind::Longrun),
            "oneshot" => Some(Kind::Oneshot),
            "bundle" => Some(Kind::Bundle),
            _ => None,
        }
    }
}

/// The oneshot initconvert-gen adds beside a longrun service with ExecStop=.
/// It depends on the service, so s6-rc always takes it down first, and its
/// `down` runs the ExecStop=. No unit type ends in ".stop", so no unit can
/// have this name.
pub fn stop_atom(id: &str) -> String {
    format!("{id}.stop")
}

/// Argument form of a unit name that is safe for both sh and execline.
pub fn unit_arg(name: &str) -> String {
    if name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"@:._-".contains(&b))
    {
        format!("-- {name}")
    } else {
        let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
        format!("--unit-hex {hex}")
    }
}

/// The index written next to a generated source tree.
#[derive(Debug, Default, Clone)]
pub struct Index {
    /// unit id -> (kind, log atom)
    pub units: BTreeMap<String, (Kind, Option<String>)>,
    pub aliases: BTreeMap<String, String>,
    pub default_target: Option<String>,
    /// timer -> triggered unit
    pub triggers: BTreeMap<String, String>,
}

impl Index {
    /// The index of the live database (its holder, snapshot.rs), so a
    /// rollback boot (uinitconverter.db=previous) reads the index of the
    /// database it booted; without a live database (or one generated
    /// before the holder existed), the newest generated one,
    /// STATE_DIR/current/index.
    pub fn load(cfg: &Config) -> Index {
        let mut idx = Index::default();
        let path = crate::snapshot::live_holder(cfg)
            .map(|h| h.join(crate::snapshot::HOLDER_INDEX))
            .unwrap_or_else(|| cfg.state_dir.join("current").join("index"));
        let Ok(text) = fs::read_to_string(path) else {
            return idx;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["unit", id, kind, log] => {
                    if let Some(k) = Kind::parse(kind) {
                        let log = (*log != "-").then(|| log.to_string());
                        idx.units.insert(id.to_string(), (k, log));
                    }
                }
                ["alias", a, id] => {
                    idx.aliases.insert(a.to_string(), id.to_string());
                }
                ["default", d] => idx.default_target = Some(d.to_string()),
                ["trigger", t, u] => {
                    idx.triggers.insert(t.to_string(), u.to_string());
                }
                _ => {}
            }
        }
        // Masked in the s6 set (by the admin, or left by AUTO_MASK=): not in the
        // compiled database, whatever the generated store has.
        if let Ok(rd) = fs::read_dir(
            cfg.repo_dir
                .join("sources")
                .join(&cfg.set_name)
                .join("masked"),
        ) {
            for e in rd.flatten() {
                idx.units.remove(e.file_name().to_string_lossy().as_ref());
            }
        }
        idx
    }

    /// The file load() reads, as initconvert-gen writes it: the format has
    /// this one writer and that one reader (the round-trip test below).
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (id, (kind, log)) in &self.units {
            out.push_str(&format!(
                "unit\t{id}\t{}\t{}\n",
                kind.as_str(),
                log.as_deref().unwrap_or("-")
            ));
        }
        for (a, id) in &self.aliases {
            out.push_str(&format!("alias\t{a}\t{id}\n"));
        }
        for (t, u) in &self.triggers {
            out.push_str(&format!("trigger\t{t}\t{u}\n"));
        }
        if let Some(d) = &self.default_target {
            out.push_str(&format!("default\t{d}\n"));
        }
        out
    }

    pub fn resolve<'a>(&'a self, name: &'a str) -> &'a str {
        self.aliases.get(name).map(String::as_str).unwrap_or(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What gen renders, the runner and the daemon load back unchanged
    /// (Index::render is gen's writer; load is their reader).
    #[test]
    fn render_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("uinitconverter-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut cfg = Config::defaults();
        cfg.state_dir = dir.join("state");
        cfg.repo_dir = dir.join("repo");
        // Not the host's /run/s6-rc: on an s6 boot load() reads the live
        // database's holder first.
        cfg.live_dir = dir.join("live");
        let mut idx = Index::default();
        idx.units.insert(
            "a.service".into(),
            (Kind::Longrun, Some("a.service-log".into())),
        );
        idx.units.insert("b.service".into(), (Kind::Oneshot, None));
        idx.units.insert("x.target".into(), (Kind::Bundle, None));
        idx.units
            .insert("gone.service".into(), (Kind::Longrun, None));
        idx.aliases
            .insert("dbus.service".into(), "dbus-broker.service".into());
        idx.triggers.insert("t.timer".into(), "b.service".into());
        idx.default_target = Some("x.target".into());
        fs::create_dir_all(cfg.state_dir.join("current")).unwrap();
        fs::write(cfg.state_dir.join("current/index"), idx.render()).unwrap();
        // Masked in the s6 set: load leaves it out.
        let masked = cfg
            .repo_dir
            .join("sources")
            .join(&cfg.set_name)
            .join("masked");
        fs::create_dir_all(&masked).unwrap();
        fs::write(masked.join("gone.service"), "").unwrap();

        let back = Index::load(&cfg);
        idx.units.remove("gone.service");
        assert_eq!(back.units, idx.units);
        assert_eq!(back.aliases, idx.aliases);
        assert_eq!(back.triggers, idx.triggers);
        assert_eq!(back.default_target, idx.default_target);
        assert_eq!(back.resolve("dbus.service"), "dbus-broker.service");
        let _ = fs::remove_dir_all(&dir);
    }
}
