//! What a .timer unit says: the unit it triggers, its OnCalendar= and
//! monotonic settings, and the names of the state files timer-run writes
//! (RUN_DIR/units/<timer>/...), which the daemon reads.

use crate::calendar::CalendarSpec;
use crate::loader::Unit;
use crate::unit::parse_timespan_usec;

pub const NEXT_REALTIME: &str = "next-realtime";
pub const NEXT_MONOTONIC: &str = "next-monotonic";
pub const LAST_TRIGGER: &str = "last-trigger";
pub const LAST_TRIGGER_MONO: &str = "last-trigger-mono";
pub const ELAPSED: &str = "elapsed";

/// Monotonic settings and the systemd property names they map to.
pub const MONOTONIC_KEYS: &[(&str, &str)] = &[
    ("OnActiveSec", "OnActiveUSec"),
    ("OnBootSec", "OnBootUSec"),
    ("OnStartupSec", "OnStartupUSec"),
    ("OnUnitActiveSec", "OnUnitActiveUSec"),
    ("OnUnitInactiveSec", "OnUnitInactiveUSec"),
];

pub fn trigger_unit(timer: &Unit) -> String {
    timer
        .get("Timer", "Unit")
        .unwrap_or_else(|| format!("{}.service", timer.id.trim_end_matches(".timer")))
}

pub fn calendars(timer: &Unit) -> Vec<(String, Option<CalendarSpec>)> {
    timer
        .get_all("Timer", "OnCalendar")
        .into_iter()
        .map(|v| {
            let spec = CalendarSpec::parse(&v).ok();
            (v, spec)
        })
        .collect()
}

pub fn monotonic(timer: &Unit) -> Vec<(&'static str, &'static str, u64)> {
    let mut out = Vec::new();
    for (key, prop) in MONOTONIC_KEYS {
        for v in timer.get_all("Timer", key) {
            if let Some(us) = parse_timespan_usec(&v, 1_000_000) {
                out.push((*key, *prop, us));
            }
        }
    }
    out
}
