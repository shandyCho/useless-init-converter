//! What a .path unit says: the unit it triggers and its path conditions.

use crate::loader::Unit;

pub const KEYS: &[&str] = &["PathExists", "PathExistsGlob", "PathChanged", "PathModified", "DirectoryNotEmpty"];

pub fn trigger_unit(unit: &Unit) -> String {
    unit.get("Path", "Unit")
        .unwrap_or_else(|| format!("{}.service", unit.id.trim_end_matches(".path")))
}

/// (kind, path) pairs.
pub fn specs(unit: &Unit) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for k in KEYS {
        for v in unit.get_all("Path", k) {
            out.push((k.to_string(), v));
        }
    }
    out
}
