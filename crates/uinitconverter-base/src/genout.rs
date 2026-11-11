//! What `initconvert-gen` prints, where another tool reads it: the result of
//! `reload` and the report of `plan`. initconvertd runs the tool for a
//! reload and for the reason a unit isn't managed, and reads these lines;
//! gen writes them with the functions here, so the two can't drift apart.
//!
//! `reload`:
//!
//! ```text
//! translated: 135 longrun, 90 oneshot, 74 bundle (32 services without warnings)
//! database: /var/lib/uinitconverter/db/STAMP (live state updated)
//! report:   /var/lib/uinitconverter/db/STAMP/report.txt
//! set current: CHANGE
//! ```
//!
//! `plan` (the report): the `translated:` line, then one `NAME [kind]` line
//! per translated unit (its details indented under it), then `skipped
//! units:` with one indented `NAME: REASON` line each.

use std::path::{Path, PathBuf};

const TRANSLATED: &str = "translated: ";
const DATABASE: &str = "database: ";
const LIVE: &str = " (live state updated)";
const REPORT: &str = "report:   ";
const SKIPPED_UNITS: &str = "skipped units:";

/// The report's first line.
pub fn translated_line(longruns: usize, oneshots: usize, bundles: usize, clean: usize) -> String {
    format!(
        "{TRANSLATED}{longruns} longrun, {oneshots} oneshot, {bundles} bundle ({clean} services without warnings)"
    )
}

/// A translated unit's line in the report.
pub fn unit_line(id: &str, kind: &str) -> String {
    format!("{id} [{kind}]")
}

/// The heading of the report's skipped units and one of its lines.
pub fn skipped_heading() -> &'static str {
    SKIPPED_UNITS
}

pub fn skipped_line(id: &str, why: &str) -> String {
    format!("    {id}: {why}")
}

/// What a reload did.
#[derive(Debug, PartialEq, Default)]
pub struct Reloaded {
    /// Translated units (longruns, oneshots and bundles).
    pub units: usize,
    pub db_dir: PathBuf,
    pub updated_live: bool,
    /// What the repository commands changed in the set on their own, and
    /// "not applied to the running system: ..." for a held-back update.
    pub set_changes: Vec<String>,
}

impl Reloaded {
    /// `first` is the report's first line (translated_line).
    pub fn render(&self, first: &str, set_name: &str) -> String {
        let mut s = format!(
            "{first}\n{DATABASE}{}{}\n{REPORT}{}\n",
            self.db_dir.display(),
            if self.updated_live { LIVE } else { "" },
            self.db_dir.join("report.txt").display()
        );
        for c in &self.set_changes {
            s.push_str(&format!("set {set_name}: {c}\n"));
        }
        s
    }

    /// None when the text has no `database:` line (the reload failed).
    pub fn parse(text: &str, set_name: &str) -> Option<Reloaded> {
        let mut r = Reloaded::default();
        let mut found = false;
        let set = format!("set {set_name}: ");
        for l in text.lines() {
            if let Some(t) = l.strip_prefix(TRANSLATED) {
                r.units = t
                    .split(" (")
                    .next()
                    .unwrap_or("")
                    .split(", ")
                    .filter_map(|p| p.split(' ').next()?.parse::<usize>().ok())
                    .sum();
            } else if let Some(d) = l.strip_prefix(DATABASE) {
                r.updated_live = d.ends_with(LIVE);
                r.db_dir = Path::new(d.strip_suffix(LIVE).unwrap_or(d)).to_path_buf();
                found = true;
            } else if let Some(c) = l.strip_prefix(&set) {
                r.set_changes.push(c.to_string());
            }
        }
        found.then_some(r)
    }
}

/// What the report says of one unit.
#[derive(Debug, PartialEq, Default)]
pub struct PlanEntry {
    pub translated: bool,
    /// gen's reason when it skips the unit.
    pub skipped: Option<String>,
}

pub fn plan_entry(report: &str, id: &str) -> PlanEntry {
    let mut e = PlanEntry::default();
    let unit = format!("{id} [");
    let skip = format!("    {id}: ");
    let mut in_skipped = false;
    for l in report.lines() {
        if l == SKIPPED_UNITS {
            in_skipped = true;
        } else if in_skipped {
            if let Some(why) = l.strip_prefix(&skip) {
                e.skipped = Some(why.to_string());
            }
        } else if l.starts_with(&unit) && l.ends_with(']') {
            e.translated = true;
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_output_round_trip() {
        let r = Reloaded {
            units: 299,
            db_dir: "/var/lib/uinitconverter/db/1-2".into(),
            updated_live: true,
            set_changes: vec![
                "a.service -> usable".into(),
                "not applied to the running system: the update would restart x.service".into(),
            ],
        };
        let text = r.render(&translated_line(135, 90, 74, 32), "current");
        assert_eq!(Reloaded::parse(&text, "current"), Some(r));
        let boot_only = Reloaded {
            units: 3,
            db_dir: "/d b".into(),
            ..Default::default()
        };
        let text = boot_only.render(&translated_line(1, 1, 1, 0), "s");
        assert_eq!(Reloaded::parse(&text, "s"), Some(boot_only));
        assert_eq!(
            Reloaded::parse("initconvert-gen: failed\n", "current"),
            None
        );
    }

    #[test]
    fn plan_entries() {
        let report = format!(
            "{}\nskipped: 1\n\n{}\n    depends: b.service\n{}\n\n{}\n{}\n",
            translated_line(1, 0, 1, 1),
            unit_line("a.service", "longrun"),
            unit_line("multi-user.target", "bundle"),
            skipped_heading(),
            skipped_line("x.service", "no ExecStart=")
        );
        assert_eq!(
            plan_entry(&report, "a.service"),
            PlanEntry {
                translated: true,
                skipped: None
            }
        );
        assert_eq!(
            plan_entry(&report, "x.service"),
            PlanEntry {
                translated: false,
                skipped: Some("no ExecStart=".into())
            }
        );
        assert_eq!(plan_entry(&report, "b.service"), PlanEntry::default());
    }
}
