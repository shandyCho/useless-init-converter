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
use crate::config::runner_cmd;
use crate::gen::Kind;
use crate::loader::{self, Unit};
use crate::tools;

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
    /// Instances of templates, transient units and the units the removed
    /// AUTO_MASK= left masked (until a reload puts them back) are created
    /// on demand; other units come from the compiled database.
    pub(super) fn can_be_dynamic(&self, id: &str, unit: &Unit) -> bool {
        if !unit.is_loaded() || self.kind_of(id).is_some() {
            return false;
        }
        // Transient units from systemd-run.
        if self.is_transient_file(unit) {
            return matches!(unit.utype(), "service" | "timer" | "path");
        }
        // Masked in the set by the generator, not by the admin.
        if crate::install::set_rx(&self.cfg, id) == Some("masked")
            && uinitconverter_base::automask::is_masked(&self.cfg, id)
        {
            return matches!(unit.utype(), "service" | "timer" | "path")
                && !self.cfg.is_excluded(id);
        }
        unit.utype() == "service"
            && loader::parse_name(id)
                .map(|n| n.instance.is_some())
                .unwrap_or(false)
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
            path.parent()
                .map(fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| fs::write(&path, ""))
        } else {
            match fs::remove_file(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                r => r,
            }
        };
        if let Err(e) = r {
            eprintln!("initconvertd: {}: {e}", path.display());
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
                dynamic.insert(
                    id,
                    Dynamic {
                        kind: Kind::Oneshot,
                        active: true,
                    },
                );
            }
        }
        if let Ok(rd) = fs::read_dir(self.cfg.run_dir.join("dyn")) {
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                // Not linked: a start that failed half-way; dyn_start redoes it.
                if fs::symlink_metadata(self.scandir(&id)).is_ok() {
                    dynamic.entry(id).or_insert(Dynamic {
                        kind: Kind::Longrun,
                        active: false,
                    });
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
        for (name, text, exec) in
            crate::gen::longrun_files(&self.cfg, id, &crate::gen::Timings::of(unit), false)
        {
            write(dir.join(name), &text, exec).map_err(e)?;
        }
        write(dir.join("down"), "", false).map_err(e)?;
        if matches!(unit.utype(), "timer" | "path") {
            return Ok(dir);
        }
        if let Some(log_dir) = self.cfg.log_dir.as_ref().filter(|_| !self.cfg.journal) {
            let ldir = dir.join("log");
            fs::create_dir_all(&ldir).map_err(e)?;
            write(
                ldir.join("run"),
                &crate::gen::logger_run(log_dir, id, false),
                true,
            )
            .map_err(e)?;
        }
        Ok(dir)
    }

    /// Dependencies to start before a unit: (name, required). Also the
    /// .requires/.wants links. A target starts everything it wants; a
    /// service only what it wants and is ordered after.
    fn start_deps(&self, unit: &Unit) -> Vec<(String, bool)> {
        let ld = loader::Loader::new(&self.cfg);
        let after: BTreeSet<String> = unit.get_words("Unit", "After").into_iter().collect();
        let mut out = Vec::new();
        for key in ["Requires", "Requisite", "BindsTo"] {
            for d in unit.get_words("Unit", key) {
                out.push((d, true));
            }
        }
        for d in ld.dir_deps(unit, "requires") {
            out.push((d, true));
        }
        let mut wants = unit.get_words("Unit", "Wants");
        wants.extend(ld.dir_deps(unit, "wants"));
        for d in wants {
            if unit.utype() == "target" || after.contains(&d) {
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

    /// Start a dynamic unit: the on-demand instances start their
    /// dependencies first, one by one.
    pub(super) async fn dyn_start(&self, id: &str, unit: &Unit) -> &'static str {
        for (dep, required) in self.start_deps(unit) {
            if dep == id {
                continue;
            }
            let r = Box::pin(self.start_any(&dep)).await;
            if required && r != "done" {
                eprintln!("initconvertd: {id}: dependency {dep} failed ({r})");
                return "dependency";
            }
        }
        self.dyn_start_one(id, unit).await
    }

    /// A unit's own start, its dependencies already handled.
    pub(super) async fn dyn_start_one(&self, id: &str, unit: &Unit) -> &'static str {
        self.stop_conflicts(id, unit).await;

        let oneshot = unit.utype() == "service"
            && (unit.get("Service", "Type").as_deref() == Some("oneshot")
                || unit.file.get("Service", "ExecStart").is_none());
        if oneshot {
            if self
                .dynamic
                .lock()
                .unwrap()
                .get(id)
                .map(|d| d.active)
                .unwrap_or(false)
            {
                return "done";
            }
            let started_at = crate::util::now_realtime_usec();
            let ok = self.run_runner(runner_cmd::UP, id, None).await
                && self.job_outcome(id, started_at, "done") == "done";
            let remain = unit.get_bool("Service", "RemainAfterExit", false);
            self.set_oneshot_active(id, ok && remain);
            self.dynamic.lock().unwrap().insert(
                id.to_string(),
                Dynamic {
                    kind: Kind::Oneshot,
                    active: ok && remain,
                },
            );
            return if ok { "done" } else { "failed" };
        }

        self.dyn_start_longrun(id, unit).await
    }

    async fn dyn_start_longrun(&self, id: &str, unit: &Unit) -> &'static str {
        let already = self.dynamic.lock().unwrap().contains_key(id);
        if !already {
            let dir = match self.write_servicedir(id, unit) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("initconvertd: {id}: {e}");
                    return "failed";
                }
            };
            let Ok(scan) = fs::canonicalize(self.cfg.live_dir.join("scandir")) else {
                eprintln!("initconvertd: {id}: no s6 scan directory");
                return "failed";
            };
            let st = Command::new(tools::path("s6-svlink"))
                .args(["-d", "-t", "5000"])
                .arg(&scan)
                .arg(&dir)
                .arg(id)
                .status()
                .await;
            if !st.map(|s| s.success()).unwrap_or(false) {
                eprintln!("initconvertd: {id}: s6-svlink failed");
                // Never an instance, so no stop would remove the directory.
                // Kept if the link is still there: a dangling entry makes
                // s6-svscan abort every scan.
                if fs::symlink_metadata(scan.join(id)).is_err() {
                    let _ = fs::remove_dir_all(&dir);
                }
                return "failed";
            }
            self.dynamic.lock().unwrap().insert(
                id.to_string(),
                Dynamic {
                    kind: Kind::Longrun,
                    active: false,
                },
            );
        }
        if already {
            // As do_start for the database's longruns: skipped or failed
            // earlier but still up for s6, so restart it and the runner
            // checks conditions and requirements again.
            let st = crate::state::for_unit(&self.cfg.run_dir, id);
            let parked = st.has(crate::state::keys::CONDITION_FAILED)
                || st.has(crate::state::keys::DEPENDENCY_FAILED)
                || st.has(crate::state::keys::START_FAILED)
                || st.has(crate::state::keys::PARKED);
            let sv = self.svstat(id).await.unwrap_or_default();
            if sv.up && parked {
                let started_at = crate::util::now_realtime_usec();
                let r = self.restart_longrun(id, unit, sv.pid).await;
                return self.job_outcome(id, started_at, r);
            }
            // Waiting for its socket's first client: start it now.
            if sv.up && st.take(crate::state::keys::SOCKET_WAIT) {
                let started_at = crate::util::now_realtime_usec();
                self.wake_socket_wait(id).await;
                let r = self.wait_woken(id, unit, started_at).await;
                return self.job_outcome(id, started_at, r);
            }
        }
        // `s6-svlink -d` also writes log/down: the s6-log of a JOURNAL=no
        // install would never run, and the service would write into a pipe
        // nobody reads.
        let log = self.scandir(id).join("log");
        if log.is_dir() {
            let _ = Command::new(tools::path("s6-svc"))
                .arg("-u")
                .arg(&log)
                .status()
                .await;
        }
        let started_at = crate::util::now_realtime_usec();
        self.svc_up(&self.scandir(id)).await;
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
                    .any(|k| u.deps(k).iter().any(|d| self.load_unit(d).0 == id))
            })
            .collect()
    }

    pub(super) async fn dyn_stop(&self, id: &str, unit: &Unit) -> &'static str {
        let t = std::time::Instant::now();
        let r = Box::pin(self.dyn_stop_seen(id, unit, &mut BTreeSet::new())).await;
        if t.elapsed() >= std::time::Duration::from_secs(1) {
            eprintln!(
                "initconvertd: stopping {id} took {:.1} s",
                t.elapsed().as_secs_f64()
            );
        }
        r
    }

    /// dyn_stop(), dependents first. `seen`: the units this stop has taken
    /// on; units that need each other (gnome-session-pre.target and
    /// gnome-session-initialized.target, Requires= one way and Requisite=
    /// the other) recursed without it until the stack overflowed.
    async fn dyn_stop_seen(
        &self,
        id: &str,
        unit: &Unit,
        seen: &mut BTreeSet<String>,
    ) -> &'static str {
        seen.insert(id.to_string());
        for dep in self.dyn_dependents(id) {
            if seen.contains(&dep) {
                continue;
            }
            let (_, du) = self.load_unit(&dep);
            Box::pin(self.dyn_stop_seen(&dep, &du, seen)).await;
        }
        self.dyn_stop_alone(id, unit).await
    }

    /// Stop `id` itself: its dependents are the caller's.
    pub(super) async fn dyn_stop_alone(&self, id: &str, unit: &Unit) -> &'static str {
        self.dyn_remove(id, unit).await
    }

    /// dyn_stop() of `id` alone: its dependents stay.
    async fn dyn_remove(&self, id: &str, unit: &Unit) -> &'static str {
        let Some(kind) = self.dynamic_kind(id) else {
            return "done";
        };
        if kind == Kind::Oneshot {
            let active = self
                .dynamic
                .lock()
                .unwrap()
                .get(id)
                .map(|d| d.active)
                .unwrap_or(false);
            if active {
                self.run_runner(runner_cmd::DOWN, id, None).await;
            }
            self.set_oneshot_active(id, false);
            self.dynamic.lock().unwrap().remove(id);
            return "done";
        }
        let sv = self.svstat(id).await.unwrap_or_default();
        if sv.up && !unit.get_all("Service", "ExecStop").is_empty() {
            self.run_runner(runner_cmd::STOP_EXEC, id, Some(sv.pid))
                .await;
        }
        let svdir = self.scandir(id);
        let _ = tokio::join!(
            self.svc_down_wait(&svdir, Duration::from_secs(120)),
            self.signal_rest_at_stop(id, unit, true)
        );
        if let Ok(scan) = fs::canonicalize(self.cfg.live_dir.join("scandir")) {
            // What s6-svunlink does, without its processes: the link goes,
            // s6-svscan is told to rescan (`an`), and the wait for its
            // supervisors to exit is the lock poll below. The link must go
            // before the directory: a dangling entry makes s6-svscan abort
            // every scan, and then no supervisor is ended.
            let link = scan.join(id);
            match fs::remove_file(&link) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!(
                        "initconvertd: {id}: {}: {e}; keeping its service directory",
                        link.display()
                    );
                    self.drop_stored_fds(id);
                    return "done";
                }
            }
            use crate::util::{s6_control, CtlWrite};
            match s6_control(&scan.join(".s6-svscan/control"), b"an") {
                CtlWrite::Written | CtlWrite::NoReader => {}
                CtlWrite::Missing | CtlWrite::Failed => {
                    let _ = Command::new(tools::path("s6-svscanctl"))
                        .arg("-an")
                        .arg(&scan)
                        .status()
                        .await;
                }
            }
        }
        self.drop_stored_fds(id);
        // Wait for s6-supervise (and its logger's) to exit before removing
        // its directory, as long as it takes, up to 5 s (s6-svunlink's
        // limit before, part 187: s6-svscan doesn't end the supervisors
        // while an entry of the scan directory can't be stat'ed; without a
        // limit this stop, and every job behind it, hung).
        let dir = self.dyn_dir(id);
        for _ in 0..500 {
            if !super::s6::lock_held(&dir.join("supervise/lock"))
                && !super::s6::lock_held(&dir.join("log/supervise/lock"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if super::s6::lock_held(&dir.join("supervise/lock")) {
            eprintln!("initconvertd: {id}: s6-svscan did not release its supervisor (look for its warnings)");
        } else if super::s6::lock_held(&dir.join("log/supervise/lock")) {
            eprintln!("initconvertd: {id}: its logger is still running (a leftover process may hold its output)");
        }
        let _ = fs::remove_dir_all(&dir);
        self.dynamic.lock().unwrap().remove(id);
        "done"
    }

    /// A longrun instance whose service ended on its own (Restart= done
    /// with it) leaves its s6-supervise, and its logger's, behind until a
    /// stop: remove them once it has been down for `after` µs. Its
    /// dependents stay, as systemd keeps Requires= dependents of a unit
    /// that exited, and the state files keep its result for `status`; the
    /// next start creates it again. Skipped while a job holds the unit.
    pub(super) async fn collect_exited(&self, id: &str, unit: &Unit, after: u64) -> bool {
        let down_since = |sv: &super::s6::SvStat| {
            let since = crate::state::for_unit(&self.cfg.run_dir, id)
                .get_u64(crate::state::keys::INACTIVE_ENTER);
            !sv.up
                && !sv.wanted_up
                && since > 0
                && crate::util::now_realtime_usec().saturating_sub(since) > after
        };
        if unit.utype() != "service" || self.dynamic_kind(id) != Some(Kind::Longrun) {
            return false;
        }
        if !self.svstat(id).await.is_some_and(|sv| down_since(&sv)) {
            return false;
        }
        let Ok(_guard) = self.unit_lock(id).try_lock_owned() else {
            return false;
        };
        // Again under the lock: a start may have run in between.
        if !self.svstat(id).await.is_some_and(|sv| down_since(&sv)) {
            return false;
        }
        self.dyn_remove(id, unit).await;
        true
    }

    /// Shutdown: s6-rc doesn't know the on-demand instances, so run their
    /// ExecStop= here, as systemd stops every unit (systemd-backlight@ saves
    /// the brightness then): active oneshots, and longruns that have one
    /// (the others just get the final kill). rc.shutdown asks for this first
    /// (StopInstances), while logind and the bus still run; the SIGTERM from
    /// s6-rc runs it again for what is left. Instances started by an earlier
    /// daemon come from load_dynamic().
    pub(super) async fn stop_dynamic_at_shutdown(&self) {
        self.stop_scopes_at_shutdown().await;
        let ids: Vec<String> = self
            .dynamic
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, d)| d.kind == Kind::Longrun || d.active)
            .map(|(n, d)| (n.clone(), d.kind))
            .filter(|(n, kind)| {
                *kind != Kind::Longrun
                    || !self
                        .load_unit(n)
                        .1
                        .get_all("Service", "ExecStop")
                        .is_empty()
            })
            .map(|(n, _)| n)
            .collect();
        // Groups that share no dependent stop at once, as systemd stops every
        // user at once (one at a time, each user@ waited for the one before:
        // three users took 8-11 s at a reboot, work_history part 154). Within
        // a group one at a time as before: dyn_stop stops the dependents
        // first, and two stops of one dependent side by side would run its
        // ExecStop= twice.
        let groups = shutdown_groups(ids, |id| self.dyn_dependents_closure(id));
        futures_util::future::join_all(groups.iter().map(|g| async move {
            for id in g {
                let (_, unit) = self.load_unit(id);
                // The bus is still served: a StopUnit for the same unit must
                // not run its ExecStop= a second time beside us.
                let lock = self.unit_lock(id);
                let _guard = lock.lock().await;
                // dyn_stop logs a slow stop (dependents like user@ included).
                self.dyn_stop(id, &unit).await;
            }
        }))
        .await;
    }

    /// `id` and the dynamic units that require it, directly or not.
    fn dyn_dependents_closure(&self, id: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::from([id.to_string()]);
        let mut todo = vec![id.to_string()];
        while let Some(n) = todo.pop() {
            for d in self.dyn_dependents(&n) {
                if seen.insert(d.clone()) {
                    todo.push(d);
                }
            }
        }
        seen
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
                    self.run_runner(runner_cmd::STOP_EXEC, id, Some(sv.pid))
                        .await;
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

    /// Stop units listed in Conflicts= before starting `id`, and those that
    /// list `id` in theirs (systemd's ConflictedBy=).
    pub(super) async fn stop_conflicts(&self, id: &str, unit: &Unit) {
        let mut names = unit.get_words("Unit", "Conflicts");
        names.extend(self.reverse(id, "Conflicts").into_iter().map(|(n, _)| n));
        for c in names {
            let (cid, cu) = self.load_unit(&c);
            if cid == id {
                continue;
            }
            let active = self.active_atoms().await;
            let st = self.unit_status(&cid, &active, &cu).await;
            if matches!(st.active, "active" | "activating" | "reloading") {
                eprintln!("initconvertd: stopping {cid} (conflicts with {id})");
                let r = if self.dynamic_kind(&cid).is_some() {
                    Box::pin(self.dyn_stop(&cid, &cu)).await
                } else {
                    Box::pin(self.do_stop(&cid, &cu)).await
                };
                // A stop like any other: PartOf=/BindsTo= units follow (the
                // GNOME session's logout is exactly this).
                if r == "done" {
                    Box::pin(self.propagate(
                        &cid,
                        "stop",
                        &mut std::collections::BTreeSet::from([cid.clone(), id.to_string()]),
                    ))
                    .await;
                }
            }
        }
    }
}

/// Split `ids` into groups whose units (by `closure`: a unit and its
/// dependents) don't overlap; the order of `ids` is kept within a group.
fn shutdown_groups(
    ids: Vec<String>,
    closure: impl Fn(&str) -> BTreeSet<String>,
) -> Vec<Vec<String>> {
    let mut groups: Vec<(BTreeSet<String>, Vec<String>)> = Vec::new();
    for id in ids {
        let mut units = closure(&id);
        let mut members = vec![id];
        // Every group this one touches joins it.
        let mut k = 0;
        while k < groups.len() {
            if groups[k].0.is_disjoint(&units) {
                k += 1;
            } else {
                let (u, m) = groups.remove(k);
                units.extend(u);
                members.splice(0..0, m);
            }
        }
        groups.push((units, members));
    }
    groups.into_iter().map(|(_, m)| m).collect()
}

#[cfg(test)]
mod shutdown_tests {
    use super::shutdown_groups;
    use std::collections::BTreeSet;

    fn closure(id: &str) -> BTreeSet<String> {
        // user-runtime-dir@UID is required by user@UID; a.service and
        // b.service share dep.service.
        let mut s = BTreeSet::from([id.to_string()]);
        if let Some(uid) = id.strip_prefix("user-runtime-dir@") {
            s.insert(format!("user@{uid}"));
        }
        if id == "a.service" || id == "b.service" {
            s.insert("dep.service".into());
        }
        s
    }

    #[test]
    fn users_stop_apart_and_shared_dependents_together() {
        let ids = [
            "user-runtime-dir@1000.service",
            "a.service",
            "user-runtime-dir@1001.service",
            "b.service",
        ]
        .map(String::from)
        .to_vec();
        let g = shutdown_groups(ids, closure);
        assert_eq!(
            g,
            vec![
                vec!["user-runtime-dir@1000.service".to_string()],
                vec!["user-runtime-dir@1001.service".to_string()],
                vec!["a.service".to_string(), "b.service".to_string()],
            ]
        );
    }
}
