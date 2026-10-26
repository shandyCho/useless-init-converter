//! The index of a generated s6-rc database: which s6-rc service each unit
//! became. sdcompat-gen writes it (STATE_DIR/current/index); the runner and
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

/// The oneshot sdcompat-gen adds beside a longrun service with ExecStop=.
/// It depends on the service, so s6-rc always takes it down first, and its
/// `down` runs the ExecStop=. No unit type ends in ".stop", so no unit can
/// have this name.
pub fn stop_atom(id: &str) -> String {
    format!("{id}.stop")
}

/// Argument form of a unit name that is safe for both sh and execline.
pub fn unit_arg(name: &str) -> String {
    if name.bytes().all(|b| b.is_ascii_alphanumeric() || b"@:._-".contains(&b)) {
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
    pub fn load(cfg: &Config) -> Index {
        let mut idx = Index::default();
        let Ok(text) = fs::read_to_string(cfg.state_dir.join("current").join("index")) else {
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
        idx
    }

    pub fn resolve<'a>(&'a self, name: &'a str) -> &'a str {
        self.aliases.get(name).map(String::as_str).unwrap_or(name)
    }
}
