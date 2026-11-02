//! Jobs: checking a start/stop/reload request, running it, and reporting its result.

use super::*;

/// Apply a Markers= assignment with systemd's syntax: "+m"/"-m" add or
/// remove single markers, a plain list replaces them all.
pub(super) fn apply_markers(
    mut cur: BTreeSet<String>,
    words: &[String],
) -> Result<BTreeSet<String>, String> {
    let relative = words.iter().any(|w| w.starts_with(['+', '-']));
    if relative && words.iter().any(|w| !w.starts_with(['+', '-'])) {
        return Err("Bad marker syntax.".into());
    }
    if !relative {
        cur.clear();
    }
    for w in words {
        let m = w.trim_start_matches(['+', '-']);
        if !matches!(m, "needs-reload" | "needs-restart") {
            return Err(format!("Unknown marker \"{m}\"."));
        }
        if w.starts_with('-') {
            cur.remove(m);
        } else {
            cur.insert(m.to_string());
        }
    }
    Ok(cur)
}

impl Daemon {
    /// The locks of all `ids`, taken without waiting while holding some: a
    /// job holding one may be starting a dependency whose lock we'd hold.
    pub(super) async fn lock_units(
        &self,
        ids: &BTreeSet<String>,
    ) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        loop {
            let guards: Vec<_> = ids
                .iter()
                .map_while(|id| self.unit_lock(id).try_lock_owned().ok())
                .collect();
            if guards.len() == ids.len() {
                return guards;
            }
            drop(guards);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Dynamic instances that are up: longruns whose supervisor says so,
    /// active oneshots.
    pub(super) async fn active_dynamic(&self) -> BTreeSet<String> {
        let all: Vec<(String, Kind, bool)> = self
            .dynamic
            .lock()
            .unwrap()
            .iter()
            .map(|(n, d)| (n.clone(), d.kind, d.active))
            .collect();
        let mut up = BTreeSet::new();
        for (id, kind, active) in all {
            let on = if kind == Kind::Oneshot {
                active
            } else {
                self.svstat(&id).await.map(|s| s.up).unwrap_or(false)
            };
            if on {
                up.insert(id);
            }
        }
        up
    }

    pub(super) fn new_job(&self, unit: &str, jtype: &str) -> u32 {
        let id = self.next_job.fetch_add(1, Ordering::SeqCst) + 1;
        self.jobs.lock().unwrap().insert(
            id,
            Job {
                id,
                unit: unit.into(),
                jtype: jtype.into(),
                state: "waiting",
                canceled: false,
            },
        );
        id
    }

    /// Validate a job request; returns the canonical unit id. `manual`: asked
    /// for by a client, so RefuseManualStart=/RefuseManualStop= apply.
    pub(super) async fn check_job(
        &self,
        name: &str,
        jtype: &str,
        allow_reload: bool,
        manual: bool,
    ) -> R<(String, Unit)> {
        let (id, unit) = self.load_unit(name);
        if jtype == "start" && shutdown_action(&id).is_some() {
            return Ok((id, unit));
        }
        match unit.load_state {
            LoadState::NotFound => return Err(err(E_NO_UNIT, format!("Unit {name} not found."))),
            LoadState::Masked => return Err(err(E_MASKED, format!("Unit {name} is masked."))),
            LoadState::BadSetting => {
                return Err(err(
                    E_LOAD_FAILED,
                    unit.load_error.clone().unwrap_or_default(),
                ))
            }
            LoadState::Loaded => {}
        }
        let starts = matches!(jtype, "start" | "restart" | "reload-or-restart");
        // rc.shutdown's `s6-rc change` holds the s6-rc lock until everything
        // is down, so a start would wait for it; a D-Bus caller (wireplumber
        // stopping in the user manager) then hung for the bus's 25 s
        // activation timeout and delayed the shutdown. systemd refuses here.
        if starts && self.cfg.run_dir.join("shutting-down").exists() {
            return Err(err(
                E_DESTRUCTIVE,
                format!(
                    "Transaction for {id}/{jtype} is destructive (the system is shutting down)."
                ),
            ));
        }
        if starts && self.kind_of(&id).is_none() && !self.cfg.is_excluded(&id) {
            if let Some(svc) = socket_service(&id, &unit) {
                // Sockets are bound by their service's runner: record this
                // one so the runner binds it too, then start the service.
                if let Err(e) = crate::socket::mark_requested(&self.cfg.run_dir, &id) {
                    eprintln!("uinitconverterd: {id}: {e}");
                }
                return Box::pin(self.check_job(&svc, jtype, allow_reload, manual)).await;
            }
        }
        if manual && starts && unit.get_bool("Unit", "RefuseManualStart", false) {
            return Err(err(
                E_ONLY_BY_DEP,
                format!("Operation refused, unit {name} may be requested by dependency only (it is configured to refuse manual start/stop)."),
            ));
        }
        if manual && jtype == "stop" && unit.get_bool("Unit", "RefuseManualStop", false) {
            return Err(err(E_ONLY_BY_DEP, format!("Operation refused, unit {name} may be requested by dependency only (it is configured to refuse manual start/stop).")));
        }
        if jtype == "reload" && !Self::can_reload(&unit) {
            return Err(err(
                E_NOT_APPLICABLE,
                format!("Job type reload is not applicable for unit {name}."),
            ));
        }
        // As systemd, a reload behind a queued or running start-type job is
        // accepted and runs after it (jobs on a unit are serialized);
        // unit_status() would say "activating" for that job and refuse.
        if jtype == "reload" {
            let behind_job = self
                .jobs
                .lock()
                .unwrap()
                .values()
                .any(|j| j.unit == id && j.jtype != "stop");
            let active = self.active_atoms().await;
            if !behind_job && self.unit_status(&id, &active, &unit).await.active != "active" {
                return Err(err(E_INACTIVE, format!("Unit {name} is not active.")));
            }
        }
        let transient = self.transient.lock().unwrap().contains_key(&id);
        let dynamic = self.dynamic_kind(&id).is_some() || self.can_be_dynamic(&id, &unit);
        if !transient && !dynamic && self.kind_of(&id).is_none() && jtype != "stop" {
            // Masked in the s6 set by the admin (AUTO_MASK='s leftovers are dynamic).
            if crate::install::set_rx(&self.cfg, &id) == Some("masked") {
                return Err(err(
                    E_MASKED,
                    format!(
                        "Unit {name} is masked in the s6 set ({}).",
                        self.cfg.set_name
                    ),
                ));
            }
            // New unit file since the last reload: translate it now (not for
            // bus activation, which any user can trigger).
            if allow_reload && !self.cfg.is_excluded(&id) {
                let _ = self.reload_db().await;
            }
            // AUTO_MASK='s leftovers are out of the database: on demand.
            if self.kind_of(&id).is_none() && !self.can_be_dynamic(&id, &unit) {
                let why = if self.cfg.is_excluded(&id) {
                    "excluded by uinitconverter configuration".to_string()
                } else {
                    gen::plan(&self.cfg)
                        .skipped
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "not translated to s6-rc".into())
                };
                return Err(err(
                    E_LOAD_FAILED,
                    format!("Unit {name} cannot be managed: {why}."),
                ));
            }
        }
        Ok((id, unit))
    }

    pub(super) async fn run_job(
        &self,
        id: &str,
        unit: &Unit,
        jtype: &str,
        mode: &str,
    ) -> &'static str {
        let transient = self.transient.lock().unwrap().contains_key(id);
        if transient {
            if jtype == "stop" {
                self.stop_transient(id).await;
            }
            return "done";
        }
        if jtype == "start" {
            if let Some(action) = shutdown_action(id) {
                let cmd = match action {
                    "poweroff" => self.cfg.poweroff_cmd.clone(),
                    "reboot" => self.cfg.reboot_cmd.clone(),
                    _ => self.cfg.halt_cmd.clone(),
                };
                eprintln!("uinitconverterd: {id} requested, running: {cmd}");
                let _ = tokio::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(cmd)
                    .spawn();
                return "done";
            }
        }
        // A running Accept=yes connection: not an s6 service, and it can't
        // be started again without its client.
        if self.connection(id).is_some() {
            return match jtype {
                "stop" => self.stop_connection(id, unit).await,
                "start" | "try-restart" | "reload-or-try-restart" => "done",
                _ => "failed",
            };
        }
        if self.dynamic_kind(id).is_some() || self.can_be_dynamic(id, unit) {
            // Not unit_status(): it reports this very job as "activating", so
            // try-restart would start an inactive instance (user@UID after
            // logout).
            let active = || async {
                match self.dynamic_kind(id) {
                    Some(Kind::Oneshot) => self
                        .dynamic
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|d| d.active)
                        .unwrap_or(false),
                    Some(_) => self.svstat(id).await.map(|s| s.up).unwrap_or(false),
                    None => false,
                }
            };
            return match jtype {
                "start" => self.dyn_start(id, unit).await,
                "stop" => {
                    let r = self.dyn_stop(id, unit).await;
                    if self.is_transient_file(unit) {
                        self.remove_transient_file(id);
                    }
                    self.emit(
                        MANAGER_PATH,
                        MANAGER_IFACE,
                        "UnitRemoved",
                        &(id.to_string(), op(unit_path(id))),
                    )
                    .await;
                    r
                }
                "restart" => self.dyn_restart(id, unit).await,
                "try-restart" => {
                    if active().await {
                        self.dyn_restart(id, unit).await
                    } else {
                        "done"
                    }
                }
                "reload" => self.reload_if_active(id, unit).await,
                _ => {
                    if Self::can_reload(unit) && active().await {
                        self.do_reload(id).await
                    } else if jtype == "reload-or-restart" || active().await {
                        self.dyn_restart(id, unit).await
                    } else {
                        "done"
                    }
                }
            };
        }
        match jtype {
            "start" => self.do_start(id, unit, mode == "isolate").await,
            "stop" => self.do_stop(id, unit).await,
            "restart" => self.do_restart(id, unit, false).await,
            "try-restart" => self.do_restart(id, unit, true).await,
            "reload" => self.reload_if_active(id, unit).await,
            "reload-or-restart" => {
                if Self::can_reload(unit) && self.active_atoms().await.contains(id) {
                    self.do_reload(id).await
                } else {
                    self.do_restart(id, unit, false).await
                }
            }
            // Nothing for an inactive unit (do_restart checks that too).
            "reload-or-try-restart" => {
                if Self::can_reload(unit) && self.active_atoms().await.contains(id) {
                    self.do_reload(id).await
                } else {
                    self.do_restart(id, unit, true).await
                }
            }
            _ => "failed",
        }
    }

    /// A reload accepted behind another job: that job may have left the
    /// unit inactive (systemd's job result "invalid").
    pub(super) async fn reload_if_active(&self, id: &str, unit: &Unit) -> &'static str {
        let active = self.active_atoms().await;
        if !matches!(
            self.unit_status(id, &active, unit).await.active,
            "active" | "reloading"
        ) {
            return "invalid";
        }
        self.do_reload(id).await
    }

    pub(super) async fn finish_job(&self, job: u32, id: &str, result: &str) {
        self.jobs.lock().unwrap().remove(&job);
        self.forget_unit_state(id);
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "JobRemoved",
            &(job, op(job_path(job)), id.to_string(), result.to_string()),
        )
        .await;
        let (_, unit) = self.load_unit(id);
        let active = self.active_atoms().await;
        let st = self.unit_status(id, &active, &unit).await;
        let mut changed: HashMap<String, Value<'static>> = HashMap::new();
        changed.insert("ActiveState".into(), Value::from(st.active.to_string()));
        changed.insert("SubState".into(), Value::from(st.sub.to_string()));
        self.emit(
            &unit_path(id),
            PROPS_IFACE,
            "PropertiesChanged",
            &(UNIT_IFACE, changed, Vec::<String>::new()),
        )
        .await;
    }

    /// Run a registered job: wait for earlier jobs on the same unit, honour
    /// CancelJob while waiting, then emit JobRemoved.
    pub(super) async fn execute_job(
        &self,
        job: u32,
        id: &str,
        unit: &Unit,
        jtype: &str,
        mode: &str,
    ) -> &'static str {
        let lock = self.unit_lock(id);
        let _guard = lock.lock().await;
        let canceled = {
            let mut jobs = self.jobs.lock().unwrap();
            match jobs.get_mut(&job) {
                Some(j) if !j.canceled => {
                    j.state = "running";
                    false
                }
                _ => true,
            }
        };
        let result = if canceled {
            "canceled"
        } else {
            self.run_job(id, unit, jtype, mode).await
        };
        self.finish_job(job, id, result).await;
        result
    }

    /// Reply with the job path; if the caller is already gone, drop the job
    /// instead of leaving it registered forever.
    pub(super) async fn reply_job(&self, c: &Peer, msg: &Message, job: u32) -> R<()> {
        if let Err(e) = self.reply(c, msg, &(op(job_path(job)),)).await {
            self.jobs.lock().unwrap().remove(&job);
            return Err(e);
        }
        Ok(())
    }

    pub(super) async fn job_method(
        &self,
        c: &Peer,
        msg: &Message,
        name: &str,
        mode: &str,
        jtype: &str,
    ) -> R<()> {
        Self::valid_name(name)?;
        let id = self.index.read().unwrap().resolve(name).to_string();
        self.mac_unit(c, msg, &id, access::job_access(jtype))
            .await?;
        self.require_unit(c, msg, &id, jtype).await?;
        let (id, unit) = self.check_job(name, jtype, true, true).await?;
        let job = self.new_job(&id, jtype);
        self.reply_job(c, msg, job).await?;
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "JobNew",
            &(job, op(job_path(job)), id.clone()),
        )
        .await;
        self.execute_job(job, &id, &unit, jtype, mode).await;
        Ok(())
    }

    /// Like systemd's: try-restart the units marked needs-restart, reload the
    /// active ones marked only needs-reload, and clear the markers. Package
    /// updates restart their services this way (systemd-update-helper).
    pub(super) async fn enqueue_marked(&self, c: &Peer, msg: &Message) -> R<()> {
        self.mac_check(c, msg, None, "start").await?;
        self.require_privileged(c, msg).await?;
        let marked = std::mem::take(&mut *self.markers.lock().unwrap());
        let mut queued = Vec::new();
        for (name, m) in marked {
            let jtype = if m.contains("needs-restart") {
                "try-restart"
            } else {
                "reload"
            };
            // As systemd: a unit the caller may not act on is skipped.
            if let Err(e) = self
                .mac_unit(c, msg, &name, access::job_access(jtype))
                .await
            {
                eprintln!(
                    "uinitconverterd: {name}: failed to enqueue marked job: {}",
                    e.msg
                );
                continue;
            }
            match self.check_job(&name, jtype, true, false).await {
                Ok((id, unit)) => queued.push((self.new_job(&id, jtype), id, unit, jtype)),
                // A reload of an inactive unit is a no-op for systemd too.
                Err(e) if e.name == E_INACTIVE => {}
                Err(e) => eprintln!("uinitconverterd: marked {name}: {}", e.msg),
            }
        }
        let paths: Vec<ObjectPath<'static>> =
            queued.iter().map(|(job, ..)| op(job_path(*job))).collect();
        if let Err(e) = self.reply(c, msg, &(paths,)).await {
            let mut jobs = self.jobs.lock().unwrap();
            for (job, ..) in &queued {
                jobs.remove(job);
            }
            return Err(e);
        }
        for (job, id, ..) in &queued {
            self.emit(
                MANAGER_PATH,
                MANAGER_IFACE,
                "JobNew",
                &(*job, op(job_path(*job)), id.clone()),
            )
            .await;
        }
        futures_util::future::join_all(
            queued
                .iter()
                .map(|(job, id, unit, jtype)| self.execute_job(*job, id, unit, jtype, "fail")),
        )
        .await;
        Ok(())
    }

    /// Queued jobs can be cancelled; running ones finish (s6-rc transitions
    /// cannot be interrupted safely).
    pub(super) fn cancel_job(&self, id: u32) -> R<()> {
        let mut jobs = self.jobs.lock().unwrap();
        match jobs.get_mut(&id) {
            None => Err(err(E_NO_JOB, format!("Job {id} does not exist."))),
            Some(j) if j.state == "waiting" => {
                j.canceled = true;
                Ok(())
            }
            Some(_) => Err(err(
                E_NOT_SUPPORTED,
                "Job is already running and cannot be cancelled",
            )),
        }
    }
}
