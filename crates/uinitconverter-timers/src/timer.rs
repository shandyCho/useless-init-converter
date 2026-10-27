//! Timer units. Each .timer becomes an s6 longrun running `uinitconverter-timers
//! timer-run`, which computes the next elapse, sleeps, and asks the daemon to
//! start the triggered unit (so all job semantics stay in one place).
//!
//! State (read by the daemon for `systemctl list-timers` / status):
//!   RUN_DIR/units/<timer>/next-realtime, next-monotonic, last-trigger(-mono), elapsed
//! Persistent=yes stamps: STATE_DIR/timers/<timer>

use std::path::PathBuf;
use std::time::Duration;

use crate::config::Config;
use crate::loader::{Loader, Unit};
use crate::state::{self, UnitState};
use crate::util;
use uinitconverter_base::unit::parse_timespan_usec;
pub use uinitconverter_base::timerspec::*;

fn persistent_stamp(cfg: &Config, timer: &str) -> PathBuf {
    cfg.state_dir.join("timers").join(timer)
}

struct Runner {
    cfg: Config,
    timer: Unit,
    target: String,
    st: UnitState,
    target_st: UnitState,
    started_mono: u64,
    started_rt: u64,
    /// Monotonic settings that only fire once per timer activation.
    fired_once: Vec<(&'static str, u64)>,
    /// Random delay chosen for the current elapse (target, delay usec).
    delay: Option<(u64, u64)>,
}

impl Runner {
    /// Next elapse as (realtime usec, monotonic usec); 0 = none.
    fn next(&self) -> (u64, u64) {
        // Relative to the last trigger (or timer start), not "now": after
        // waking up exactly at an elapse, "now" would already be past it.
        let last = self.st.get_u64(LAST_TRIGGER);
        let base = (self.started_rt.max(last) / 1_000_000) as i64;
        let rt = calendars(&self.timer)
            .iter()
            .filter_map(|(_, spec)| spec.as_ref()?.next_after(base))
            .map(|s| s as u64 * 1_000_000)
            .min()
            .unwrap_or(0);

        // As systemd: relative to the unit's last (de)activation or this
        // timer's last trigger, whichever is later. Without the trigger, a
        // unit still running (or skipped by a condition) after the trigger
        // would leave the elapse in the past and re-trigger at once.
        let last_trigger = self.st.get_u64(LAST_TRIGGER_MONO);
        let unit_active = self.target_st.get_u64("inactive-exit-mono").max(last_trigger);
        let unit_inactive = self.target_st.get_u64("inactive-enter-mono").max(last_trigger);
        let mut mono = u64::MAX;
        for (key, _, us) in monotonic(&self.timer) {
            let at = match key {
                "OnBootSec" | "OnStartupSec" | "OnActiveSec" => {
                    if self.fired_once.contains(&(key, us)) {
                        continue;
                    }
                    let at = if key == "OnActiveSec" { self.started_mono + us } else { us };
                    // As systemd: a one-time elapse that had passed when this
                    // timer (re)started is done if the timer has triggered
                    // since boot (dnf-makecache's OnBootSec= on a restart).
                    // fired_once covers this run's own elapses.
                    if last_trigger > 0 && at < self.started_mono {
                        continue;
                    }
                    at
                }
                "OnUnitActiveSec" if unit_active > 0 => unit_active + us,
                "OnUnitInactiveSec" if unit_inactive > 0 => unit_inactive + us,
                _ => continue,
            };
            mono = mono.min(at);
        }
        (rt, if mono == u64::MAX { 0 } else { mono })
    }

    fn random_delay(&mut self, target: u64) -> u64 {
        let max = self
            .timer
            .get("Timer", "RandomizedDelaySec")
            .and_then(|v| parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(0);
        if max == 0 {
            return 0;
        }
        if let Some((t, d)) = self.delay {
            if t == target {
                return d;
            }
        }
        let seed = if self.timer.get_bool("Timer", "FixedRandomDelay", false) {
            // Stable per machine and timer.
            let mut h: u64 = 1469598103934665603;
            for b in format!("{}{}", util::machine_id(), self.timer.id).bytes() {
                h = (h ^ b as u64).wrapping_mul(1099511628211);
            }
            h
        } else {
            let mut buf = [0u8; 8];
            unsafe { libc::getrandom(buf.as_mut_ptr() as *mut libc::c_void, 8, 0) };
            u64::from_ne_bytes(buf)
        };
        let d = seed % max;
        self.delay = Some((target, d));
        d
    }

    /// Ask the daemon to start the unit. Returns false if the daemon could
    /// not be reached (the elapse is then retried).
    async fn trigger(&mut self, conn: &mut Option<zbus::Connection>) -> bool {
        if conn.is_none() {
            match crate::client::connect(&self.cfg).await {
                Ok(c) => *conn = Some(c),
                Err(e) => {
                    eprintln!("uinitconverter[{}]: cannot reach the daemon (will retry): {e}", self.timer.id);
                    return false;
                }
            }
        }
        let c = conn.as_ref().unwrap();
        match crate::client::manager_call(c, "StartUnit", &(self.target.as_str(), "replace")).await {
            Ok(_) => {}
            Err(e) if e.contains("org.freedesktop") => {
                // The daemon answered with an error (unit missing, masked, ...).
                eprintln!("uinitconverter[{}]: starting {} failed: {e}", self.timer.id, self.target);
            }
            Err(e) => {
                eprintln!("uinitconverter[{}]: lost connection to the daemon (will retry): {e}", self.timer.id);
                *conn = None;
                return false;
            }
        }
        self.st.set(LAST_TRIGGER, &util::now_realtime_usec().to_string());
        self.st.set(LAST_TRIGGER_MONO, &util::now_monotonic_usec().to_string());
        if self.timer.get_bool("Timer", "Persistent", false) {
            let _ = util::write_atomic(
                &persistent_stamp(&self.cfg, &self.timer.id),
                util::now_realtime_usec().to_string().as_bytes(),
            );
        }
        true
    }
}

pub fn cmd_timer_run(cfg: Config, name: &str) -> i32 {
    let ld = Loader::new(&cfg);
    let timer = ld.load(name);
    if !timer.is_loaded() {
        eprintln!("uinitconverter: timer {name}: {}", timer.load_state.as_str());
        return 1;
    }
    for (raw, spec) in calendars(&timer) {
        if spec.is_none() {
            eprintln!("uinitconverter[{name}]: cannot parse OnCalendar={raw}, ignored");
        }
    }
    // The unit's own name: its runner keeps its state under that, also when
    // Unit= names an alias (dnf-makecache.timer: Unit=dnf5-makecache.service).
    let target = ld.load(&trigger_unit(&timer)).id;
    let st = state::for_unit(&cfg.run_dir, &timer.id);
    st.ensure();
    st.clear(ELAPSED);
    st.stamp(crate::state::keys::ACTIVE_ENTER);
    let target_st = state::for_unit(&cfg.run_dir, &target);

    // Readiness for s6: the timer is "active (waiting)" right away.
    unsafe {
        if libc::fcntl(3, libc::F_GETFD) >= 0 {
            libc::write(3, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(3);
        }
    }

    let mut r = Runner {
        started_mono: util::now_monotonic_usec(),
        started_rt: util::now_realtime_usec(),
        cfg,
        timer,
        target,
        st,
        target_st,
        fired_once: Vec::new(),
        delay: None,
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("uinitconverter: {e}");
            return 1;
        }
    };
    rt.block_on(async move {
        let mut conn: Option<zbus::Connection> = None;

        // Persistent=yes: catch up on an elapse missed while the timer was down.
        if r.timer.get_bool("Timer", "Persistent", false) {
            if let Some(last) = util::read_trimmed(&persistent_stamp(&r.cfg, &r.timer.id))
                .and_then(|s| s.parse::<u64>().ok())
            {
                let missed = calendars(&r.timer).iter().any(|(_, s)| {
                    s.as_ref()
                        .and_then(|s| s.next_after((last / 1_000_000) as i64))
                        .map(|t| (t as u64) * 1_000_000 <= util::now_realtime_usec())
                        .unwrap_or(false)
                });
                while missed && !r.trigger(&mut conn).await {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        loop {
            let (next_rt, next_mono) = r.next();
            r.st.set(NEXT_REALTIME, &next_rt.to_string());
            r.st.set(NEXT_MONOTONIC, &next_mono.to_string());
            if next_rt == 0 && next_mono == 0 {
                // OnUnitActiveSec=/OnUnitInactiveSec= are armed once the unit
                // has run or the timer has triggered: keep waiting for that.
                if monotonic(&r.timer).iter().any(|(k, _, _)| matches!(*k, "OnUnitActiveSec" | "OnUnitInactiveSec")) {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
                // RemainAfterElapse=: stay active, nothing left to do.
                r.st.set(ELAPSED, "1");
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            }
            let now_rt = util::now_realtime_usec();
            let now_mono = util::now_monotonic_usec();
            let mut wait = u64::MAX;
            let mut due_mono = false;
            let mut due_rt = false;
            if next_rt > 0 {
                let target = next_rt + r.random_delay(next_rt);
                if target <= now_rt {
                    due_rt = true;
                } else {
                    wait = wait.min(target - now_rt);
                }
            }
            if next_mono > 0 {
                if next_mono <= now_mono {
                    due_mono = true;
                } else {
                    wait = wait.min(next_mono - now_mono);
                }
            }
            if due_rt || due_mono {
                if !r.trigger(&mut conn).await {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                if due_mono {
                    for (key, _, us) in monotonic(&r.timer) {
                        if matches!(key, "OnBootSec" | "OnStartupSec" | "OnActiveSec") {
                            let at = if key == "OnActiveSec" { r.started_mono + us } else { us };
                            if at <= now_mono {
                                r.fired_once.push((key, us));
                            }
                        }
                    }
                }
                r.delay = None;
                continue;
            }
            // Re-evaluate at least every 30 s (clock changes, unit activity).
            tokio::time::sleep(Duration::from_micros(wait.min(30_000_000))).await;
        }
    })
}
