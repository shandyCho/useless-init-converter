//! StartTransientUnit and what follows from it: scopes, slices, attached processes.

use super::*;

/// Targets/services that mean "shut the machine down" (logind starts these).
/// Wait until these cgroups have no process, up to `limit`; how many still do.
pub(super) async fn wait_cgroups_empty(dirs: &[PathBuf], limit: Duration) -> usize {
    let start = std::time::Instant::now();
    loop {
        let left = dirs
            .iter()
            .filter(|d| !util::cgroup_pids(d).is_empty())
            .count();
        if left == 0 || start.elapsed() >= limit {
            return left;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// AttachProcessesToUnit's subcgroup, relative to the unit's cgroup, as
/// systemd's path_is_normalized() takes it: a leading or trailing '/' is
/// fine, but no ".", ".." or empty component, which could leave the unit's
/// cgroup. None if it isn't normalized.
pub(super) fn subcgroup(sub: &str) -> Option<&str> {
    let rel = sub.trim_start_matches('/').trim_end_matches('/');
    if rel.is_empty() {
        return Some("");
    }
    rel.split('/')
        .all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\0'))
        .then_some(rel)
}

/// The unit a cgroup belongs to, as the system manager sees it: the
/// outermost .service or .scope (below it is a delegated tree, such as
/// user@1000.service/app.slice/x.service), else the innermost slice.
pub(super) fn unit_of_cgroup(cg: &str) -> Option<String> {
    let parts: Vec<&str> = cg.split('/').filter(|c| !c.is_empty()).collect();
    parts
        .iter()
        .find(|c| c.ends_with(".service") || c.ends_with(".scope"))
        .or_else(|| parts.iter().rev().find(|c| c.ends_with(".slice")))
        .map(|c| c.to_string())
}

/// PIDs behind the pidfds of a PIDFDs (ah) property.
pub(super) fn pids_from_pidfds(v: &OwnedValue) -> Vec<u32> {
    use std::os::fd::AsRawFd;
    let mut out = Vec::new();
    if let Value::Array(a) = &**v {
        for item in a.iter() {
            if let Value::Fd(fd) = item {
                let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))
                    .unwrap_or_default();
                if let Some(pid) = info
                    .lines()
                    .find_map(|l| l.strip_prefix("Pid:"))
                    .and_then(|p| p.trim().parse::<i64>().ok())
                    .filter(|&p| p > 0)
                {
                    out.push(pid as u32);
                }
            }
        }
    }
    out
}

impl Daemon {
    /// Runtime property changes: only Markers= is kept (package scriptlets
    /// mark units for EnqueueMarkedJobs); the others are accepted and ignored.
    pub(super) async fn set_unit_properties(
        &self,
        c: &Peer,
        msg: &Message,
        name: &str,
        props: &[(String, OwnedValue)],
    ) -> R<()> {
        Self::valid_name(name)?;
        let id = self.index.read().unwrap().resolve(name).to_string();
        self.mac_unit(c, msg, &id, "start").await?;
        self.require_unit(c, msg, &id, "set-property").await?;
        for (key, v) in props {
            if key != "Markers" {
                continue;
            }
            let words: Vec<String> = match &**v {
                Value::Array(a) => a
                    .iter()
                    .filter_map(|x| <&str>::try_from(x).ok().map(String::from))
                    .collect(),
                _ => return Err(err(E_INVALID_ARGS, "Markers= takes a string array")),
            };
            let (id, unit) = self.load_unit(name);
            if unit.load_state == LoadState::NotFound {
                return Err(err(E_NO_UNIT, format!("Unit {name} not found.")));
            }
            let mut all = self.markers.lock().unwrap();
            let m = apply_markers(all.get(&id).cloned().unwrap_or_default(), &words)
                .map_err(|e| err(E_INVALID_ARGS, e))?;
            if m.is_empty() {
                all.remove(&id);
            } else {
                all.insert(id, m);
            }
        }
        self.reply(c, msg, &()).await
    }

    /// Stop a transient unit. Scopes: SIGTERM (+SIGHUP), SIGKILL after a grace
    /// period. Slices: stop everything inside, then remove the cgroup.
    pub(super) async fn stop_transient(&self, id: &str) {
        let Some((kind, dir, hup)) = self
            .transient
            .lock()
            .unwrap()
            .get(id)
            .map(|t| (t.kind, t.cgroup.clone(), t.send_sighup))
        else {
            return;
        };
        if dir == std::path::Path::new(crate::cgroup::CGROUP_ROOT) || dir == crate::cgroup::base() {
            eprintln!("uinitconverterd: refusing to stop {id}: it is the root cgroup");
            return;
        }
        if kind == "slice" {
            let inside: Vec<String> = self
                .dynamic
                .lock()
                .unwrap()
                .keys()
                .filter(|n| {
                    let (_, u) = self.load_unit(n);
                    self.cgroup_dir_of(n, &u).starts_with(&dir)
                })
                .cloned()
                .collect();
            for n in inside {
                let (_, u) = self.load_unit(&n);
                self.dyn_stop(&n, &u).await;
                self.emit(
                    MANAGER_PATH,
                    MANAGER_IFACE,
                    "UnitRemoved",
                    &(n.clone(), op(unit_path(&n))),
                )
                .await;
            }
            let scopes: Vec<String> = self
                .transient
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, t)| {
                    t.kind == "scope" && t.cgroup.starts_with(&dir) && n.as_str() != id
                })
                .map(|(n, _)| n.clone())
                .collect();
            for sc in scopes {
                Box::pin(self.stop_transient(&sc)).await;
            }
            crate::cgroup::kill_all(&dir, libc::SIGTERM, 0);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            crate::cgroup::kill_all(&dir, libc::SIGKILL, 0);
            crate::cgroup::remove(&dir);
            self.transient.lock().unwrap().remove(id);
            let _ = std::fs::remove_dir_all(self.cfg.unit_run_dir(id));
            self.emit(
                MANAGER_PATH,
                MANAGER_IFACE,
                "UnitRemoved",
                &(id.to_string(), op(unit_path(id))),
            )
            .await;
            return;
        }
        crate::cgroup::kill_all(&dir, libc::SIGTERM, 0);
        if hup {
            crate::cgroup::kill_all(&dir, libc::SIGHUP, 0);
        }
        let d = dir.clone();
        tokio::spawn(async move {
            for _ in 0..100 {
                if util::cgroup_pids(&d).is_empty() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            crate::cgroup::kill_all(&d, libc::SIGKILL, 0);
        });
    }

    pub(super) async fn start_transient(&self, c: &Peer, msg: &Message) -> R<()> {
        self.mac_check(c, msg, None, "start").await?;
        self.require_privileged(c, msg).await?;
        type Aux = Vec<(String, Vec<(String, OwnedValue)>)>;
        let (name, mode, props, aux): (String, String, Vec<(String, OwnedValue)>, Aux) =
            Self::args(msg)?;
        // The name becomes a cgroup and a state directory: as for the job
        // methods, only valid unit names (no '/', no "..").
        Self::valid_name(&name)?;
        if name.ends_with(".service") || name.ends_with(".timer") || name.ends_with(".path") {
            return self
                .start_transient_file(c, msg, &name, &mode, &props, &aux)
                .await;
        }
        let kind: &'static str = if name.ends_with(".scope") {
            "scope"
        } else if name.ends_with(".slice") {
            if crate::cgroup::slice_path(&name).as_os_str().is_empty() {
                return Err(err(
                    E_INVALID_ARGS,
                    "The root slice cannot be created as a transient unit.",
                ));
            }
            "slice"
        } else {
            return Err(err(
                E_NOT_SUPPORTED,
                format!("Transient units of this type are not supported by uinitconverter: {name}"),
            ));
        };
        if self.transient.lock().unwrap().contains_key(&name) {
            return Err(err(
                "org.freedesktop.systemd1.UnitExists",
                format!("Unit {name} already exists."),
            ));
        }
        let mut pids: Vec<u32> = Vec::new();
        let mut slice = if kind == "scope" {
            "system.slice".to_string()
        } else {
            String::new()
        };
        let mut description = name.clone();
        let mut send_sighup = false;
        for (k, v) in &props {
            match k.as_str() {
                "PIDs" => {
                    if let Ok(list) = <Vec<u32>>::try_from(
                        v.try_clone()
                            .map_err(|e| err(E_INVALID_ARGS, e.to_string()))?,
                    ) {
                        pids.extend(list);
                    }
                }
                "PIDFDs" => pids.extend(pids_from_pidfds(v)),
                "Slice" => {
                    if let Ok(s) = <&str>::try_from(&**v) {
                        slice = s.to_string();
                    }
                }
                "Description" => {
                    if let Ok(s) = <&str>::try_from(&**v) {
                        description = s.to_string();
                    }
                }
                "SendSIGHUP" => {
                    if let Ok(b) = <bool>::try_from(&**v) {
                        send_sighup = b;
                    }
                }
                _ => {}
            }
        }
        let dir = if kind == "slice" {
            crate::cgroup::slice_dir(&name)
        } else {
            if pids.is_empty() {
                if let Some(p) = self.caller_pid(c, msg).await {
                    pids.push(p);
                }
            }
            crate::cgroup::unit_cgroup_dir(&name, &slice)
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| err(E_FAILED, format!("{}: {e}", dir.display())))?;
        for p in &pids {
            if let Err(e) = std::fs::write(dir.join("cgroup.procs"), p.to_string()) {
                return Err(err(
                    E_FAILED,
                    format!("cannot move PID {p} into {}: {e}", dir.display()),
                ));
            }
        }
        let st = state::for_unit(&self.cfg.run_dir, &name);
        st.stamp(keys::ACTIVE_ENTER);
        if kind == "slice" {
            slice = crate::cgroup::parent_slice(&name);
        }
        let t = Transient {
            kind,
            cgroup: dir,
            send_sighup,
            slice,
            description,
        };
        transient::save_scope(&self.cfg.run_dir, &name, &t);
        self.transient.lock().unwrap().insert(name.clone(), t);
        let job = self.new_job(&name, "start");
        self.reply_job(c, msg, job).await?;
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "UnitNew",
            &(name.clone(), op(unit_path(&name))),
        )
        .await;
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "JobNew",
            &(job, op(job_path(job)), name.clone()),
        )
        .await;
        self.finish_job(job, &name, "done").await;
        Ok(())
    }

    /// systemd-run: write the unit file(s), then start the main unit on demand.
    pub(super) async fn start_transient_file(
        &self,
        c: &Peer,
        msg: &Message,
        name: &str,
        mode: &str,
        props: &[(String, OwnedValue)],
        aux: &[(String, Vec<(String, OwnedValue)>)],
    ) -> R<()> {
        if crate::loader::parse_name(name).is_none() {
            return Err(err(E_INVALID_ARGS, format!("Invalid unit name {name}")));
        }
        for n in std::iter::once(name).chain(aux.iter().map(|(n, _)| n.as_str())) {
            if self.kind_of(n).is_some() || self.dynamic_kind(n).is_some() {
                return Err(err(
                    "org.freedesktop.systemd1.UnitExists",
                    format!("Unit {n} already exists."),
                ));
            }
        }
        self.write_transient(name, props, aux).map_err(|e| {
            if e.contains("not supported") {
                err(E_NOT_SUPPORTED, e)
            } else if e.starts_with("Invalid") {
                err(E_INVALID_ARGS, e)
            } else {
                err("org.freedesktop.systemd1.UnitExists", e)
            }
        })?;
        let (id, unit) = self.load_unit(name);
        if !unit.is_loaded() {
            self.remove_transient_file(name);
            return Err(err(
                E_LOAD_FAILED,
                unit.load_error
                    .unwrap_or_else(|| format!("Unit {name} failed to load.")),
            ));
        }
        if let Err(e) = self.store_stdio(&id, props) {
            self.remove_transient_file(&id);
            return Err(err(E_INVALID_ARGS, e));
        }
        let job = self.new_job(&id, "start");
        if let Err(e) = self.reply_job(c, msg, job).await {
            self.remove_transient_file(&id);
            return Err(e);
        }
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "UnitNew",
            &(id.clone(), op(unit_path(&id))),
        )
        .await;
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "JobNew",
            &(job, op(job_path(job)), id.clone()),
        )
        .await;
        let result = self.execute_job(job, &id, &unit, "start", mode).await;
        // A start that failed before the unit became an instance (s6-svlink,
        // a dependency): watch_dynamic never sees it, so it would stay loaded
        // and the fd store would keep its Standard*FileDescriptor, which holds
        // the caller's pipe open: `systemctl --user -M` in an RPM scriptlet
        // left dnf waiting for good. Listed as a finished oneshot, it unloads
        // like any other inactive transient unit, once clients read the result.
        if result != "done" && unit.utype() == "service" && self.dynamic_kind(&id).is_none() {
            crate::state::for_unit(&self.cfg.run_dir, &id)
                .stamp(crate::state::keys::INACTIVE_ENTER);
            self.dynamic.lock().unwrap().insert(
                id.clone(),
                super::dynamic::Dynamic {
                    kind: crate::gen::Kind::Oneshot,
                    active: false,
                },
            );
        }
        Ok(())
    }

    /// AttachProcessesToUnit(unit, subcgroup, pids)
    /// As systemd, "" names the caller's own unit (systemd --user moves its
    /// processes within user@UID.service so). An unprivileged caller may
    /// move its own processes inside its own delegated unit; anything else
    /// needs manage-units.
    pub(super) async fn attach_processes(&self, c: &Peer, msg: &Message) -> R<()> {
        let (name, sub, pids): (String, String, Vec<u32>) = Self::args(msg)?;
        let Some(sub) = subcgroup(&sub) else {
            return Err(err(
                E_INVALID_ARGS,
                format!("Control group path is not normalized: {sub}"),
            ));
        };
        let name = if name.is_empty() {
            let pid = self.caller_pid(c, msg).await.unwrap_or(0);
            util::proc_cgroup(pid)
                .and_then(|cg| unit_of_cgroup(&cg))
                .ok_or_else(|| {
                    err(
                        E_NO_UNIT_FOR_PID,
                        format!("PID {pid} does not belong to any loaded unit."),
                    )
                })?
        } else {
            name
        };
        Self::valid_name(&name)?;
        let (id, unit) = self.load_unit(&name);
        self.mac_check(c, msg, Some(&unit), "start").await?;
        let caller = self.caller_uid(c, msg).await;
        let caller_unit = match self.caller_pid(c, msg).await {
            Some(p) => util::proc_cgroup(p).and_then(|cg| unit_of_cgroup(&cg)),
            None => None,
        };
        let own = |pid: &u32| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(format!("/proc/{pid}"))
                .ok()
                .map(|m| m.uid())
                == caller
                && util::proc_cgroup(*pid)
                    .and_then(|cg| unit_of_cgroup(&cg))
                    .as_deref()
                    == Some(id.as_str())
        };
        let delegated = caller.is_some_and(|u| u != 0)
            && caller_unit.as_deref() == Some(id.as_str())
            && (unit.get_bool("Service", "Delegate", false)
                || unit.get_bool("Scope", "Delegate", false))
            && pids.iter().all(own);
        if !delegated {
            self.require_privileged(c, msg).await?;
        }
        let mut dir = self.cgroup_dir_of(&id, &unit);
        if !sub.is_empty() {
            dir = dir.join(sub);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| err(E_FAILED, format!("{}: {e}", dir.display())))?;
        for p in pids {
            std::fs::write(dir.join("cgroup.procs"), p.to_string())
                .map_err(|e| err(E_FAILED, format!("cannot move PID {p}: {e}")))?;
        }
        self.reply(c, msg, &()).await
    }

    /// Shutdown: the scopes before the on-demand instances. systemd stops
    /// session-N.scope and only then user@UID.service, so GNOME's session ends
    /// by itself before its user manager goes to exit.target. Stopped at the
    /// same time, the manager tears down the session's services under a
    /// gnome-shell that is still exiting, and activations fail under it.
    /// Bounded: stop_transient sends SIGKILL after 10 s.
    pub(super) async fn stop_scopes_at_shutdown(&self) {
        self.stop_display_manager_at_shutdown().await;
        let root = crate::cgroup::base();
        let scopes: Vec<(String, PathBuf)> = self
            .transient
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| {
                t.kind == "scope" && t.cgroup != root && !util::cgroup_pids(&t.cgroup).is_empty()
            })
            .map(|(n, t)| (n.clone(), t.cgroup.clone()))
            .collect();
        let names: Vec<&str> = scopes.iter().map(|(n, _)| n.as_str()).collect();
        let t = std::time::Instant::now();
        for name in &names {
            self.stop_transient(name).await;
        }
        let dirs: Vec<PathBuf> = scopes.iter().map(|(_, d)| d.clone()).collect();
        let left = wait_cgroups_empty(&dirs, Duration::from_secs(11)).await;
        // Always logged, unlike the slow-stop lines: its time against the user
        // manager's exit.target says whether this wait is what ordered them.
        eprintln!(
            "uinitconverterd: shutdown scopes [{}]: {left} not empty after {:.3} s",
            names.join(" "),
            t.elapsed().as_secs_f64()
        );
    }

    /// systemd queues the display manager's stop together with the sessions'.
    /// Left running until s6-rc's turn, gdm sees its session end and starts a
    /// new greeter, whose user@UID is refused (destructive); its worker aborts,
    /// up to gdm's limit of six tries. Only asks: this doesn't wait, so it adds
    /// nothing to the order above. display-manager.service is the alias name
    /// display managers install, so no product is named here. Sent again at
    /// s6-rc's SIGTERM, which changes nothing.
    async fn stop_display_manager_at_shutdown(&self) {
        let id = self
            .index
            .read()
            .unwrap()
            .resolve("display-manager.service")
            .to_string();
        let dir = self.scandir(&id);
        if !dir.is_dir() {
            return;
        }
        let _ = tokio::process::Command::new(crate::tools::path("s6-svc"))
            .arg("-d")
            .arg(dir)
            .status()
            .await;
    }

    pub(super) async fn gc_scopes(&self) {
        let empty: Vec<(String, PathBuf)> = self
            .transient
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.kind == "scope" && util::cgroup_pids(&t.cgroup).is_empty())
            .map(|(n, t)| (n.clone(), t.cgroup.clone()))
            .collect();
        for (name, dir) in empty {
            self.transient.lock().unwrap().remove(&name);
            crate::cgroup::remove(&dir);
            let _ = std::fs::remove_dir_all(self.cfg.unit_run_dir(&name));
            self.emit(
                MANAGER_PATH,
                MANAGER_IFACE,
                "UnitRemoved",
                &(name.clone(), op(unit_path(&name))),
            )
            .await;
        }
    }
}
