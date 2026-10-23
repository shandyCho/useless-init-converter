//! s6 / s6-rc backend: unit state queries and job execution.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::Command;

use super::Daemon;
use crate::gen::Kind;
use crate::loader::Unit;
use crate::state::{self, keys};
use crate::util;

#[derive(Debug, Clone, Default)]
pub struct SvStat {
    pub up: bool,
    pub wanted_up: bool,
    pub ready: bool,
    pub pid: u32,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub active: &'static str,
    pub sub: &'static str,
    pub main_pid: u32,
    pub result: String,
    pub condition_ok: bool,
}

/// Whether another process holds a lock on `path`, as skalibs' fd_islocked()
/// asks (s6-rc's change lock, s6-supervise's supervise/lock). skalibs locks
/// with fcntl(); F_GETLK only looks.
pub(super) fn lock_held(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::File::open(path) else { return false };
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_RDLCK as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETLK, &mut fl) };
    r == 0 && fl.l_type != libc::F_UNLCK as libc::c_short
}

impl Daemon {
    pub(super) fn runner_cmd(&self, sub: &str) -> Command {
        let mut c = Command::new(&self.cfg.runner);
        c.arg(sub);
        if !self.cfg.runner_args().is_empty() {
            c.arg("--config").arg(&self.cfg.config_path);
        }
        c
    }

    pub(super) fn scandir(&self, atom: &str) -> PathBuf {
        self.cfg.live_dir.join("scandir").join(atom)
    }

    async fn s6rc_db_list(&self, what: &str) -> Vec<String> {
        let out = Command::new("s6-rc-db")
            .arg("-l")
            .arg(&self.cfg.live_dir)
            .args(["list", what])
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Atom names in the order of the live `state` file, cached per compiled db.
    async fn atom_order(&self) -> Vec<String> {
        let db = std::fs::canonicalize(self.cfg.live_dir.join("compiled")).unwrap_or_default();
        if let Some((cached_db, order)) = self.atom_order.lock().unwrap().as_ref() {
            if *cached_db == db {
                return order.clone();
            }
        }
        let mut atomic: BTreeSet<String> = self.s6rc_db_list("longruns").await.into_iter().collect();
        atomic.extend(self.s6rc_db_list("oneshots").await);
        // s6-rc-db lists services in descending index order.
        let mut order: Vec<String> = self
            .s6rc_db_list("all")
            .await
            .into_iter()
            .filter(|n| atomic.contains(n))
            .collect();
        order.reverse();
        *self.atom_order.lock().unwrap() = Some((db, order.clone()));
        order
    }

    fn active_from_state_file(&self, order: &[String]) -> Option<BTreeSet<String>> {
        let bytes = std::fs::read(self.cfg.live_dir.join("state")).ok()?;
        (bytes.len() == order.len() && !order.is_empty()).then(|| {
            order
                .iter()
                .zip(bytes.iter())
                .filter(|(_, b)| *b & 1 != 0)
                .map(|(n, _)| n.clone())
                .collect()
        })
    }

    /// Active atoms. Read from the live state file so queries never contend
    /// for the s6-rc lock while a job runs; that layout is not documented, so
    /// it is checked against `s6-rc -a list` for every database, and if they
    /// disagree twice in a row the slower `s6-rc -a list` is used instead.
    /// Atoms that are up: s6-rc's view plus the longruns started beside an
    /// s6-rc change that s6-rc has not been told about yet.
    pub(super) async fn active_atoms(&self) -> BTreeSet<String> {
        let mut active = self.active_atoms_live().await;
        active.extend(self.beside.lock().unwrap().iter().cloned());
        active
    }

    async fn active_atoms_live(&self) -> BTreeSet<String> {
        let order = self.atom_order().await;
        let from_file = self.active_from_state_file(&order);
        let (trusted, last) = *self.state_reader.lock().unwrap();
        match trusted {
            Some(true) => {
                if let Some(f) = from_file {
                    return f;
                }
            }
            Some(false) => {}
            None => {
                let due = last.map(|t| t.elapsed() > Duration::from_secs(5)).unwrap_or(true);
                if due {
                    self.state_reader.lock().unwrap().1 = Some(Instant::now());
                    if let (Some(f), Some(r)) = (&from_file, self.active_atoms_s6rc().await) {
                        let mut agree = *f == r;
                        if !agree {
                            // A transition may have happened in between: look again.
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            let f2 = self.active_from_state_file(&order);
                            agree = match (f2, self.active_atoms_s6rc().await) {
                                (Some(a), Some(b)) => a == b,
                                _ => true,
                            };
                        }
                        if !agree {
                            eprintln!("sdcompatd: the s6-rc state file layout is not what sdcompat expects; using `s6-rc -a list` (slower)");
                        }
                        self.state_reader.lock().unwrap().0 = Some(agree);
                        return r;
                    }
                }
                if let Some(f) = from_file {
                    return f;
                }
            }
        }
        for _ in 0..30 {
            if let Some(r) = self.active_atoms_s6rc().await {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        from_file.unwrap_or_default()
    }

    pub(super) async fn active_atoms_s6rc(&self) -> Option<BTreeSet<String>> {
        let out = Command::new("s6-rc")
            .arg("-l")
            .arg(&self.cfg.live_dir)
            .args(["-a", "list"])
            .output()
            .await
            .ok()?;
        out.status.success().then(|| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
    }

    /// Startup: run the state-file check once.
    pub(super) async fn verify_state_reader(&self) {
        let _ = self.active_atoms().await;
    }

    /// Atoms contained in a bundle (flattened by s6-rc-db).
    pub(super) async fn bundle_atoms(&self, bundle: &str) -> Vec<String> {
        if let Some(v) = self.bundle_cache.lock().unwrap().get(bundle) {
            return v.clone();
        }
        let v = self.bundle_atoms_uncached(bundle).await;
        self.bundle_cache.lock().unwrap().insert(bundle.to_string(), v.clone());
        v
    }

    async fn bundle_atoms_uncached(&self, bundle: &str) -> Vec<String> {
        let out = Command::new("s6-rc-db")
            .arg("-l")
            .arg(&self.cfg.live_dir)
            .args(["contents", bundle])
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// s6-svstat for status queries, cached for 500 ms (jobs use svstat()).
    pub(super) async fn svstat_cached(&self, atom: &str) -> Option<SvStat> {
        if let Some((t, v)) = self.svstat_cache.lock().unwrap().get(atom) {
            if t.elapsed() < Duration::from_millis(500) {
                return Some(v.clone());
            }
        }
        let v = self.svstat(atom).await?;
        self.svstat_cache.lock().unwrap().insert(atom.to_string(), (Instant::now(), v.clone()));
        Some(v)
    }

    /// A real stop discards the unit's stored fds and its held sockets.
    pub(super) fn drop_stored_fds(&self, id: &str) {
        let store = self.cfg.run_dir.join("fdstore");
        crate::fdstore::del(&store, id, None);
        crate::fdstore::del(&store, &format!("{id}#sockets"), None);
    }

    pub(super) fn forget_unit_state(&self, id: &str) {
        self.svstat_cache.lock().unwrap().remove(id);
    }

    pub(super) async fn svstat(&self, atom: &str) -> Option<SvStat> {
        let out = Command::new("s6-svstat")
            .args(["-o", "up,wantedup,ready,pid"])
            .arg(self.scandir(atom))
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let f: Vec<&str> = text.split_whitespace().collect();
        if f.len() < 4 {
            return None;
        }
        Some(SvStat {
            up: f[0] == "true",
            wanted_up: f[1] == "true",
            ready: f[2] == "true",
            pid: f[3].parse::<i64>().ok().filter(|&p| p > 0).unwrap_or(0) as u32,
        })
    }

    pub(super) fn kind_of(&self, id: &str) -> Option<Kind> {
        self.index.read().unwrap().units.get(id).map(|(k, _)| *k)
    }

    /// Runtime status of a unit, combining s6-rc state, s6-svstat and the
    /// runner's state files.
    pub(super) async fn unit_status(&self, id: &str, active: &BTreeSet<String>, unit: &Unit) -> Status {
        let st = state::for_unit(&self.cfg.run_dir, id);
        let result = st.get(keys::RESULT).unwrap_or_else(|| "success".into());
        let failed_result = !matches!(result.as_str(), "success" | "running");
        let condition_ok = !st.has(keys::CONDITION_FAILED);
        // Soft failures: reported to s6-rc as "up", but failed or never run.
        let dep_failed = st.has(keys::DEPENDENCY_FAILED);
        let start_failed = st.has(keys::START_FAILED);
        let failed_now = failed_result && st.get_u64(keys::INACTIVE_ENTER) >= st.get_u64(keys::INACTIVE_EXIT);
        let result = if start_failed && !failed_result { "exit-code".to_string() } else { result };
        let mut s = Status { active: "inactive", sub: "dead", main_pid: 0, result: result.clone(), condition_ok };

        if let Some(scope) = self.transient.lock().unwrap().get(id) {
            let populated = scope.kind == "slice" || !util::cgroup_pids(&scope.cgroup).is_empty();
            s.active = if populated { "active" } else { "inactive" };
            s.sub = if populated { "running" } else { "dead" };
            return s;
        }

        if let Some(job) = self.jobs.lock().unwrap().values().find(|j| j.unit == id) {
            match job.jtype.as_str() {
                "stop" => (s.active, s.sub) = ("deactivating", "stop"),
                "reload" => {}
                _ => (s.active, s.sub) = ("activating", "start"),
            }
            if job.jtype != "reload" {
                return s;
            }
        }

        if let Some(kind) = self.dynamic_kind(id) {
            if kind == Kind::Oneshot {
                let up = self.dynamic.lock().unwrap().get(id).map(|d| d.active).unwrap_or(false);
                if up {
                    (s.active, s.sub) = ("active", "exited");
                } else if failed_result {
                    (s.active, s.sub) = ("failed", "failed");
                }
                return s;
            }
            let Some(sv) = self.svstat_cached(id).await else { return s };
            if dep_failed || st.has(keys::SOCKET_WAIT) {
                return s;
            }
            if start_failed {
                (s.active, s.sub) = ("failed", "failed");
                return s;
            }
            if sv.up && (id.ends_with(".timer") || id.ends_with(".path")) {
                s.active = "active";
                s.sub = if st.has(crate::timer::ELAPSED) { "elapsed" } else { "waiting" };
                return s;
            }
            if sv.up {
                s.main_pid = st.get(keys::MAIN_PID).and_then(|p| p.parse().ok()).unwrap_or(sv.pid);
                (s.active, s.sub) = if sv.ready { ("active", "running") } else { ("activating", "start") };
            } else if sv.wanted_up {
                (s.active, s.sub) = ("activating", "auto-restart");
            } else if failed_result {
                (s.active, s.sub) = ("failed", "failed");
            }
            return s;
        }

        match self.kind_of(id) {
            None => {
                if failed_result && st.has(keys::INACTIVE_ENTER) {
                    (s.active, s.sub) = ("failed", "failed");
                }
            }
            Some(Kind::Bundle) => {
                let atoms = self.bundle_atoms(id).await;
                let up = if atoms.is_empty() {
                    self.started_targets.lock().unwrap().contains(id) || self.default_active(active)
                } else {
                    atoms.iter().any(|a| active.contains(a))
                };
                if up {
                    (s.active, s.sub) = ("active", "active");
                }
            }
            Some(Kind::Oneshot) if matches!(unit.utype(), "mount" | "swap") => {
                let up = if unit.utype() == "mount" {
                    crate::mountunit::is_mounted(&crate::mountunit::where_of(unit))
                } else {
                    unit.get("Swap", "What").map(|w| crate::mountunit::swap_active(&w)).unwrap_or(false)
                };
                if up {
                    (s.active, s.sub) = ("active", if unit.utype() == "mount" { "mounted" } else { "active" });
                } else if failed_result {
                    (s.active, s.sub) = ("failed", "failed");
                }
            }
            Some(Kind::Oneshot) => {
                let remain = unit.get_bool("Service", "RemainAfterExit", false);
                if dep_failed {
                    // never ran: inactive (systemd's "dependency" result)
                } else if failed_now {
                    (s.active, s.sub) = ("failed", "failed");
                } else if active.contains(id) && remain {
                    (s.active, s.sub) = ("active", "exited");
                }
            }
            Some(Kind::Longrun) if id.ends_with(".timer") || id.ends_with(".path") => {
                if !active.contains(id) || !self.svstat_cached(id).await.map(|s| s.up).unwrap_or(false) {
                    return s;
                }
                s.active = "active";
                s.sub = if st.has(crate::timer::ELAPSED) {
                    "elapsed"
                } else {
                    let target = self.index.read().unwrap().triggers.get(id).cloned();
                    let running = match target {
                        Some(t) => {
                            let (_, tu) = self.load_unit(&t);
                            let ts = Box::pin(self.unit_status(&t, active, &tu)).await;
                            matches!(ts.active, "active" | "activating" | "reloading")
                        }
                        None => false,
                    };
                    if running { "running" } else { "waiting" }
                };
            }
            Some(Kind::Longrun) => {
                if !active.contains(id) {
                    if failed_result && st.has(keys::INACTIVE_ENTER) {
                        (s.active, s.sub) = ("failed", "failed");
                    }
                    return s;
                }
                let Some(sv) = self.svstat_cached(id).await else { return s };
                // Waiting for socket activity: the service itself is not running.
                if !condition_ok || dep_failed || st.has(keys::SOCKET_WAIT) {
                    return s;
                }
                if start_failed {
                    (s.active, s.sub) = ("failed", "failed");
                    return s;
                }
                if sv.up {
                    s.main_pid = st.get(keys::MAIN_PID).and_then(|p| p.parse().ok()).unwrap_or(sv.pid);
                    if sv.ready {
                        let reloading = self.jobs.lock().unwrap().values().any(|j| j.unit == id && j.jtype == "reload");
                        (s.active, s.sub) = if reloading { ("reloading", "reload") } else { ("active", "running") };
                    } else {
                        (s.active, s.sub) = ("activating", "start");
                    }
                } else if sv.wanted_up {
                    (s.active, s.sub) = ("activating", "auto-restart");
                } else if failed_result {
                    (s.active, s.sub) = ("failed", "failed");
                }
            }
        }
        s
    }

    fn default_active(&self, active: &BTreeSet<String>) -> bool {
        !active.is_empty()
    }

    async fn s6rc(&self, args: &[&str], names: &[String]) -> Result<(), String> {
        let _guard = self.s6_lock.lock().await;
        self.s6rc_locked(args, names).await
    }

    async fn s6rc_locked(&self, args: &[&str], names: &[String]) -> Result<(), String> {
        let out = Command::new("s6-rc")
            .arg("-l")
            .arg(&self.cfg.live_dir)
            .arg("-b")
            .args(args)
            .args(names)
            .output()
            .await
            .map_err(|e| format!("s6-rc: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    /// Whether some process (the boot's `s6-rc change`, or one of ours) holds
    /// s6-rc's change lock.
    fn s6rc_lock_held(&self) -> bool {
        lock_held(&self.cfg.live_dir.join("lock"))
    }

    async fn all_dependencies(&self, id: &str) -> Option<Vec<String>> {
        let out = Command::new("s6-rc-db")
            .arg("-l")
            .arg(&self.cfg.live_dir)
            .args(["all-dependencies", id])
            .output()
            .await
            .ok()?;
        out.status.success().then(|| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && l != id)
                .collect()
        })
    }

    /// Start a longrun with s6-svc while another s6-rc change holds the lock.
    /// s6-rc keeps the lock for a whole change, including the wait for each
    /// service's readiness. At boot, upowerd and accounts-daemon (Type=dbus,
    /// in the default bundle) need polkit before they take their bus names,
    /// and polkit is D-Bus activated: its `s6-rc change` waited for the boot's
    /// change, which waited for them, until their 105 s start timeout. When
    /// every dependency is up, s6-rc would do nothing but `s6-svc -u` anyway;
    /// reconcile_beside() records the unit in s6-rc once the lock is free.
    async fn start_beside(&self, id: &str, unit: &Unit) -> Option<&'static str> {
        // rc.shutdown is bringing everything down: do not start anything.
        if self.cfg.run_dir.join("shutting-down").exists() {
            return None;
        }
        if self.s6_lock.try_lock().is_ok() && !self.s6rc_lock_held() {
            return None;
        }
        let deps = self.all_dependencies(id).await?;
        let active = self.active_atoms().await;
        // With JOURNAL=no the service's s6-log is one of its dependencies,
        // and it is down for an on-demand unit. s6-rc would only `s6-svc -u`
        // it too, once its own dependencies (the fd holder) are up.
        let logger = format!("{id}.log");
        let missing: Vec<&String> = deps.iter().filter(|d| !active.contains(*d)).collect();
        let start_logger = match missing.as_slice() {
            [] => false,
            [l] if **l == logger => {
                let ldeps = self.all_dependencies(&logger).await?;
                if !ldeps.iter().all(|d| active.contains(d)) {
                    return None;
                }
                true
            }
            _ => return None,
        };
        eprintln!("sdcompatd: start {id}: s6-rc is busy with another change, starting it beside");
        if start_logger {
            self.beside.lock().unwrap().insert(logger.clone());
            let _ = Command::new("s6-svc").arg("-u").arg(self.scandir(&logger)).status().await;
            if !self.wait_up(&logger, Duration::from_secs(10)).await {
                eprintln!("sdcompatd: start {id}: logger {logger} did not come up");
                return Some("failed");
            }
        }
        self.beside.lock().unwrap().insert(id.to_string());
        let _ = Command::new("s6-svc").arg("-u").arg(self.scandir(id)).status().await;
        Some(self.wait_ready(id, unit, None).await)
    }

    /// Wait until a longrun without a unit (a logger) is up and ready.
    async fn wait_up(&self, atom: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.svstat(atom).await {
                Some(sv) if sv.up && sv.ready => return true,
                Some(_) if Instant::now() < deadline => {}
                _ => return false,
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// start_beside() for each longrun of a bundle. At boot, udev wants
    /// bluetooth.target and sound.target while the boot's change waits for
    /// NetworkManager-wait-online, so their `s6-rc change` started bluetoothd
    /// only once the network was up. True when nothing is left for
    /// `s6-rc change`; otherwise it starts the rest (oneshots, longruns whose
    /// dependencies are not up, native s6 services).
    async fn start_bundle_beside(&self, id: &str) -> bool {
        if self.s6_lock.try_lock().is_ok() && !self.s6rc_lock_held() {
            return false;
        }
        let active = self.active_atoms().await;
        let mut left: Vec<String> = self.bundle_atoms(id).await.into_iter().filter(|a| !active.contains(a)).collect();
        // One member may need another: repeat while something starts.
        loop {
            let before = left.len();
            let mut rest = Vec::new();
            for atom in left {
                let started = match self.kind_of(&atom) {
                    Some(Kind::Longrun) => {
                        let (_, unit) = self.load_unit(&atom);
                        self.start_beside(&atom, &unit).await.is_some()
                    }
                    _ => false,
                };
                if !started {
                    rest.push(atom);
                }
            }
            left = rest;
            if left.is_empty() || left.len() == before {
                return left.is_empty();
            }
        }
    }

    /// Record the longruns start_beside() started as up in s6-rc, once no
    /// other change holds the lock.
    pub(super) async fn reconcile_beside(&self) {
        if self.beside.lock().unwrap().is_empty() || self.s6rc_lock_held() {
            return;
        }
        let _guard = self.s6_lock.lock().await;
        self.reconcile_beside_locked().await;
    }

    /// The caller holds `s6_lock`.
    pub(super) async fn reconcile_beside_locked(&self) {
        let ids: Vec<String> = self.beside.lock().unwrap().iter().cloned().collect();
        for id in ids {
            // Stopped or given up since: s6-rc already considers it down.
            if self.svstat(&id).await.map(|s| s.wanted_up).unwrap_or(false) {
                // The service is up, so this only updates s6-rc's state.
                if let Err(e) = self.s6rc_locked(&["-u", "change"], &[id.clone()]).await {
                    eprintln!("sdcompatd: recording {id} in s6-rc: {e}");
                }
            }
            self.beside.lock().unwrap().remove(&id);
            self.forget_unit_state(&id);
        }
    }

    /// Stop a longrun that start_beside() started and s6-rc does not know
    /// about yet (`s6-rc -d change` would do nothing). False if it is not one.
    async fn stop_beside(&self, id: &str, unit: &Unit) -> bool {
        {
            let _guard = self.s6_lock.lock().await;
            if !self.beside.lock().unwrap().remove(id) {
                return false;
            }
        }
        let usec = unit
            .get("Service", "TimeoutStopSec")
            .or_else(|| unit.get("Service", "TimeoutSec"))
            .and_then(|v| crate::unit::parse_timeout_usec(&v))
            .unwrap_or(90_000_000);
        let ms = (usec / 1000).saturating_add(5000).min(u32::MAX as u64);
        let _ = Command::new("s6-svc")
            .args(["-wD", "-T", &ms.to_string(), "-d"])
            .arg(self.scandir(id))
            .status()
            .await;
        true
    }

    pub(super) async fn run_runner(&self, sub: &str, id: &str, main_pid: Option<u32>) -> bool {
        let mut c = self.runner_cmd(sub);
        if let Some(p) = main_pid {
            c.arg("--mainpid").arg(p.to_string());
        }
        c.arg("--").arg(id);
        match c.status().await {
            Ok(s) => s.success(),
            Err(e) => {
                eprintln!("sdcompatd: {sub} {id}: {e}");
                false
            }
        }
    }

    fn start_timeout(unit: &Unit) -> Duration {
        let usec = unit.start_timeout_usec();
        if usec == u64::MAX {
            Duration::from_secs(86400 * 365)
        } else {
            Duration::from_micros(usec) + Duration::from_secs(1)
        }
    }

    /// Start a service whose runner waits for socket activity, now: the caller
    /// removed SOCKET_WAIT, and SIGUSR1 ends the runner's wait. s6 signals
    /// whatever process runs, so not one that started under a second ago
    /// (it may not block SIGUSR1 yet). Down (s6 restarting it): the next run
    /// sees no SOCKET_WAIT and starts the service.
    async fn wake_socket_wait(&self, id: &str) {
        for _ in 0..20 {
            let out = Command::new("s6-svstat").args(["-o", "up,updownfor"]).arg(self.scandir(id)).output().await;
            let Ok(out) = out else { return };
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let f: Vec<&str> = text.split_whitespace().collect();
            match f.as_slice() {
                ["true", secs] if secs.parse::<u64>().unwrap_or(0) >= 1 => {
                    let _ = Command::new("s6-svc").arg("-1").arg(self.scandir(id)).status().await;
                    return;
                }
                ["true", _] => tokio::time::sleep(Duration::from_millis(200)).await,
                _ => return,
            }
        }
    }

    /// wait_ready() for a service woken from its socket wait: its run process
    /// told s6 it was ready when the wait began, so go by the unit state.
    async fn wait_woken(&self, id: &str, unit: &Unit, started_at: u64) -> &'static str {
        let deadline = Instant::now() + Self::start_timeout(unit);
        let st = state::for_unit(&self.cfg.run_dir, id);
        loop {
            // Started (and maybe exited and waiting again), or skipped or
            // failed to start (job_outcome() tells these apart).
            if st.get_u64(keys::ACTIVE_ENTER) >= started_at
                || st.has(keys::CONDITION_FAILED)
                || st.has(keys::DEPENDENCY_FAILED)
                || st.has(keys::START_FAILED)
            {
                return "done";
            }
            match self.svstat(id).await {
                Some(sv) if !sv.up && !sv.wanted_up => return "failed",
                Some(_) => {}
                None => return "failed",
            }
            if Instant::now() > deadline {
                return "timeout";
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until a longrun is up and ready (and, if `old_pid` is given, has a new PID).
    pub(super) async fn wait_ready(&self, id: &str, unit: &Unit, old_pid: Option<u32>) -> &'static str {
        let started_at = util::now_realtime_usec();
        let deadline = Instant::now() + Self::start_timeout(unit);
        let st = state::for_unit(&self.cfg.run_dir, id);
        loop {
            // Ran and exited already, now waiting for socket activity again.
            if st.has(keys::SOCKET_WAIT) && st.get_u64(keys::ACTIVE_ENTER) >= started_at {
                return "done";
            }
            if let Some(sv) = self.svstat(id).await {
                if sv.up && sv.ready && old_pid.map(|o| o != sv.pid).unwrap_or(true) {
                    return "done";
                }
                if !sv.up && !sv.wanted_up {
                    // Like systemd: a service that became active during this
                    // job and died afterwards still completed its start job.
                    let became_active = st.get_u64(keys::ACTIVE_ENTER) >= started_at;
                    return if st.has(keys::CONDITION_FAILED) || became_active { "done" } else { "failed" };
                }
            } else {
                return "failed";
            }
            if Instant::now() > deadline {
                return "timeout";
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(super) async fn do_start(&self, id: &str, unit: &Unit, isolate: bool) -> &'static str {
        let started_at = util::now_realtime_usec();
        let base = self.do_start_inner(id, unit, isolate, started_at).await;
        self.forget_unit_state(id);
        self.job_outcome(id, started_at, base)
    }

    async fn do_start_inner(&self, id: &str, unit: &Unit, isolate: bool, started_at: u64) -> &'static str {
        self.stop_conflicts(id, unit).await;
        self.repair_requirements(id, unit, 0).await;
        let kind = self.kind_of(id);
        let active = self.active_atoms().await;
        match kind {
            None => "failed",
            Some(Kind::Bundle) => {
                self.started_targets.lock().unwrap().insert(id.to_string());
                if !isolate && self.start_bundle_beside(id).await {
                    return "done";
                }
                let mut names = vec![id.to_string()];
                let mut args = vec!["-u", "change"];
                if isolate {
                    args = vec!["-p", "-u", "change"];
                    let extra = self.cfg.sysinit_bundle.clone();
                    if self.bundle_atoms(&extra).await.len() > 0 || active.contains(&extra) {
                        names.push(extra);
                    }
                    if active.contains("sdcompatd") {
                        names.push("sdcompatd".into());
                    }
                }
                match self.s6rc(&args, &names).await {
                    Ok(()) => "done",
                    Err(e) => {
                        eprintln!("sdcompatd: start {id}: {e}");
                        // Wanted units failing does not fail a target.
                        "done"
                    }
                }
            }
            Some(Kind::Oneshot) => {
                if active.contains(id) {
                    if matches!(unit.utype(), "mount" | "swap") {
                        let st = self.unit_status(id, &active, unit).await;
                        if st.active == "active" {
                            return "done";
                        }
                        return if self.run_runner("up", id, None).await { "done" } else { "failed" };
                    }
                    let st = self.unit_status(id, &active, unit).await;
                    if unit.get_bool("Service", "RemainAfterExit", false) && st.active == "active" {
                        return "done";
                    }
                    // Not active, failed, or skipped for a dependency: run it again.
                    return if self.run_runner("up", id, None).await { "done" } else { "failed" };
                }
                match self.s6rc(&["-u", "change"], &[id.to_string()]).await {
                    Ok(()) => "done",
                    Err(e) => {
                        eprintln!("sdcompatd: start {id}: {e}");
                        self.dependency_or_failed(id, started_at)
                    }
                }
            }
            Some(Kind::Longrun) => {
                // Waiting for socket activity, also when started beside a
                // change and not yet recorded in s6-rc.
                if state::for_unit(&self.cfg.run_dir, id).take(keys::SOCKET_WAIT) {
                    self.wake_socket_wait(id).await;
                    return self.wait_woken(id, unit, started_at).await;
                }
                if active.contains(id) {
                    let sv = self.svstat(id).await.unwrap_or_default();
                    let st = state::for_unit(&self.cfg.run_dir, id);
                    let parked = st.has(keys::CONDITION_FAILED) || st.has(keys::DEPENDENCY_FAILED) || st.has(keys::START_FAILED);
                    if sv.up && parked {
                        // Skipped or failed earlier but still "up" for s6:
                        // restart so conditions and requirements are re-checked.
                        st.set(keys::RESTART_REQUESTED, "1");
                        let _ = Command::new("s6-svc").arg("-r").arg(self.scandir(id)).status().await;
                        return self.wait_ready(id, unit, Some(sv.pid)).await;
                    }
                    if sv.up {
                        return self.wait_ready(id, unit, None).await;
                    }
                    let _ = Command::new("s6-svc").arg("-u").arg(self.scandir(id)).status().await;
                    return self.wait_ready(id, unit, None).await;
                }
                if let Some(r) = self.start_beside(id, unit).await {
                    return r;
                }
                match self.s6rc(&["-u", "change"], &[id.to_string()]).await {
                    Ok(()) => {
                        let st = state::for_unit(&self.cfg.run_dir, id);
                        let became_active = st.get_u64(keys::ACTIVE_ENTER) >= started_at;
                        match self.svstat(id).await {
                            Some(sv) if !sv.up && !sv.wanted_up && !st.has(keys::CONDITION_FAILED) && !became_active => "failed",
                            _ => "done",
                        }
                    }
                    Err(e) => {
                        eprintln!("sdcompatd: start {id}: {e}");
                        self.dependency_or_failed(id, started_at)
                    }
                }
            }
        }
    }

    /// s6-rc reports every start as successful (soft failures); the real
    /// result comes from the runner's state files.
    pub(super) fn job_outcome(&self, id: &str, started_at: u64, base: &'static str) -> &'static str {
        if base != "done" {
            return base;
        }
        let st = state::for_unit(&self.cfg.run_dir, id);
        if st.has(keys::DEPENDENCY_FAILED) {
            return "dependency";
        }
        if st.has(keys::START_FAILED) {
            return "failed";
        }
        let failed = st.get(keys::RESULT).map(|r| !matches!(r.as_str(), "success" | "running")).unwrap_or(false);
        if failed
            && st.get_u64(keys::INACTIVE_EXIT) >= started_at
            && st.get_u64(keys::INACTIVE_ENTER) >= st.get_u64(keys::INACTIVE_EXIT)
        {
            return if st.get(keys::RESULT).as_deref() == Some("timeout") { "timeout" } else { "failed" };
        }
        base
    }

    /// Before starting a unit, retry required units that failed earlier:
    /// s6-rc still considers them "up", so it would not run them again.
    pub(super) async fn repair_requirements(&self, id: &str, unit: &Unit, depth: u32) {
        if depth > 8 {
            return;
        }
        let mut deps = unit.get_words("Unit", "Requires");
        deps.extend(unit.get_words("Unit", "BindsTo"));
        for d in deps {
            let (did, du) = self.load_unit(&d);
            if did == id || self.kind_of(&did).is_none() {
                continue;
            }
            let active = self.active_atoms().await;
            let st = self.unit_status(&did, &active, &du).await;
            let dep_state = state::for_unit(&self.cfg.run_dir, &did);
            if st.active == "failed" || dep_state.has(keys::DEPENDENCY_FAILED) {
                Box::pin(self.repair_requirements(&did, &du, depth + 1)).await;
                Box::pin(self.do_start(&did, &du, false)).await;
            }
        }
    }

    /// After a failed s6-rc start: did the unit itself run and fail, or did
    /// a dependency fail before it was attempted?
    fn dependency_or_failed(&self, id: &str, started_at: u64) -> &'static str {
        let st = state::for_unit(&self.cfg.run_dir, id);
        if st.get_u64(keys::INACTIVE_EXIT) >= started_at {
            "failed"
        } else {
            "dependency"
        }
    }

    pub(super) async fn do_stop(&self, id: &str, unit: &Unit) -> &'static str {
        match self.kind_of(id) {
            None => "done",
            Some(Kind::Bundle) => {
                self.started_targets.lock().unwrap().remove(id);
                "done"
            }
            Some(kind) => {
                let active = self.active_atoms().await;
                if !active.contains(id) {
                    // Mounted outside s6-rc (fstab, by hand): still unmount it.
                    if matches!(unit.utype(), "mount" | "swap") {
                        return if self.run_runner("down", id, None).await { "done" } else { "failed" };
                    }
                    return "done";
                }
                let waiting = state::for_unit(&self.cfg.run_dir, id).has(keys::SOCKET_WAIT);
                if kind == Kind::Longrun && !waiting && !unit.get_all("Service", "ExecStop").is_empty() {
                    let pid = self.svstat(id).await.map(|s| s.pid).filter(|&p| p > 0);
                    self.run_runner("stop-exec", id, pid).await;
                }
                let r = if kind == Kind::Longrun && self.stop_beside(id, unit).await {
                    "done"
                } else {
                    match self.s6rc(&["-d", "change"], &[id.to_string()]).await {
                        Ok(()) => "done",
                        Err(e) => {
                            eprintln!("sdcompatd: stop {id}: {e}");
                            "failed"
                        }
                    }
                };
                // FileDescriptorStorePreserve=restart: a real stop discards
                // stored fds and the held listening sockets.
                self.drop_stored_fds(id);
                self.forget_unit_state(id);
                r
            }
        }
    }

    pub(super) async fn do_restart(&self, id: &str, unit: &Unit, only_if_active: bool) -> &'static str {
        // The new run's result, as do_start() reports it: the runner tells
        // s6 "ready" (and exits 0 for a oneshot) even when the start failed.
        let started_at = util::now_realtime_usec();
        let active = self.active_atoms().await;
        if !only_if_active || active.contains(id) {
            self.stop_conflicts(id, unit).await;
        }
        match self.kind_of(id) {
            Some(Kind::Longrun) if active.contains(id) => {
                let sv = self.svstat(id).await.unwrap_or_default();
                // Waiting for socket activity: inactive, so nothing to stop.
                let waiting = state::for_unit(&self.cfg.run_dir, id).has(keys::SOCKET_WAIT);
                if !sv.up || waiting {
                    if only_if_active {
                        return "done";
                    }
                    return self.do_start(id, unit, false).await;
                }
                if !unit.get_all("Service", "ExecStop").is_empty() {
                    self.run_runner("stop-exec", id, Some(sv.pid)).await;
                }
                state::for_unit(&self.cfg.run_dir, id).set(keys::RESTART_REQUESTED, "1");
                let _ = Command::new("s6-svc").arg("-r").arg(self.scandir(id)).status().await;
                let r = self.wait_ready(id, unit, Some(sv.pid)).await;
                self.job_outcome(id, started_at, r)
            }
            Some(Kind::Oneshot) if active.contains(id) => {
                self.run_runner("down", id, None).await;
                let r = if self.run_runner("up", id, None).await { "done" } else { "failed" };
                self.job_outcome(id, started_at, r)
            }
            _ if only_if_active => "done",
            _ => self.do_start(id, unit, false).await,
        }
    }

    pub(super) async fn do_reload(&self, id: &str) -> &'static str {
        let pid = self.svstat(id).await.map(|s| s.pid).filter(|&p| p > 0);
        if self.run_runner("reload-exec", id, pid).await {
            "done"
        } else {
            "failed"
        }
    }

    pub(super) fn can_reload(unit: &Unit) -> bool {
        !unit.get_all("Service", "ExecReload").is_empty()
            || unit.get("Service", "Type").as_deref() == Some("notify-reload")
    }
}
