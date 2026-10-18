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
    /// Set by the daemon before `s6-svc -r`, so finish restarts regardless of Restart=.
    pub const RESTART_REQUESTED: &str = "restart-requested";
}

pub fn for_unit(run_dir: &Path, unit: &str) -> UnitState {
    UnitState::new(run_dir.join("units").join(unit))
}
