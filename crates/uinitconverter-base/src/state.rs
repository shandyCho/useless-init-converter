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
        self.set(&format!("{key}-mono"), &util::now_monotonic_usec().to_string());
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

pub fn for_unit(run_dir: &Path, unit: &str) -> UnitState {
    UnitState::new(run_dir.join("units").join(&*util::path_component(unit)))
}
