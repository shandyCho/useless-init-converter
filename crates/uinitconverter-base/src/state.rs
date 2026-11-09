//! Per-unit runtime state written by the runner and read by the daemon.
//! Lives in RUN_DIR/units/<unit>/, one small file per value.

use std::fs;
use std::path::{Path, PathBuf};

use crate::util;

pub struct UnitState {
    pub dir: PathBuf,
}

impl UnitState {
    pub fn new(dir: PathBuf) -> UnitState {
        UnitState { dir }
    }

    pub fn ensure(&self) {
        let _ = fs::create_dir_all(&self.dir);
    }

    pub fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    pub fn set(&self, key: &str, val: &str) {
        self.ensure();
        let _ = util::write_atomic(&self.path(key), val.as_bytes());
    }

    pub fn get(&self, key: &str) -> Option<String> {
        util::read_trimmed(&self.path(key))
    }

    pub fn get_u64(&self, key: &str) -> u64 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    pub fn get_i64(&self, key: &str) -> i64 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    pub fn clear(&self, key: &str) {
        let _ = fs::remove_file(self.path(key));
    }

    /// Remove `key`; true if this call removed it (the other side did not).
    pub fn take(&self, key: &str) -> bool {
        fs::remove_file(self.path(key)).is_ok()
    }

    pub fn has(&self, key: &str) -> bool {
        self.path(key).exists()
    }

    /// Record "<key>" as realtime and "<key>-mono" as monotonic usec.
    pub fn stamp(&self, key: &str) {
        self.set(key, &util::now_realtime_usec().to_string());
        self.set(
            &format!("{key}-mono"),
            &util::now_monotonic_usec().to_string(),
        );
    }
}

/// Keys used across runner and daemon.
pub mod keys {
    /// Result: success, exit-code, signal, timeout, start-limit-hit, resources, condition
    pub const RESULT: &str = "result";
    pub const CONDITION_FAILED: &str = "condition-failed";
    pub const ASSERT_FAILED: &str = "assert-failed";
    pub const STATUS_TEXT: &str = "status";
    pub const MAIN_PID: &str = "mainpid";
    pub const EXEC_MAIN_START: &str = "exec-main-start";
    pub const EXEC_MAIN_EXIT: &str = "exec-main-exit";
    /// 1 = exited, 2 = killed, 3 = dumped (CLD_* codes)
    pub const EXEC_MAIN_CODE: &str = "exec-main-code";
    pub const EXEC_MAIN_STATUS: &str = "exec-main-status";
    pub const ACTIVE_ENTER: &str = "active-enter";
    pub const ACTIVE_EXIT: &str = "active-exit";
    pub const INACTIVE_ENTER: &str = "inactive-enter";
    pub const INACTIVE_EXIT: &str = "inactive-exit";
    pub const NRESTARTS: &str = "nrestarts";
    pub const RESTART_TIMES: &str = "restart-times";
    pub const INVOCATION: &str = "invocation";
    pub const CGROUP: &str = "cgroup";
    pub const NOTIFY_SOCKET: &str = "notify";
    /// Count of RELOADING=1 ... READY=1 cycles seen by the notify helper
    /// (Type=notify-reload); a reload waits for it to go up.
    /// STOPPING=1 from a Type=notify service: the daemon stops the unit
    /// (and what is bound to it) as systemd does; set by the notify helper.
    pub const STOPPING: &str = "stopping-requested";
    pub const RELOADS_DONE: &str = "reloads-done";
    /// The start failed (s6-rc was still told "ready" so wanted-by units go on).
    pub const START_FAILED: &str = "start-failed";
    /// EXTEND_TIMEOUT_USEC= from a starting notify service: the start
    /// timeout ends no earlier than this (monotonic usec).
    pub const EXTEND_UNTIL: &str = "extend-until";
    /// Left by ResetFailed in place of START_FAILED: the run process still
    /// waits as "up" for s6, but the unit is inactive, not failed.
    pub const PARKED: &str = "parked";
    /// Not started because a required unit (Requires=/BindsTo=/Requisite=) failed.
    pub const DEPENDENCY_FAILED: &str = "dependency-failed";
    /// Set by the daemon before `s6-svc -r`, so finish restarts regardless of Restart=.
    pub const RESTART_REQUESTED: &str = "restart-requested";
    /// The service exited and its runner waits for activity on its sockets
    /// before starting it again (systemd's socket activation, Accept=no).
    pub const SOCKET_WAIT: &str = "socket-wait";
    /// The run script started the service since it was last stopped: an
    /// on-demand service restarted by s6 (Restart=, daemon-reload) starts
    /// at once instead of waiting for its sockets again.
    pub const ACTIVATED: &str = "activated";
    /// Set by the daemon before it starts a longrun: an on-demand service
    /// started by a job (systemctl start, a dependency, D-Bus activation)
    /// does not wait for socket activity first. The runner takes it.
    pub const START_REQUESTED: &str = "start-requested";
}

/// The values of keys::RESULT, the contract between the runner (writes)
/// and the daemon (reads, and exports them as systemd's Result property).
/// Every value is one of systemd's Result names, except RUNNING, which is
/// ours: a start in progress, exported as SUCCESS. Write these constants,
/// never a literal: the test below fails on a literal in either crate.
pub mod result {
    pub const SUCCESS: &str = "success";
    pub const EXIT_CODE: &str = "exit-code";
    pub const SIGNAL: &str = "signal";
    pub const TIMEOUT: &str = "timeout";
    pub const RESOURCES: &str = "resources";
    pub const START_LIMIT_HIT: &str = "start-limit-hit";
    pub const PROTOCOL: &str = "protocol";
    /// Ours, not systemd's: the run script is starting the unit.
    pub const RUNNING: &str = "running";
    pub const ALL: &[&str] = &[
        SUCCESS,
        EXIT_CODE,
        SIGNAL,
        TIMEOUT,
        RESOURCES,
        START_LIMIT_HIT,
        PROTOCOL,
        RUNNING,
    ];

    /// A failed unit's result: anything but SUCCESS and RUNNING.
    pub fn is_failure(v: &str) -> bool {
        v != SUCCESS && v != RUNNING
    }

    /// As systemd's Result property shows it.
    pub fn exported(v: &str) -> &str {
        if v == RUNNING {
            SUCCESS
        } else {
            v
        }
    }
}

pub fn for_unit(run_dir: &Path, unit: &str) -> UnitState {
    UnitState::new(run_dir.join("units").join(&*util::path_component(unit)))
}

#[cfg(test)]
mod tests {
    use super::result;
    use std::path::Path;

    #[test]
    fn running_is_ours_and_shown_as_success() {
        assert!(!result::is_failure(result::SUCCESS));
        assert!(!result::is_failure(result::RUNNING));
        for v in result::ALL
            .iter()
            .filter(|v| **v != result::SUCCESS && **v != result::RUNNING)
        {
            assert!(result::is_failure(v), "{v}");
            assert_eq!(result::exported(v), *v);
        }
        assert_eq!(result::exported(result::RUNNING), result::SUCCESS);
    }

    /// The contract is the constants: a line that reads or writes
    /// keys::RESULT with a literal value in the runner or the daemon would
    /// drift from the other side unnoticed.
    #[test]
    fn result_values_are_written_as_constants() {
        fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    rs_files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        for c in [
            "uinitconverter-runner",
            "uinitconverterd",
            "uinitconverter-timers",
        ] {
            rs_files(&crates.join(c).join("src"), &mut files);
        }
        assert!(
            files.len() > 10,
            "no sources found under {}",
            crates.display()
        );
        let mut bad = Vec::new();
        for f in &files {
            for (n, line) in std::fs::read_to_string(f).unwrap().lines().enumerate() {
                if !line.contains("keys::RESULT") {
                    continue;
                }
                if result::ALL
                    .iter()
                    .any(|v| line.contains(&format!("\"{v}\"")))
                {
                    bad.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "write state::result constants:\n{}",
            bad.join("\n")
        );
    }
}
