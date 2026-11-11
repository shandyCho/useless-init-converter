//! What the runner asks of its supervisor, s6-supervise: whether the service
//! is still wanted up, the supervised process of a service, and how often
//! it died lately. (Readiness goes the other way, on the notification-fd
//! pipe: Runner::signal_ready.)

use crate::config::Config;
use crate::tools;

/// s6-svstat on the current directory (finish runs in the service dir):
/// false once the service was told to stay down (s6-svc -d).
pub fn wanted_up() -> bool {
    use uinitconverter_base::util::{svstatus, SvRead};
    match svstatus(std::path::Path::new(".")) {
        SvRead::Status(s) => return s.wanted_up,
        SvRead::NotSupervised => return true,
        SvRead::Unknown => {}
    }
    std::process::Command::new(tools::path("s6-svstat"))
        .args(["-o", "wantedup", "."])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() != "false")
        .unwrap_or(true)
}

/// How many times the service died in the `interval_usec` before its latest
/// death, that one included: s6-supervise's death tally of the current
/// directory, as `s6-svdt` prints it (finish runs in the service dir, after
/// s6-supervise has recorded the death). None when there is no tally.
pub fn recent_deaths(interval_usec: u64) -> Option<usize> {
    let out = std::process::Command::new(tools::path("s6-svdt"))
        .arg(".")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| count_recent(&String::from_utf8_lossy(&out.stdout), interval_usec))
}

/// s6-svdt's lines are `TAI64N signal NAME` or `TAI64N exitcode N`. The
/// stamps are compared with each other, so no clock is converted.
fn count_recent(tally: &str, interval_usec: u64) -> usize {
    let stamps: Vec<u64> = tally
        .lines()
        .filter_map(|l| tai64n_usec(l.split_whitespace().next()?))
        .collect();
    let Some(&last) = stamps.iter().max() else {
        return 0;
    };
    stamps.iter().filter(|&&t| last - t < interval_usec).count()
}

/// A TAI64N label (`@`, 16 hex digits of seconds, 8 of nanoseconds) in µs.
fn tai64n_usec(label: &str) -> Option<u64> {
    let hex = label.strip_prefix('@')?;
    if hex.len() != 24 || !hex.is_ascii() {
        return None;
    }
    let secs = u64::from_str_radix(&hex[..16], 16).ok()? & ((1 << 62) - 1);
    let nanos = u64::from_str_radix(&hex[16..], 16).ok()?;
    secs.checked_mul(1_000_000)?.checked_add(nanos / 1000)
}

/// Whether service ID is up (None: down or unknown) and, if so, the PID
/// s6-supervise runs for it (None when it gives none).
pub fn up_pid(cfg: &Config, id: &str) -> Option<Option<u32>> {
    use uinitconverter_base::util::{svstatus, SvRead};
    match svstatus(&cfg.live_dir.join("scandir").join(id)) {
        SvRead::Status(s) if s.up => return Some((s.pid > 0).then_some(s.pid)),
        SvRead::Status(_) | SvRead::NotSupervised => return None,
        SvRead::Unknown => {}
    }
    let out = std::process::Command::new(tools::path("s6-svstat"))
        .args(["-o", "up,pid"])
        .arg(cfg.live_dir.join("scandir").join(id))
        .output();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    match text.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["true", pid] => Some(pid.parse::<u32>().ok().filter(|&p| p > 0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deaths_in_the_interval_before_the_last_one() {
        // 0 s, 3 s, 9.5 s and 12 s after 4000000069136c00.
        let tally = "@4000000069136c0000000000 exitcode 1\n\
                     @4000000069136c0300000000 signal SIGSEGV\n\
                     @4000000069136c091dcd6500 exitcode 0\n\
                     @4000000069136c0c00000000 exitcode 1\n";
        assert_eq!(count_recent(tally, 10_000_000), 3);
        assert_eq!(count_recent(tally, 1_000_000), 1);
        assert_eq!(count_recent(tally, 13_000_000), 4);
        assert_eq!(count_recent("", 10_000_000), 0);
        assert_eq!(count_recent("garbage line\n", 10_000_000), 0);
        assert_eq!(tai64n_usec("@400000000000000100000000"), Some(1_000_000));
        assert_eq!(tai64n_usec("4000000000000001"), None);
    }
}
