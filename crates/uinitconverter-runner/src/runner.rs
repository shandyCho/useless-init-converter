//! The service runner: what systemd's PID 1 does between fork() and exec().
//!
//! Called from generated s6-rc scripts:
//!   run      longrun `run` script; ends by exec()ing the main process
//!   finish   longrun `finish` script; cleanup and Restart= policy
//!   up/down  oneshot scripts
//! and by the daemon:
//!   stop-exec / reload   ExecStop= / ExecReload= with $MAINPID

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::cgroup;
use crate::conditions;
use crate::config::Config;
use crate::execcmd::{self, ExecCommand};
use crate::install;
use crate::loader::{LoadState, Loader, Unit};
use crate::sandbox::{self, Sandbox};
use crate::socket;
use crate::state::{keys, result, UnitState};
use crate::supervisor;
use crate::tools;
use crate::util::{self, UserInfo};

mod connection;
mod environment;
mod exec;
mod forking;
mod notify;
mod sockets;
mod status;
mod stdio;

pub use connection::{accept_finish, accept_main, connection_main};
use environment::*;
use exec::*;
pub use notify::notify_helper_main;
use notify::*;
use status::*;

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn cstr(s: &str) -> CString {
    CString::new(
        s.as_bytes()
            .iter()
            .copied()
            .filter(|&b| b != 0)
            .collect::<Vec<u8>>(),
    )
    .unwrap()
}

pub struct Runner {
    pub cfg: Config,
    pub unit: Unit,
    pub st: UnitState,
    user: Option<UserInfo>,
    gid: Option<u32>,
    groups: Vec<u32>,
    /// Why User=/Group= didn't resolve, for a command other than a start:
    /// the exit code and message for any process spawned.
    cred_error: Option<(i32, String)>,
    pub env: Vec<(String, String)>,
    writable_dirs: Vec<String>,
    sandbox: Sandbox,
    notify_fd: Option<i32>,
    is_root: bool,
    /// Start timeout for commands run by the runner itself (oneshots, forking).
    deadline: std::cell::Cell<Option<std::time::Instant>>,
    /// What `deadline` limits, for the log: "start", or "stop" for ExecStop=.
    deadline_what: std::cell::Cell<&'static str>,
    timed_out: std::cell::Cell<bool>,
    /// `run --on-demand` (initconvert-gen): bind the sockets and wait for
    /// activity before the first start, as systemd's socket activation.
    pub on_demand: bool,
    /// `connection` (Accept=yes): the client's socket, and whether the
    /// command being started gets it (ExecStart= only).
    conn: Option<i32>,
    conn_main: std::cell::Cell<bool>,
    /// The connection's fd name (LISTEN_FDNAMES): the socket's
    /// FileDescriptorName=, "connection" without one, as systemd.
    conn_name: String,
}

fn log(unit: &str, msg: &str) {
    eprintln!("uinitconverter[{unit}]: {msg}");
    util::syslog_line(libc::LOG_ERR, &format!("{unit}: {msg}"));
}

impl Runner {
    /// `starting`: the command starts the unit (`run`, `up`), so a User=,
    /// Group= or SupplementaryGroups= that doesn't resolve fails it.
    pub fn new(cfg: Config, name: &str, starting: bool) -> Result<Runner, String> {
        let unit = Loader::new(&cfg).load(name);
        if unit.load_state != LoadState::Loaded {
            return Err(format!("unit {name}: {}", unit.load_state.as_str()));
        }
        let st = crate::state::for_unit(&cfg.run_dir, &unit.id);
        st.ensure();
        let is_root = unsafe { libc::geteuid() } == 0;

        // Only a start needs the credentials. For the other commands a user
        // that is gone by now (GDM's greeter user, dropped from userdb before
        // its user@ stops) must not skip the stop's bookkeeping; as systemd,
        // only a process spawned with them fails (217/USER, 216/GROUP).
        let (user, gid, groups, cred_error) = match resolve_credentials(&unit) {
            Ok((user, gid, groups)) => (user, gid, groups, None),
            Err((_, e)) if starting => return Err(e),
            Err(e) => (None, None, Vec::new(), Some(e)),
        };

        let mut r = Runner {
            on_demand: false,
            sandbox: Sandbox::from_unit(&unit, &st.dir, Vec::new()),
            cfg,
            unit,
            st,
            user,
            gid,
            groups,
            cred_error,
            env: Vec::new(),
            writable_dirs: Vec::new(),
            notify_fd: None,
            is_root,
            deadline: std::cell::Cell::new(None),
            deadline_what: std::cell::Cell::new("start"),
            timed_out: std::cell::Cell::new(false),
            conn: None,
            conn_main: std::cell::Cell::new(false),
            conn_name: "connection".into(),
        };
        r.build_env()?;
        Ok(r)
    }

    fn id(&self) -> &str {
        &self.unit.id
    }

    pub fn setenv(&mut self, k: &str, v: &str) {
        env_set(&mut self.env, k, v);
    }

    // ---- directories ----

    // ---- process setup ----

    fn start_timeout(&self) -> Option<std::time::Duration> {
        let u = self.unit.start_timeout_usec();
        (u != u64::MAX).then(|| std::time::Duration::from_micros(u))
    }

    /// A required unit (Requires=/Requisite=/BindsTo=) that failed. s6-rc
    /// only orders units; requirement failures are enforced here.
    fn failed_requirement(&self) -> Option<String> {
        let ld = Loader::new(&self.cfg);
        let idx = crate::index::Index::load(&self.cfg);
        let mut deps = self.unit.get_words("Unit", "Requires");
        deps.extend(self.unit.get_words("Unit", "Requisite"));
        deps.extend(self.unit.get_words("Unit", "BindsTo"));
        deps.extend(ld.dir_deps(&self.unit, "requires"));
        for d in deps {
            let d = idx.resolve(&d).to_string();
            if d == self.unit.id {
                continue;
            }
            let st = crate::state::for_unit(&self.cfg.run_dir, &d);
            if st.has(keys::START_FAILED) || st.has(keys::DEPENDENCY_FAILED) {
                return Some(d);
            }
            let failed = st
                .get(keys::RESULT)
                .map(|r| result::is_failure(&r))
                .unwrap_or(false);
            if failed && st.get_u64(keys::INACTIVE_ENTER) >= st.get_u64(keys::INACTIVE_EXIT) {
                return Some(d);
            }
        }
        None
    }

    /// Start failed: record it, but still tell s6-rc "ready" so units that
    /// merely want this one are not blocked (systemd semantics).
    fn soft_fail(&mut self, reason: &str) {
        log(self.id(), reason);
        self.st.set(keys::START_FAILED, reason);
        self.signal_ready();
    }

    fn signal_ready(&mut self) {
        if let Some(fd) = self.notify_fd.take() {
            unsafe {
                libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
                libc::close(fd);
            }
        }
    }

    /// s6 hands us the notification pipe on fd 3; move it out of the way so
    /// fds 3.. are free for LISTEN_FDS.
    fn take_notify_fd(&mut self) {
        unsafe {
            if libc::fcntl(3, libc::F_GETFD) >= 0 {
                let fd = libc::fcntl(3, libc::F_DUPFD_CLOEXEC, 64);
                libc::close(3);
                if fd >= 0 {
                    self.notify_fd = Some(fd);
                }
            }
        }
    }

    fn prepare_start(&mut self) -> Result<(), String> {
        for k in [
            keys::CONDITION_FAILED,
            keys::ASSERT_FAILED,
            keys::STATUS_TEXT,
            keys::MAIN_PID,
            keys::START_FAILED,
            keys::DEPENDENCY_FAILED,
            keys::PARKED,
            keys::EXTEND_UNTIL,
            // A new run starts with a clean result, as systemd's
            // service_start() does; otherwise the exit handler keeps a
            // previous run's failure.
            keys::RESULT,
        ] {
            self.st.clear(k);
        }
        let inv = util::read_trimmed(Path::new("/proc/sys/kernel/random/uuid"))
            .unwrap_or_default()
            .replace('-', "");
        self.st.set(keys::INVOCATION, &inv);
        // journald reads the invocation ID from this xattr on the unit's cgroup
        // (systemctl status uses it to pick the current run's log lines).
        if let Some(dir) = self.cgroup_dir() {
            let bytes: Vec<u8> = (0..inv.len() / 2)
                .filter_map(|i| u8::from_str_radix(&inv[i * 2..i * 2 + 2], 16).ok())
                .collect();
            let p = cstr(&dir.display().to_string());
            unsafe {
                libc::setxattr(
                    p.as_ptr(),
                    c"trusted.invocation_id".as_ptr(),
                    bytes.as_ptr() as *const libc::c_void,
                    bytes.len(),
                    0,
                );
            }
        }
        self.setenv("INVOCATION_ID", &inv);
        self.memory_pressure_env();
        self.st.stamp(keys::INACTIVE_EXIT);
        self.create_directories()?;
        self.sandbox.prepare()?;
        Ok(())
    }

    /// Conditions and ExecCondition=. Ok(false) means "skip".
    fn check_conditions(&self) -> Result<bool, String> {
        let ev = conditions::evaluate(&self.unit);
        if !ev.asserts_ok {
            let why = ev.failure.unwrap_or_default();
            self.st.set(keys::ASSERT_FAILED, &why);
            return Err(format!("assertion failed: {why}"));
        }
        if !ev.conditions_ok {
            self.st
                .set(keys::CONDITION_FAILED, &ev.failure.unwrap_or_default());
            return Ok(false);
        }
        for line in self.unit.get_all("Service", "ExecCondition") {
            for cmd in execcmd::parse(&line)? {
                match self.spawn_wait(&cmd, &[]) {
                    Status::Exited(0) => {}
                    Status::Exited(c) if (1..255).contains(&c) => {
                        self.st.set(
                            keys::CONDITION_FAILED,
                            &format!("ExecCondition={} exited {c}", cmd.path),
                        );
                        return Ok(false);
                    }
                    other => return Err(format!("ExecCondition failed: {}", describe(other))),
                }
            }
        }
        Ok(true)
    }

    fn service_type(&self) -> String {
        self.unit.service_type()
    }

    fn main_command(&self) -> Result<ExecCommand, String> {
        let lines = self.unit.get_all("Service", "ExecStart");
        if lines.len() != 1 {
            return Err(format!(
                "expected exactly one ExecStart= for a long-running service, found {}",
                lines.len()
            ));
        }
        let cmd = execcmd::parse(&lines[0])?
            .into_iter()
            .next()
            .ok_or_else(|| "empty ExecStart=".to_string())?;
        Ok(self.user_manager_command(cmd))
    }

    /// USER_MANAGER=s6: user@UID.service runs uinitconverter's user manager
    /// instead of `systemd --user` (work_history part 141). The unit file
    /// stays as it is, so a systemd boot is not touched.
    fn user_manager_command(&self, cmd: ExecCommand) -> ExecCommand {
        let is_systemd_user = Path::new(&cmd.path).file_name() == Some("systemd".as_ref())
            && cmd.argv.iter().any(|a| a == "--user");
        if !self.unit.id.starts_with("user@") || !is_systemd_user {
            return cmd;
        }
        let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
        if self.cfg.user_manager_now(&cmdline) != "s6" {
            return cmd;
        }
        // The s6 user manager is the uinitconverter-user package: without
        // it, user@ keeps systemd --user.
        let manager = crate::config::tool_path("initconvert-user-manager");
        if !manager.exists() {
            log(
                self.id(),
                &format!(
                    "USER_MANAGER=s6, but {} is missing (install uinitconverter-user): running systemd --user",
                    manager.display()
                ),
            );
            return cmd;
        }
        let manager = manager.display().to_string();
        let mut argv = vec![manager.clone()];
        if self.cfg.config_path != Path::new(crate::config::DEFAULT_CONFIG) {
            argv.push("--config".into());
            argv.push(self.cfg.config_path.display().to_string());
        }
        ExecCommand {
            path: manager,
            argv,
            ..cmd
        }
    }

    // ---- subcommands ----

    /// longrun `run` script.
    pub fn cmd_run(mut self) -> i32 {
        self.take_notify_fd();
        if self.should_wait_on_demand() {
            self.st.set(keys::SOCKET_WAIT, "1");
        }
        if self.st.has(keys::SOCKET_WAIT) {
            // Ready for s6 while waiting, like a listening socket unit: an
            // `s6-rc -u change` recording a unit started beside the boot's
            // change would otherwise wait for readiness until its timeout,
            // then bring it down. The daemon's start waits on the unit state.
            self.signal_ready();
            self.wait_for_sockets();
            self.start_waiting_requirements();
        }
        self.st.set(keys::ACTIVATED, "1");
        self.st.clear(keys::START_REQUESTED);
        self.enter_cgroup();
        if let Err(e) = self.prepare_start() {
            self.soft_fail(&e);
            return 1;
        }
        if let Some(dep) = self.failed_requirement() {
            // Like systemd's "dependency" job result: not started, not failed.
            log(
                self.id(),
                &format!("not starting: required unit {dep} failed"),
            );
            self.st.set(keys::DEPENDENCY_FAILED, &dep);
            self.signal_ready();
            loop {
                unsafe { libc::pause() };
            }
        }
        match self.check_conditions() {
            Ok(true) => {}
            Ok(false) => {
                // Stay "up" for s6 but report the unit as skipped.
                log(self.id(), "condition check failed, not starting");
                self.signal_ready();
                loop {
                    unsafe { libc::pause() };
                }
            }
            Err(e) => {
                self.st.set(keys::RESULT, result::EXIT_CODE);
                self.soft_fail(&e);
                return 1;
            }
        }
        self.deadline
            .set(self.start_timeout().map(|t| std::time::Instant::now() + t));
        if let Err(st) = self.run_list("ExecStartPre", &[]) {
            self.st.set(
                keys::RESULT,
                if self.timed_out.get() {
                    result::TIMEOUT
                } else {
                    result_for(st)
                },
            );
            self.soft_fail("ExecStartPre= failed");
            return exit_code_for(st);
        }
        let main = match self.main_command() {
            Ok(c) => c,
            Err(e) => {
                self.soft_fail(&e);
                return EXIT_EXEC;
            }
        };
        let ty = self.service_type();
        if ty == "forking" {
            return self.run_forking(&main);
        }
        self.deadline.set(None);

        if let Err(e) = self.setup_listen_fds() {
            self.st.set(keys::RESULT, result::RESOURCES);
            self.soft_fail(&e);
            return 1;
        }

        let pid = std::process::id();
        self.st.set(keys::MAIN_PID, &pid.to_string());
        self.st.stamp(keys::EXEC_MAIN_START);

        match ty.as_str() {
            "notify" | "notify-reload" => match self.start_notify_helper(pid) {
                Ok(sock) => self.setenv("NOTIFY_SOCKET", &sock),
                Err(e) => {
                    self.soft_fail(&e);
                    return 1;
                }
            },
            "dbus" => self.start_dbus_helper(pid),
            _ => {
                if let Err(e) = self.start_exec_helper() {
                    self.soft_fail(&e);
                    return 1;
                }
            }
        }
        // The notify fd now belongs to the helper.
        if let Some(fd) = self.notify_fd.take() {
            unsafe { libc::close(fd) };
        }
        if let (Some(service), true) = (self.unit.get("Service", "PAMName"), self.is_root) {
            return self.exec_with_pam(&service, &main);
        }
        if matches!(ty.as_str(), "notify" | "notify-reload")
            && self.unit.get("Service", "NotifyAccess").as_deref() == Some("all")
        {
            return self.run_guarded(&main);
        }
        let (code, msg) = self.exec_command(&main, &[]);
        log(self.id(), &msg);
        if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.get()) {
            let b = (code as u32).to_ne_bytes();
            unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 4) };
        }
        code
    }

    /// OnSuccess=/OnFailure= and SuccessAction=/FailureAction= after the unit finished.
    /// StartUnit for each of `units` through the daemon, detached: the
    /// daemon may be busy with the very job that ran us.
    fn start_units_detached(&self, why: &str, units: Vec<String>) {
        if units.is_empty() {
            return;
        }
        let cfg = self.cfg.clone();
        let id = self.unit.id.clone();
        let run = || -> Result<(), String> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(async {
                let conn = crate::client::connect(&cfg).await?;
                for u in &units {
                    if let Err(e) =
                        crate::client::manager_call(&conn, "StartUnit", &(u.as_str(), "replace"))
                            .await
                    {
                        eprintln!("uinitconverter[{id}]: {why}={u}: {e}");
                    }
                }
                Ok(())
            })
        };
        let _ = double_fork(|| {
            if let Err(e) = run() {
                log(self.id(), &format!("{why}: {e}"));
            }
        });
    }

    /// A unit's ActiveState from the manager (None if it can't be asked).
    fn unit_active_state(&self, name: &str) -> Option<String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        rt.block_on(async {
            let conn = crate::client::connect(&self.cfg).await.ok()?;
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                crate::client::unit_active_state(&conn, name),
            )
            .await
            .ok()?
            .ok()
        })
    }

    /// A Manager method without arguments, detached like
    /// start_units_detached.
    fn call_manager_detached(&self, method: &'static str) {
        let cfg = self.cfg.clone();
        let id = self.unit.id.clone();
        let _ = double_fork(move || {
            let r = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| {
                    rt.block_on(async {
                        let conn = crate::client::connect(&cfg).await?;
                        crate::client::manager_call(&conn, method, &())
                            .await
                            .map(|_| ())
                    })
                });
            if let Err(e) = r {
                eprintln!("uinitconverter[{id}]: {method}: {e}");
            }
        });
    }

    /// Leaving the socket wait: as systemd's activation is a start job for
    /// the service, its Requires=, BindsTo= and Wants= (and .wants/,
    /// .requires/) start with it: pipewire.service's .wants/wireplumber
    /// (without it no stream reaches a sink, and gnome-shell waited for an
    /// unplayed sound at exit), iscsiuio's Requires=iscsid. Those that are
    /// on demand too have their sockets listening already, so this one
    /// need not wait; for the others StartUnit is a no-op when they run.
    fn start_waiting_requirements(&self) {
        let ld = Loader::new(&self.cfg);
        let mut names = Vec::new();
        for key in ["Requires", "BindsTo", "Wants"] {
            names.extend(self.unit.get_words("Unit", key));
        }
        names.extend(ld.dir_deps(&self.unit, "requires"));
        names.extend(ld.dir_deps(&self.unit, "wants"));
        let mut ids: Vec<String> = Vec::new();
        for n in names {
            let id = ld.load(&n).id;
            if id != self.unit.id && !ids.contains(&id) {
                ids.push(id);
            }
        }
        self.start_units_detached("on demand", ids);
    }

    fn on_result(&self, success: bool) {
        let (units_key, action_key) = if success {
            ("OnSuccess", "SuccessAction")
        } else {
            ("OnFailure", "FailureAction")
        };
        let units: Vec<String> = self
            .unit
            .get_words("Unit", units_key)
            .into_iter()
            .map(|u| u.replace("%n", &self.unit.id))
            .collect();
        // A user manager's session failing into USER_FALLBACK_ON= (GNOME's
        // session units: OnFailure=gnome-session-shutdown.target): the
        // daemon hands the user over to systemd --user (it reads this
        // request in its watcher loop).
        // Not when that unit is up or on its way already: a logout under
        // way (gnome-session-shutdown.target's Conflicts= stopping units,
        // which may fail into it). Asked before this OnFailure= starts it.
        if !success {
            if let Some(u) = units.iter().find(|u| self.cfg.is_fallback_unit(u)) {
                let state = self.unit_active_state(u);
                if matches!(
                    state.as_deref(),
                    Some("active" | "activating" | "deactivating")
                ) {
                    log(
                        self.id(),
                        &format!("failed while {u} is under way: no fallback"),
                    );
                } else {
                    let text = format!("{u}\n{} failed (OnFailure={u})", self.unit.id);
                    let _ = util::write_atomic(
                        &self.cfg.run_dir.join(crate::state::FALLBACK_REQUEST),
                        text.as_bytes(),
                    );
                }
            }
        }
        self.start_units_detached(units_key, units);
        if let Some(action) = self.unit.get("Unit", action_key) {
            // A user manager may only exit (systemd allows none, exit and
            // exit-force there): systemctl --user exit is exit.target's
            // systemd-exit.service with SuccessAction=exit-force.
            if self.cfg.user_mode {
                if action.starts_with("exit") {
                    log(
                        self.id(),
                        &format!("{action_key}={action}: the user manager exits"),
                    );
                    self.call_manager_detached("Exit");
                } else if action != "none" {
                    log(
                        self.id(),
                        &format!("{action_key}={action} is not allowed in a user manager"),
                    );
                }
                return;
            }
            let cmd = match action.as_str() {
                "none" => None,
                a if a.starts_with("reboot") || a == "soft-reboot" || a == "kexec" => {
                    Some(self.cfg.reboot_cmd.clone())
                }
                a if a.starts_with("poweroff") || a.starts_with("exit") => {
                    Some(self.cfg.poweroff_cmd.clone())
                }
                a if a.starts_with("halt") => Some(self.cfg.halt_cmd.clone()),
                other => {
                    log(self.id(), &format!("{action_key}={other} is not supported"));
                    None
                }
            };
            if let Some(cmd) = cmd {
                log(self.id(), &format!("{action_key}={action}: running {cmd}"));
                let _ = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(cmd)
                    .spawn();
            }
        }
    }

    fn after_ready(&self, main_pid: u32) {
        self.st.stamp(keys::ACTIVE_ENTER);
        if let Err(st) = self.run_list("ExecStartPost", &[("MAINPID".into(), main_pid.to_string())])
        {
            self.st.set(keys::RESULT, result_for(st));
            unsafe { libc::kill(main_pid as i32, libc::SIGTERM) };
        }
    }

    /// longrun `finish` script: cleanup and Restart= policy.
    pub fn cmd_finish(self, exit_code: i32, signum: i32) -> i32 {
        let stopped = !supervisor::wanted_up();
        // Stopped: an on-demand service waits for activity again next time.
        if stopped {
            self.st.clear(keys::ACTIVATED);
        }
        if self.st.has(keys::SOCKET_WAIT) {
            // Ended while waiting for its sockets: the service wasn't running.
            let requested_restart = self.st.has(keys::RESTART_REQUESTED);
            self.st.clear(keys::RESTART_REQUESTED);
            if stopped || requested_restart {
                self.st.clear(keys::SOCKET_WAIT);
            }
            if stopped {
                for s in self.socket_units() {
                    socket::remove_paths(&s);
                }
            }
            return 0;
        }
        let status = if exit_code == 256 || exit_code < 0 && signum > 0 {
            Status::Killed(signum)
        } else {
            Status::Exited(exit_code)
        };
        self.st.stamp(keys::EXEC_MAIN_EXIT);
        let (code, val) = match status {
            Status::Exited(c) => (1, c),
            Status::Killed(s) => (2, s),
        };
        self.st.set(keys::EXEC_MAIN_CODE, &code.to_string());
        self.st.set(keys::EXEC_MAIN_STATUS, &val.to_string());
        self.st.stamp(keys::ACTIVE_EXIT);

        let requested_restart = self.st.has(keys::RESTART_REQUESTED);
        self.st.clear(keys::RESTART_REQUESTED);
        let stopping = stopped || requested_restart;
        let kill_signal = self
            .unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .unwrap_or(libc::SIGTERM);
        let clean_exit =
            self.is_clean(status) || (stopping && status == Status::Killed(kill_signal));
        let mut result = if clean_exit {
            result::SUCCESS
        } else {
            match status {
                Status::Killed(_) => result::SIGNAL,
                Status::Exited(_) => result::EXIT_CODE,
            }
        }
        .to_string();
        if let Some(r) = self.st.get(keys::RESULT) {
            if r != result::SUCCESS && !stopping {
                result = r;
            }
        }
        // A start timeout or failed ExecStartPost= ends in our own SIGTERM,
        // which on its own would count as clean.
        let clean = clean_exit && result == result::SUCCESS;

        self.cleanup(&result, status, stopping);

        if stopping {
            self.st.set(keys::RESULT, &result);
            if !requested_restart {
                self.st.stamp(keys::INACTIVE_ENTER);
            }
            return 0;
        }
        if self.st.has(keys::ASSERT_FAILED) {
            self.st.set(keys::RESULT, result::EXIT_CODE);
            return 125;
        }

        let restart_mode = self
            .unit
            .get("Service", "Restart")
            .unwrap_or_else(|| "no".into());
        let mut restart = match restart_mode.as_str() {
            "always" => true,
            "on-success" => clean,
            "on-failure" => !clean,
            "on-abnormal" | "on-watchdog" => {
                result == result::TIMEOUT || matches!(status, Status::Killed(_)) && !clean_exit
            }
            "on-abort" => matches!(status, Status::Killed(_)) && !clean_exit,
            _ => false,
        };
        if status_in(
            &self.unit.get_words("Service", "RestartPreventExitStatus"),
            status,
        ) {
            restart = false;
        }
        if status_in(
            &self.unit.get_words("Service", "RestartForceExitStatus"),
            status,
        ) {
            restart = true;
        }

        if restart {
            let interval = self
                .unit
                .get("Unit", "StartLimitIntervalSec")
                .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
                .unwrap_or(10_000_000);
            let burst: usize = self
                .unit
                .get("Unit", "StartLimitBurst")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5);
            let now = util::now_monotonic_usec();
            // No limit: nothing to remember (the list would only grow).
            let limited = interval > 0 && burst > 0;
            let mut times: Vec<u64> = self
                .st
                .get(keys::RESTART_TIMES)
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .filter(|&t: &u64| limited && now.saturating_sub(t) < interval)
                .collect();
            if limited {
                times.push(now);
            }
            self.st.set(
                keys::RESTART_TIMES,
                &times
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            if limited && times.len() > burst {
                log(self.id(), "start request repeated too quickly, giving up");
                self.st.set(keys::RESULT, result::START_LIMIT_HIT);
                self.st.stamp(keys::INACTIVE_ENTER);
                self.on_result(false);
                return 125;
            }
            let n = self.st.get_u64(keys::NRESTARTS) + 1;
            self.st.set(keys::NRESTARTS, &n.to_string());
            self.st.set(keys::RESULT, &result);
            let delay = self
                .unit
                .get("Service", "RestartSec")
                .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
                .unwrap_or(100_000);
            // s6-supervise already waits ~1s between restarts.
            if delay > 1_000_000 {
                std::thread::sleep(std::time::Duration::from_micros(delay - 1_000_000));
            }
            return 0;
        }
        self.st.set(keys::RESULT, &result);
        self.st.stamp(keys::INACTIVE_ENTER);
        self.on_result(result == result::SUCCESS);
        if result == result::SUCCESS && self.has_held_sockets() {
            // Stay up for s6: the next run waits for socket activity.
            self.st.set(keys::SOCKET_WAIT, "1");
            return 0;
        }
        125
    }

    fn is_clean(&self, status: Status) -> bool {
        if status_in(&self.unit.get_words("Service", "SuccessExitStatus"), status) {
            return true;
        }
        match status {
            Status::Exited(0) => true,
            Status::Killed(s) => {
                [libc::SIGHUP, libc::SIGINT, libc::SIGTERM, libc::SIGPIPE].contains(&s)
            }
            _ => false,
        }
    }

    fn cleanup(&self, result: &str, status: Status, stopping: bool) {
        let (code, st) = match status {
            Status::Exited(c) => ("exited", c.to_string()),
            Status::Killed(s) => ("killed", util::signal_name(s)),
        };
        let env = [
            ("SERVICE_RESULT".to_string(), result.to_string()),
            ("EXIT_CODE".to_string(), code.to_string()),
            ("EXIT_STATUS".to_string(), st),
        ];
        // KillMode=control-group / mixed: leftover processes go too, before
        // ExecStopPost= and again after it (systemd's stop-sigterm,
        // stop-post and final-sigterm states).
        let mode = self
            .unit
            .get("Service", "KillMode")
            .unwrap_or_else(|| "control-group".into());
        let kill_leftovers = || {
            if mode != "control-group" && mode != "mixed" {
                return;
            }
            let Some(dir) = self.cgroup_dir() else { return };
            let me = std::process::id();
            if cgroup::kill_all(&dir, libc::SIGTERM, me) > 0 {
                for _ in 0..20 {
                    if util::cgroup_pids(&dir).is_empty() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if self.unit.get_bool("Service", "SendSIGKILL", true) {
                    cgroup::kill_all(&dir, libc::SIGKILL, me);
                }
            }
        };
        kill_leftovers();
        let _ = self.run_list("ExecStopPost", &env);
        kill_leftovers();
        if let Some(sock) = self.st.get(keys::NOTIFY_SOCKET) {
            let _ = fs::remove_file(sock);
            self.st.clear(keys::NOTIFY_SOCKET);
        }
        // Listening sockets stay in the daemon's fd store across restarts;
        // only a real stop removes their paths (RemoveOnStop=).
        if stopping {
            for s in self.socket_units() {
                socket::remove_paths(&s);
            }
        }
        self.remove_runtime_dirs();
        let _ = fs::remove_dir_all(self.st.dir.join("private"));
    }

    /// .mount/.swap: mount(8) or swapon(8), with the usual state bookkeeping.
    fn mount_job(&self, up: bool) -> i32 {
        use crate::mountunit;
        let code = match (self.unit.utype(), up) {
            ("mount", true) => mountunit::mount_up(&self.unit),
            ("mount", false) => mountunit::mount_down(&self.unit),
            ("swap", true) => mountunit::swap_up(&self.unit),
            _ => mountunit::swap_down(&self.unit),
        };
        if up {
            self.st.stamp(keys::INACTIVE_EXIT);
            self.st.set(
                keys::RESULT,
                if code == 0 {
                    result::SUCCESS
                } else {
                    result::EXIT_CODE
                },
            );
            self.st.set(keys::EXEC_MAIN_STATUS, &code.to_string());
            if code == 0 {
                self.st.stamp(keys::ACTIVE_ENTER);
            } else {
                self.st.stamp(keys::INACTIVE_ENTER);
            }
        } else {
            self.st.stamp(keys::ACTIVE_EXIT);
            self.st.stamp(keys::INACTIVE_ENTER);
        }
        code
    }

    /// oneshot `up` script.
    pub fn cmd_up(mut self) -> i32 {
        // Every outcome is reported to s6-rc as success: a failing oneshot
        // must not block units that only want it. The result is recorded and
        // dependents with Requires= check it (see failed_requirement()).
        if let Some(dep) = self.failed_requirement() {
            log(
                self.id(),
                &format!("not starting: required unit {dep} failed"),
            );
            self.st.set(keys::DEPENDENCY_FAILED, &dep);
            return 0;
        }
        if matches!(self.unit.utype(), "mount" | "swap") {
            match conditions::evaluate(&self.unit) {
                ev if !ev.conditions_ok => {
                    self.st
                        .set(keys::CONDITION_FAILED, &ev.failure.unwrap_or_default());
                    return 0;
                }
                _ => self.st.clear(keys::CONDITION_FAILED),
            }
            let code = self.mount_job(true);
            if code != 0 {
                self.on_result(false);
            }
            return 0;
        }
        self.enter_cgroup();
        if let Err(e) = self.prepare_start() {
            log(self.id(), &e);
            self.st.set(keys::RESULT, result::RESOURCES);
            self.st.stamp(keys::INACTIVE_ENTER);
            return 0;
        }
        match self.check_conditions() {
            Ok(true) => {}
            Ok(false) => {
                log(self.id(), "condition check failed, skipped");
                self.st.set(keys::RESULT, result::SUCCESS);
                return 0;
            }
            Err(e) => {
                log(self.id(), &e);
                self.st.set(keys::RESULT, result::EXIT_CODE);
                self.st.stamp(keys::INACTIVE_ENTER);
                self.on_result(false);
                return 0;
            }
        }
        self.st.set(keys::RESULT, result::RUNNING);
        self.st.stamp(keys::EXEC_MAIN_START);
        self.deadline
            .set(self.start_timeout().map(|t| std::time::Instant::now() + t));
        let mut failed: Option<Status> = None;
        for key in ["ExecStartPre", "ExecStart", "ExecStartPost"] {
            if let Err(st) = self.run_list(key, &[]) {
                failed = Some(st);
                break;
            }
        }
        self.deadline.set(None);
        self.st.stamp(keys::EXEC_MAIN_EXIT);
        if let Some(st) = failed {
            let (code, val) = match st {
                Status::Exited(c) => (1, c),
                Status::Killed(s) => (2, s),
            };
            let result = if self.timed_out.get() {
                "timeout"
            } else {
                result_for(st)
            };
            self.st.set(keys::EXEC_MAIN_CODE, &code.to_string());
            self.st.set(keys::EXEC_MAIN_STATUS, &val.to_string());
            self.st.set(keys::RESULT, result);
            self.cleanup(result, st, true);
            self.st.stamp(keys::INACTIVE_ENTER);
            self.on_result(false);
            return 0;
        }
        self.st.set(keys::EXEC_MAIN_CODE, "1");
        self.st.set(keys::EXEC_MAIN_STATUS, "0");
        self.st.set(keys::RESULT, result::SUCCESS);
        self.st.stamp(keys::ACTIVE_ENTER);
        if !self.unit.get_bool("Service", "RemainAfterExit", false) {
            // systemd stops a oneshot right after it finished.
            let _ = self.run_list("ExecStop", &[]);
            self.cleanup(result::SUCCESS, Status::Exited(0), true);
            self.st.stamp(keys::INACTIVE_ENTER);
        }
        self.on_result(true);
        0
    }

    /// oneshot `down` script.
    pub fn cmd_down(self) -> i32 {
        if matches!(self.unit.utype(), "mount" | "swap") {
            return self.mount_job(false);
        }
        if self.unit.get_bool("Service", "RemainAfterExit", false) {
            let _ = self.run_list("ExecStop", &[]);
            self.cleanup(result::SUCCESS, Status::Exited(0), true);
        }
        self.st.stamp(keys::ACTIVE_EXIT);
        self.st.stamp(keys::INACTIVE_ENTER);
        0
    }

    /// ExecStop= of a longrun (called by the daemon before s6-rc -d).
    pub fn cmd_stop_exec(self, main_pid: Option<u32>) -> i32 {
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        match self.run_list("ExecStop", &env) {
            Ok(()) => 0,
            Err(st) => exit_code_for(st),
        }
    }

    /// `down` of the longrun's UNIT.stop oneshot (initconvert-gen), which s6-rc
    /// takes down before the longrun: ExecStop= with $MAINPID, as systemd
    /// runs it before the KillSignal=, if the service runs. Skipped, as
    /// systemd's stop of an inactive unit, when it was not started or waits
    /// for socket activity. Always 0: s6-rc would keep a service whose
    /// ExecStop= failed up.
    pub fn cmd_stop_atom(self) -> i32 {
        let skipped = [
            keys::SOCKET_WAIT,
            keys::CONDITION_FAILED,
            keys::DEPENDENCY_FAILED,
            keys::START_FAILED,
            keys::PARKED,
        ]
        .into_iter()
        .any(|k| self.st.has(k));
        if skipped {
            return 0;
        }
        let Some(sv_pid) = supervisor::up_pid(&self.cfg, self.id()) else {
            return 0;
        };
        // The service's own PID: under PAMName= or Type=forking the
        // supervised process is the runner around it.
        let main_pid = self
            .st
            .get(keys::MAIN_PID)
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|&p| p > 0 && unsafe { libc::kill(p as i32, 0) } == 0)
            .or(sv_pid);
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        let t = self.unit.stop_timeout_usec();
        if t != u64::MAX {
            self.deadline.set(Some(
                std::time::Instant::now() + std::time::Duration::from_micros(t),
            ));
            self.deadline_what.set("stop");
        }
        let _ = self.run_list("ExecStop", &env);
        0
    }

    /// ExecReload=, or ReloadSignal for Type=notify-reload.
    pub fn cmd_reload(self, main_pid: Option<u32>) -> i32 {
        if self.unit.get_all("Service", "ExecReload").is_empty() {
            if self.service_type() == "notify-reload" {
                if let Some(p) = main_pid {
                    let sig = self
                        .unit
                        .get("Service", "ReloadSignal")
                        .and_then(|s| util::signal_number(&s))
                        .unwrap_or(libc::SIGHUP);
                    let before = self
                        .st
                        .get(keys::RELOADS_DONE)
                        .map(|_| self.st.get_u64(keys::RELOADS_DONE));
                    if unsafe { libc::kill(p as i32, sig) } < 0 {
                        return 1;
                    }
                    // Like systemd: done once the service has sent RELOADING=1
                    // and then READY=1, within TimeoutStartSec=.
                    let Some(before) = before else { return 0 };
                    let deadline = self.start_timeout().map(|t| std::time::Instant::now() + t);
                    while self.st.get_u64(keys::RELOADS_DONE) == before {
                        if unsafe { libc::kill(p as i32, 0) } < 0 {
                            log(self.id(), "exited while reloading");
                            return 1;
                        }
                        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                            log(self.id(), "reload timed out");
                            return 1;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    return 0;
                }
            }
            log(self.id(), "no ExecReload= configured");
            return 1;
        }
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        match self.run_list("ExecReload", &env) {
            Ok(()) => 0,
            Err(st) => exit_code_for(st),
        }
    }
}
