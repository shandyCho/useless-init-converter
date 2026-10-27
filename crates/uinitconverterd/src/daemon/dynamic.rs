//! On-demand template instances (user@1000.service, user-runtime-dir@1000.service,
//! getty@tty3.service, ...) that are not in the compiled s6-rc database.
//!
//! Longruns get a service directory under RUN_DIR/dyn linked into the s6
//! scan directory (s6-svlink), with a log/ subdirectory for s6-log. Oneshots
//! are run directly. Their dependencies are started first, and units that
//! require a stopped unit are stopped with it.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use tokio::process::Command;

use super::Daemon;
use crate::gen::Kind;
use crate::loader::{self, Unit};
use crate::util;

pub struct Dynamic {
    pub kind: Kind,
    /// Oneshot with RemainAfterExit=yes that is currently up.
    pub active: bool,
}

fn write(path: PathBuf, content: &str, exec: bool) -> std::io::Result<()> {
    fs::write(&path, content)?;
    if exec {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

impl Daemon {
    /// Only instances of templates are created on demand; other units come
    /// from the compiled database.
    pub(super) fn can_be_dynamic(&self, id: &str, unit: &Unit) -> bool {
        if !unit.is_loaded() || self.kind_of(id).is_some() {
            return false;
        }
        // Transient units from systemd-run.
        if self.is_transient_file(unit) {
            return matches!(unit.utype(), "service" | "timer" | "path");
        }
        unit.utype() == "service"
            && loader::parse_name(id).map(|n| n.instance.is_some()).unwrap_or(false)
            && !self.cfg.is_excluded(id)
    }

    pub(super) fn dynamic_kind(&self, id: &str) -> Option<Kind> {
        self.dynamic.lock().unwrap().get(id).map(|d| d.kind)
    }

    /// Active oneshot instances (RemainAfterExit=yes) are recorded here, so a
    /// restarted daemon still reports them active and runs their ExecStop=
    /// at shutdown (systemd-backlight@ saves the brightness then). RUN_DIR
    /// is fresh at boot.
    fn oneshot_marker(&self, id: &str) -> PathBuf {
        self.cfg.run_dir.join("dyn-oneshot").join(id)
    }

    fn set_oneshot_active(&self, id: &str, active: bool) {
        let path = self.oneshot_marker(id);
        let r = if active {
            path.parent().map(fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| fs::write(&path, ""))
        } else {
            match fs::remove_file(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                r => r,
            }
        };
        if let Err(e) = r {
            eprintln!("uinitconverterd: {}: {e}", path.display());
        }
    }

    /// Startup: take over the on-demand units of a previous daemon. Active
    /// oneshots come from their markers. Longruns (user@UID, systemd-run
    /// services, timers, paths) kept running under s6: their service
    /// directory in RUN_DIR/dyn is linked into the scan directory from
    /// dyn_start until dyn_stop removes both.
    pub(super) fn load_dynamic(&self) {
        let mut dynamic = self.dynamic.lock().unwrap();
        if let Ok(rd) = fs::read_dir(self.cfg.run_dir.join("dyn-oneshot")) {
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                dynamic.insert(id, Dynamic { kind: Kind::Oneshot, active: true });
            }
        }
        if let Ok(rd) = fs::read_dir(self.cfg.run_dir.join("dyn")) {
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                // Not linked: a start that failed half-way; dyn_start redoes it.
                if fs::symlink_metadata(self.scandir(&id)).is_ok() {
                    dynamic.entry(id).or_insert(Dynamic { kind: Kind::Longrun, active: false });
                }
            }
        }
    }

    fn dyn_dir(&self, id: &str) -> PathBuf {
        self.cfg.run_dir.join("dyn").join(id)
    }

    fn write_servicedir(&self, id: &str, unit: &Unit) -> Result<PathBuf, String> {
        let dir = self.dyn_dir(id);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let e = |e: std::io::Error| e.to_string();
        // The same scripts and timings as the database's longruns (gen);
        // `down`: s6-svlink -d links it down, dyn_start brings it up.
        for (name, text, exec) in crate::gen::longrun_files(&self.cfg, id, &crate::gen::Timings::of(unit), false) {
            write(dir.join(name), &text, exec).map_err(e)?;
        }
        write(dir.join("down"), "", false).map_err(e)?;
        if matches!(unit.utype(), "timer" | "path") {
            return Ok(dir);
        }
        if let Some(log_dir) = self.cfg.log_dir.as_ref().filter(|_| !self.cfg.journal) {
            let ldir = dir.join("log");
            fs::create_dir_all(&ldir).map_err(e)?;
            let logdir = util::sh_quote(&log_dir.join(id).display().to_string());
            write(
                ldir.join("run"),
                &format!("#!/bin/sh\nmkdir -p {logdir} || exit 1\nexec s6-log n10 s1000000 T {logdir}\n"),
                true,
            )
            .map_err(e)?;
        }
        Ok(dir)
    }

    /// Dependencies to start before a unit: (name, required).
    fn start_deps(unit: &Unit) -> Vec<(String, bool)> {
        let after: BTreeSet<String> = unit.get_words("Unit", "After").into_iter().collect();
        let mut out = Vec::new();
        for key in ["Requires", "Requisite", "BindsTo"] {
            for d in unit.get_words("Unit", key) {
                out.push((d, true));
            }
        }
        for d in unit.get_words("Unit", "Wants") {
            if after.contains(&d) {
                out.push((d, false));
            }
        }
        out
    }

    /// Start any unit: database, dynamic instance, or transient.
    ///
    /// Takes the unit's job lock: a dependency start must not run beside a
    /// D-Bus job on the same unit. logind starts user-runtime-dir@UID and
    /// user@UID (BindsTo= it) back to back, and without the lock both ran
    /// `systemd-user-runtime-dir start`, mounting /run/user/UID twice.
    pub(super) async fn start_any(&self, name: &str) -> &'static str {
        let (id, unit) = self.load_unit(name);
        if self.transient.lock().unwrap().contains_key(&id) {
            return "done";
        }
        let lock = self.unit_lock(&id);
        let _guard = lock.lock().await;
        if self.kind_of(&id).is_some() {
            let active = self.active_atoms().await;
            if self.unit_status(&id, &active, &unit).await.active == "active" {
                return "done";
            }
            return self.do_start(&id, &unit, false).await;
        }
        if self.can_be_dynamic(&id, &unit) {
            return Box::pin(self.dyn_start(&id, &unit)).await;
        }
        // Not loadable, excluded, or a type we do not run: treat as satisfied,
        // as the generator does for dropped dependencies.
        "done"
    }

    pub(super) async fn dyn_start(&self, id: &str, unit: &Unit) -> &'static str {
        for (dep, required) in Self::start_deps(unit) {
            if dep == id {
                continue;
            }
            let r = Box::pin(self.start_any(&dep)).await;
            if required && r != "done" {
                eprintln!("uinitconverterd: {id}: dependency {dep} failed ({r})");
                return "dependency";
            }
        }
        self.stop_conflicts(id, unit).await;

        let oneshot = unit.utype() == "service"
            && (unit.get("Service", "Type").as_deref() == Some("oneshot")
                || unit.file.get("Service", "ExecStart").is_none());
        if oneshot {
            if self.dynamic.lock().unwrap().get(id).map(|d| d.active).unwrap_or(false) {
                return "done";
            }
            let started_at = crate::util::now_realtime_usec();
            let ok = self.run_runner("up", id, None).await
                && self.job_outcome(id, started_at, "done") == "done";
            let remain = unit.get_bool("Service", "RemainAfterExit", false);
            self.set_oneshot_active(id, ok && remain);
            self.dynamic
                .lock()
                .unwrap()
                .insert(id.to_string(), Dynamic { kind: Kind::Oneshot, active: ok && remain });
            return if ok { "done" } else { "failed" };
        }

        let already = self.dynamic.lock().unwrap().contains_key(id);
        if !already {
            let dir = match self.write_servicedir(id, unit) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("uinitconverterd: {id}: {e}");
                    return "failed";
                }
            };
            let Ok(scan) = fs::canonicalize(self.cfg.live_dir.join("scandir")) else {
                eprintln!("uinitconverterd: {id}: no s6 scan directory");
                return "failed";
            };
            let st = Command::new("s6-svlink")
                .args(["-d", "-t", "5000"])
                .arg(&scan)
                .arg(&dir)
                .arg(id)
                .status()
                .await;
            if !st.map(|s| s.success()).unwrap_or(false) {
                eprintln!("uinitconverterd: {id}: s6-svlink failed");
                return "failed";
            }
            self.dynamic
                .lock()
                .unwrap()
                .insert(id.to_string(), Dynamic { kind: Kind::Longrun, active: false });
        }
        // `s6-svlink -d` also writes log/down: the s6-log of a JOURNAL=no
        // install would never run, and the service would write into a pipe
        // nobody reads.
        let log = self.scandir(id).join("log");
        if log.is_dir() {
            let _ = Command::new("s6-svc").arg("-u").arg(&log).status().await;
        }
        let started_at = crate::util::now_realtime_usec();
        let _ = Command::new("s6-svc").arg("-u").arg(self.scandir(id)).status().await;
        let r = self.wait_ready(id, unit, None).await;
        self.job_outcome(id, started_at, r)
    }

    /// Dynamic units that require (Requires/BindsTo) `id`.
    fn dyn_dependents(&self, id: &str) -> Vec<String> {
        let names: Vec<String> = self.dynamic.lock().unwrap().keys().cloned().collect();
        names
            .into_iter()
            .filter(|n| n != id)
            .filter(|n| {
                let (_, u) = self.load_unit(n);
                ["Requires", "BindsTo", "Requisite"]
                    .iter()
                    .any(|k| u.get_words("Unit", k).iter().any(|d| d == id))
            })
            .collect()
    }

    pub(super) async fn dyn_stop(&self, id: &str, unit: &Unit) -> &'static str {
        for dep in self.dyn_dependents(id) {
            let (_, du) = self.load_unit(&dep);
            Box::pin(self.dyn_stop(&dep, &du)).await;
        }
        let Some(kind) = self.dynamic_kind(id) else { return "done" };
        if kind == Kind::Oneshot {
            let active = self.dynamic.lock().unwrap().get(id).map(|d| d.active).unwrap_or(false);
            if active {
                self.run_runner("down", id, None).await;
            }
            self.set_oneshot_active(id, false);
            self.dynamic.lock().unwrap().remove(id);
            return "done";
        }
        let sv = self.svstat(id).await.unwrap_or_default();
        if sv.up && !unit.get_all("Service", "ExecStop").is_empty() {
            self.run_runner("stop-exec", id, Some(sv.pid)).await;
        }
        let _ = Command::new("s6-svc").args(["-wD", "-d", "-T", "120000"]).arg(self.scandir(id)).status().await;
        if let Ok(scan) = fs::canonicalize(self.cfg.live_dir.join("scandir")) {
            // s6-svunlink waits for s6-svscan to end the supervisors, which
            // it doesn't while any entry in the scan directory can't be
            // stat'ed (it aborts the scan). Without a limit this stop, and
            // every job behind it, hung: logind's user@0 stop, then sudo.
            // It exits 0 when the limit passes, too.
            let st = Command::new("s6-svunlink").args(["-t", "5000"]).arg(&scan).arg(id).status().await;
            let link = scan.join(id);
            if !st.map(|s| s.success()).unwrap_or(false) && fs::symlink_metadata(&link).is_ok() {
                // It failed before removing the link (it couldn't check the
                // supervisors). Removing the directory under it would leave
                // a dangling entry, on which s6-svscan aborts every scan.
                eprintln!("uinitconverterd: {id}: s6-svunlink failed, removing {}", link.display());
                if let Err(e) = fs::remove_file(&link) {
                    eprintln!("uinitconverterd: {id}: {}: {e}; keeping its service directory", link.display());
                    self.drop_stored_fds(id);
                    return "done";
                }
                let _ = Command::new("s6-svscanctl").arg("-an").arg(&scan).status().await;
            }
        }
        self.drop_stored_fds(id);
        // Give s6-supervise a moment to exit before removing its directory.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let dir = self.dyn_dir(id);
        if super::s6::lock_held(&dir.join("supervise/lock")) {
            eprintln!("uinitconverterd: {id}: s6-svscan did not release its supervisor (look for its warnings)");
        } else if super::s6::lock_held(&dir.join("log/supervise/lock")) {
            eprintln!("uinitconverterd: {id}: its logger is still running (a leftover process may hold its output)");
        }
        let _ = fs::remove_dir_all(&dir);
        self.dynamic.lock().unwrap().remove(id);
        "done"
    }

    /// Shutdown: s6-rc doesn't know the on-demand instances, so run their
    /// ExecStop= here, as systemd stops every unit (systemd-backlight@ saves
    /// the brightness then): active oneshots, and longruns that have one
    /// (the others just get the final kill). rc.shutdown asks for this first
    /// (StopInstances), while logind and the bus still run; the SIGTERM from
    /// s6-rc runs it again for what is left. Instances started by an earlier
    /// daemon come from load_dynamic().
    pub(super) async fn stop_dynamic_at_shutdown(&self) {
        let ids: Vec<(String, Kind)> = self
            .dynamic
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, d)| d.kind == Kind::Longrun || d.active)
            .map(|(n, d)| (n.clone(), d.kind))
            .collect();
        for (id, kind) in ids {
            let (_, unit) = self.load_unit(&id);
            if kind == Kind::Longrun && unit.get_all("Service", "ExecStop").is_empty() {
                continue;
            }
            // The bus is still served: a StopUnit for the same unit must not
            // run its ExecStop= a second time beside us.
            let lock = self.unit_lock(&id);
            let _guard = lock.lock().await;
            let t = std::time::Instant::now();
            self.dyn_stop(&id, &unit).await;
            // Says where a slow power-off waited (dependents like user@ included).
            let took = t.elapsed();
            if took >= Duration::from_secs(2) {
                eprintln!("uinitconverterd: stopping {id} took {:.1} s", took.as_secs_f64());
            }
        }
    }

    pub(super) async fn dyn_restart(&self, id: &str, unit: &Unit) -> &'static str {
        self.stop_conflicts(id, unit).await;
        match self.dynamic_kind(id) {
            Some(Kind::Longrun) => {
                let sv = self.svstat(id).await.unwrap_or_default();
                if !sv.up {
                    return self.dyn_start(id, unit).await;
                }
                if !unit.get_all("Service", "ExecStop").is_empty() {
                    self.run_runner("stop-exec", id, Some(sv.pid)).await;
                }
                let started_at = crate::util::now_realtime_usec();
                let r = self.restart_longrun(id, unit, sv.pid).await;
                self.job_outcome(id, started_at, r)
            }
            Some(_) => {
                self.dyn_stop(id, unit).await;
                self.dyn_start(id, unit).await
            }
            None => self.dyn_start(id, unit).await,
        }
    }

    /// Stop units listed in Conflicts= before starting `id`.
    pub(super) async fn stop_conflicts(&self, id: &str, unit: &Unit) {
        for c in unit.get_words("Unit", "Conflicts") {
            let (cid, cu) = self.load_unit(&c);
            if cid == id {
                continue;
            }
            let active = self.active_atoms().await;
            let st = self.unit_status(&cid, &active, &cu).await;
            if matches!(st.active, "active" | "activating" | "reloading") {
                eprintln!("uinitconverterd: stopping {cid} (conflicts with {id})");
                if self.dynamic_kind(&cid).is_some() {
                    Box::pin(self.dyn_stop(&cid, &cu)).await;
                } else {
                    Box::pin(self.do_stop(&cid, &cu)).await;
                }
            }
        }
    }
}
