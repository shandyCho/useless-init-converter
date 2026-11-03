//! org.freedesktop.systemd1.Manager methods.

use super::*;

pub(super) type UnitEntry = (
    String,
    String,
    String,
    String,
    String,
    String,
    ObjectPath<'static>,
    u32,
    String,
    ObjectPath<'static>,
);

pub(super) type Changes = Vec<(String, String, String)>;

pub(super) fn changes(c: Vec<install::Change>) -> Changes {
    c.into_iter().map(|c| (c.kind, c.path, c.source)).collect()
}

pub(super) const FLAG_RUNTIME: u64 = 1;

pub(super) const FLAG_FORCE: u64 = 2;

impl Daemon {
    pub(super) async fn unit_entry(
        &self,
        id: &str,
        unit: &Unit,
        active: &BTreeSet<String>,
    ) -> UnitEntry {
        let st = self.unit_status(id, active, unit).await;
        let (job_id, job_type) = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .find(|j| j.unit == id)
            .map(|j| (j.id, j.jtype.clone()))
            .unwrap_or((0, String::new()));
        (
            id.to_string(),
            unit.description(),
            unit.load_state.as_str().to_string(),
            st.active.to_string(),
            st.sub.to_string(),
            String::new(),
            op(unit_path(id)),
            job_id,
            job_type,
            op(if job_id > 0 {
                job_path(job_id)
            } else {
                "/".into()
            }),
        )
    }

    pub(super) async fn list_units(
        &self,
        states: &[String],
        patterns: &[String],
        names: Option<&[String]>,
    ) -> Vec<UnitEntry> {
        let active = self.active_atoms().await;
        let ids: Vec<String> = match names {
            Some(n) => n.to_vec(),
            None => self.known_units(),
        };
        let selected: Vec<(String, Unit)> = ids
            .into_iter()
            .filter(|n| patterns.is_empty() || patterns.iter().any(|p| util::fnmatch(p, n)))
            .map(|n| self.load_unit(&n))
            .collect();
        // Status queries spawn s6-svstat: run them concurrently.
        let entries = futures_util::future::join_all(
            selected
                .iter()
                .map(|(id, unit)| self.unit_entry(id, unit, &active)),
        )
        .await;
        entries
            .into_iter()
            .filter(|e| {
                states.is_empty() || states.iter().any(|s| *s == e.2 || *s == e.3 || *s == e.4)
            })
            .collect()
    }

    pub(super) async fn count_failed(&self) -> u32 {
        let ids = self.known_units();
        let mut candidates = Vec::new();
        for id in ids {
            let st = state::for_unit(&self.cfg.run_dir, &id);
            if st
                .get(keys::RESULT)
                .map(|r| result::is_failure(&r))
                .unwrap_or(false)
            {
                candidates.push(id);
            }
        }
        if candidates.is_empty() {
            return 0;
        }
        let active = self.active_atoms().await;
        let mut n = 0;
        for id in candidates {
            let (_, unit) = self.load_unit(&id);
            if self.unit_status(&id, &active, &unit).await.active == "failed" {
                n += 1;
            }
        }
        n
    }

    /// install::list_unit_files() with the states as the s6 set has them.
    pub(super) fn list_unit_files(&self) -> Vec<(String, String)> {
        install::list_unit_files(&self.cfg)
            .into_iter()
            .map(|(p, st)| {
                let st = install::set_file_state(&self.cfg, p.rsplit('/').next().unwrap_or(&p), st);
                (p, st)
            })
            .collect()
    }

    pub(super) async fn file_op_done(&self) {
        self.invalidate_caches();
        self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitFilesChanged", &())
            .await;
    }

    pub(super) fn write_manager_env(&self) {
        let env = self.manager_env.lock().unwrap().clone();
        let mut text = String::new();
        for e in env {
            if let Some((k, v)) = e.split_once('=') {
                let v = v
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "\\$")
                    .replace('`', "\\`");
                text.push_str(&format!("{k}=\"{v}\"\n"));
            }
        }
        let _ = util::write_atomic(&self.cfg.run_dir.join("manager.env"), text.as_bytes());
    }

    pub(super) async fn manager_method(&self, c: &Peer, msg: &Message, member: &str) -> R<()> {
        match member {
            "GetUnit" => {
                let (name,): (String,) = Self::args(msg)?;
                let (id, unit) = self.load_unit(&name);
                let known =
                    self.kind_of(&id).is_some() || self.transient.lock().unwrap().contains_key(&id);
                if !known
                    && (unit.load_state == LoadState::NotFound
                        || crate::loader::parse_name(&name).is_none())
                {
                    return Err(err(E_NO_UNIT, format!("Unit {name} not loaded.")));
                }
                self.mac_check(c, msg, Some(&unit), "status").await?;
                self.reply(c, msg, &(op(unit_path(&id)),)).await
            }
            "LoadUnit" => {
                let (name,): (String,) = Self::args(msg)?;
                Self::valid_name(&name)?;
                let (id, unit) = self.load_unit(&name);
                self.mac_check(c, msg, Some(&unit), "status").await?;
                self.reply(c, msg, &(op(unit_path(&id)),)).await
            }
            "GetUnitByPID" => {
                let (mut pid,): (u32,) = Self::args(msg)?;
                if pid == 0 {
                    pid = self.caller_pid(c, msg).await.unwrap_or(0);
                }
                match self.unit_of_pid(pid) {
                    Some(u) => {
                        self.mac_unit(c, msg, &u, "status").await?;
                        self.reply(c, msg, &(op(unit_path(&u)),)).await
                    }
                    None => Err(err(
                        E_NO_UNIT_FOR_PID,
                        format!("PID {pid} does not belong to any loaded unit."),
                    )),
                }
            }
            // polkitd asks this for every caller, then reads the unit's
            // NoNewPrivileges.
            "GetUnitByPIDFD" => {
                use std::os::fd::AsRawFd;
                let (fd,): (zbus::zvariant::OwnedFd,) = Self::args(msg)?;
                let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))
                    .unwrap_or_default();
                let pid = info
                    .lines()
                    .find_map(|l| l.strip_prefix("Pid:"))
                    .and_then(|p| p.trim().parse::<i64>().ok())
                    .filter(|&p| p > 0)
                    .ok_or_else(|| err(E_NO_UNIT_FOR_PID, "Not a pidfd of a running process."))?
                    as u32;
                match self.unit_of_pid(pid) {
                    Some(u) => {
                        self.mac_unit(c, msg, &u, "status").await?;
                        let inv = state::for_unit(&self.cfg.run_dir, &u)
                            .get(keys::INVOCATION)
                            .unwrap_or_default();
                        let bytes: Vec<u8> = (0..inv.len() / 2)
                            .filter_map(|i| u8::from_str_radix(inv.get(i * 2..i * 2 + 2)?, 16).ok())
                            .collect();
                        self.reply(c, msg, &(op(unit_path(&u)), u.clone(), bytes))
                            .await
                    }
                    None => Err(err(
                        E_NO_UNIT_FOR_PID,
                        format!("PID {pid} does not belong to any loaded unit."),
                    )),
                }
            }
            "StartUnit"
            | "StopUnit"
            | "ReloadUnit"
            | "RestartUnit"
            | "TryRestartUnit"
            | "ReloadOrRestartUnit"
            | "ReloadOrTryRestartUnit" => {
                let (name, mode): (String, String) = Self::args(msg)?;
                let jtype = match member {
                    "StartUnit" => "start",
                    "StopUnit" => "stop",
                    "ReloadUnit" => "reload",
                    "RestartUnit" => "restart",
                    "TryRestartUnit" => "try-restart",
                    "ReloadOrRestartUnit" => "reload-or-restart",
                    _ => "reload-or-try-restart",
                };
                self.job_method(c, msg, &name, &mode, jtype).await
            }
            "KillUnit" => {
                let (name, whom, sig): (String, String, i32) = Self::args(msg)?;
                self.kill_unit(c, msg, &name, &whom, sig).await
            }
            "ResetFailedUnit" => {
                let (name,): (String,) = Self::args(msg)?;
                Self::valid_name(&name)?;
                let id = self.index.read().unwrap().resolve(&name).to_string();
                self.mac_unit(c, msg, &id, "reload").await?;
                self.require_unit(c, msg, &id, "reset-failed").await?;
                self.reset_failed(&name);
                self.reply(c, msg, &()).await
            }
            "ResetFailed" => {
                self.mac_check(c, msg, None, "reload").await?;
                self.require_privileged(c, msg).await?;
                for u in self.known_units() {
                    self.reset_failed(&u);
                }
                self.reply(c, msg, &()).await
            }
            "GetUnitProcesses" => {
                let (name,): (String,) = Self::args(msg)?;
                if crate::loader::parse_name(&name).is_none() {
                    return Err(err(E_NO_UNIT, format!("Unit {name} not loaded.")));
                }
                let (id, unit) = self.load_unit(&name);
                self.mac_check(c, msg, Some(&unit), "status").await?;
                let dir = self.cgroup_dir_of(&id, &unit);
                let procs: Vec<(String, u32, String)> = util::cgroup_pids(&dir)
                    .into_iter()
                    .map(|p| {
                        (
                            util::proc_cgroup(p).unwrap_or_default(),
                            p,
                            util::proc_cmdline(p),
                        )
                    })
                    .collect();
                self.reply(c, msg, &(procs,)).await
            }
            "StartTransientUnit" => self.start_transient(c, msg).await,
            "AttachProcessesToUnit" => self.attach_processes(c, msg).await,
            "SetUnitProperties" => {
                let (name, _runtime, props): (String, bool, Vec<(String, OwnedValue)>) =
                    Self::args(msg)?;
                self.set_unit_properties(c, msg, &name, &props).await
            }
            "EnqueueMarkedJobs" => self.enqueue_marked(c, msg).await,
            // Reference counting is accepted and ignored.
            "RefUnit" | "UnrefUnit" | "AbandonScope" => {
                let (name,): (String,) = Self::args(msg)?;
                self.mac_unit(
                    c,
                    msg,
                    &name,
                    if member == "AbandonScope" {
                        "stop"
                    } else {
                        "start"
                    },
                )
                .await?;
                self.require_privileged(c, msg).await?;
                self.reply(c, msg, &()).await
            }
            "GetJob" => {
                let (id,): (u32,) = Self::args(msg)?;
                if let Some(u) = self.job_unit(id) {
                    self.mac_unit(c, msg, &u, "status").await?;
                    self.reply(c, msg, &(op(job_path(id)),)).await
                } else {
                    Err(err(E_NO_JOB, format!("Job {id} does not exist.")))
                }
            }
            "CancelJob" => {
                let (id,): (u32,) = Self::args(msg)?;
                if let Some(u) = self.job_unit(id) {
                    self.mac_unit(c, msg, &u, "stop").await?;
                }
                self.require_privileged(c, msg).await?;
                self.cancel_job(id)?;
                self.reply(c, msg, &()).await
            }
            "ClearJobs" => {
                self.mac_check(c, msg, None, "reload").await?;
                self.reply(c, msg, &()).await
            }
            "ListJobs" => {
                self.mac_check(c, msg, None, "status").await?;
                let jobs: Vec<(
                    u32,
                    String,
                    String,
                    String,
                    ObjectPath<'static>,
                    ObjectPath<'static>,
                )> = self
                    .jobs
                    .lock()
                    .unwrap()
                    .values()
                    .map(|j| {
                        (
                            j.id,
                            j.unit.clone(),
                            j.jtype.clone(),
                            j.state.to_string(),
                            op(job_path(j.id)),
                            op(unit_path(&j.unit)),
                        )
                    })
                    .collect();
                self.reply(c, msg, &(jobs,)).await
            }
            "ListUnits" => {
                let v = self
                    .visible_units(c, msg, self.list_units(&[], &[], None).await)
                    .await;
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitsFiltered" => {
                let (states,): (Vec<String>,) = Self::args(msg)?;
                let v = self
                    .visible_units(c, msg, self.list_units(&states, &[], None).await)
                    .await;
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitsByPatterns" => {
                let (states, patterns): (Vec<String>, Vec<String>) = Self::args(msg)?;
                let v = self
                    .visible_units(c, msg, self.list_units(&states, &patterns, None).await)
                    .await;
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitsByNames" => {
                let (mut names,): (Vec<String>,) = Self::args(msg)?;
                // systemd skips names that aren't unit names.
                names.retain(|n| crate::loader::parse_name(n).is_some());
                let v = self.list_units(&[], &[], Some(&names)).await;
                self.reply(c, msg, &(v,)).await
            }
            "Subscribe" | "Unsubscribe" => {
                self.mac_check(c, msg, None, "status").await?;
                self.reply(c, msg, &()).await
            }
            "Reload" | "Reexecute" => {
                self.mac_check(c, msg, None, "reload").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.reload-daemon")
                    .await?;
                self.reload_db().await.map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &()).await
            }
            "PowerOff" | "Reboot" | "Halt" | "KExec" | "SoftReboot" | "SwitchRoot"
                if self.cfg.user_mode =>
            {
                Err(err(
                    E_NOT_SUPPORTED,
                    format!("{member} is only supported by system managers."),
                ))
            }
            "Exit" if self.cfg.user_mode => {
                // systemd: a user manager exits (its units stopped).
                self.require_privileged(c, msg).await?;
                self.reply(c, msg, &()).await?;
                self.request_exit();
                Ok(())
            }
            "PowerOff" | "Reboot" | "Halt" => {
                self.mac_check(
                    c,
                    msg,
                    None,
                    if member == "Reboot" { "reboot" } else { "halt" },
                )
                .await?;
                self.require_privileged(c, msg).await?;
                let cmd = match member {
                    "PowerOff" => self.cfg.poweroff_cmd.clone(),
                    "Reboot" => self.cfg.reboot_cmd.clone(),
                    _ => self.cfg.halt_cmd.clone(),
                };
                self.reply(c, msg, &()).await?;
                let _ = tokio::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(cmd)
                    .spawn();
                Ok(())
            }
            "KExec" | "SoftReboot" | "SwitchRoot" | "Exit" => Err(err(
                E_NOT_SUPPORTED,
                format!("{member} is not supported by uinitconverter"),
            )),
            "SetEnvironment" | "UnsetEnvironment" | "UnsetAndSetEnvironment" => {
                self.mac_check(c, msg, None, "reload").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.set-environment")
                    .await?;
                let (unset, set): (Vec<String>, Vec<String>) = match member {
                    "SetEnvironment" => (Vec::new(), Self::args::<(Vec<String>,)>(msg)?.0),
                    "UnsetEnvironment" => (Self::args::<(Vec<String>,)>(msg)?.0, Vec::new()),
                    _ => Self::args(msg)?,
                };
                {
                    let mut env = self.manager_env.lock().unwrap();
                    for u in &unset {
                        env.retain(|e| e != u && e.split('=').next() != Some(u.as_str()));
                    }
                    for s in set {
                        let k = s.split('=').next().unwrap_or("").to_string();
                        env.retain(|e| e.split('=').next() != Some(k.as_str()));
                        env.push(s);
                    }
                }
                self.write_manager_env();
                self.reply(c, msg, &()).await
            }
            "ListUnitFiles" => {
                self.mac_check(c, msg, None, "status").await?;
                let v = self.list_unit_files();
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitFilesByPatterns" => {
                self.mac_check(c, msg, None, "status").await?;
                let (states, patterns): (Vec<String>, Vec<String>) = Self::args(msg)?;
                let v: Vec<(String, String)> = self
                    .list_unit_files()
                    .into_iter()
                    .filter(|(p, s)| {
                        let n = p.rsplit('/').next().unwrap_or(p);
                        (states.is_empty() || states.contains(s))
                            && (patterns.is_empty()
                                || patterns.iter().any(|pat| util::fnmatch(pat, n)))
                    })
                    .collect();
                self.reply(c, msg, &(v,)).await
            }
            "GetUnitFileState" => {
                self.mac_check(c, msg, None, "status").await?;
                let (name,): (String,) = Self::args(msg)?;
                let st =
                    install::unit_file_state(&self.cfg, &name).map_err(|e| err(E_NO_UNIT, e))?;
                let st = install::set_file_state(&self.cfg, &name, st);
                self.reply(c, msg, &(st,)).await
            }
            "EnableUnitFiles" | "ReenableUnitFiles" | "EnableUnitFilesWithFlags" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (files, runtime, force) = if member == "EnableUnitFilesWithFlags" {
                    let (f, flags): (Vec<String>, u64) = Self::args(msg)?;
                    (f, flags & FLAG_RUNTIME != 0, flags & FLAG_FORCE != 0)
                } else {
                    Self::args::<(Vec<String>, bool, bool)>(msg)?
                };
                let mut all = Vec::new();
                if member == "ReenableUnitFiles" {
                    all.extend(
                        install::disable(&self.cfg, &files, runtime)
                            .map_err(|e| err(E_FAILED, e))?,
                    );
                }
                let (info, ch) = install::enable(&self.cfg, &files, runtime, force)
                    .map_err(|e| err(E_FAILED, e))?;
                all.extend(ch);
                self.reply(c, msg, &(info, changes(all))).await?;
                self.file_op_done().await;
                Ok(())
            }
            "DisableUnitFiles"
            | "DisableUnitFilesWithFlags"
            | "DisableUnitFilesWithFlagsAndInstallInfo" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (files, runtime) = if member == "DisableUnitFiles" {
                    Self::args::<(Vec<String>, bool)>(msg)?
                } else {
                    let (f, flags): (Vec<String>, u64) = Self::args(msg)?;
                    (f, flags & FLAG_RUNTIME != 0)
                };
                let ch = changes(
                    install::disable(&self.cfg, &files, runtime).map_err(|e| err(E_FAILED, e))?,
                );
                if member == "DisableUnitFilesWithFlagsAndInstallInfo" {
                    self.reply(c, msg, &(true, ch)).await?;
                } else {
                    self.reply(c, msg, &(ch,)).await?;
                }
                self.file_op_done().await;
                Ok(())
            }
            "MaskUnitFiles" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (files, runtime, force): (Vec<String>, bool, bool) = Self::args(msg)?;
                let ch = install::mask(&self.cfg, &files, runtime, force)
                    .map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "UnmaskUnitFiles" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (files, runtime): (Vec<String>, bool) = Self::args(msg)?;
                let ch =
                    install::unmask(&self.cfg, &files, runtime).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "PresetUnitFiles" | "PresetUnitFilesWithMode" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (files, mode, runtime, force) = if member == "PresetUnitFiles" {
                    let (f, r, fo): (Vec<String>, bool, bool) = Self::args(msg)?;
                    (f, "full".to_string(), r, fo)
                } else {
                    Self::args::<(Vec<String>, String, bool, bool)>(msg)?
                };
                let (info, ch) = install::preset(&self.cfg, &files, &mode, runtime, force)
                    .map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(info, changes(ch))).await?;
                self.file_op_done().await;
                Ok(())
            }
            "PresetAllUnitFiles" => {
                self.mac_check(c, msg, None, "enable").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (mode, runtime, force): (String, bool, bool) = Self::args(msg)?;
                let names: Vec<String> = install::list_unit_files(&self.cfg)
                    .into_iter()
                    .filter(|(_, s)| s == "enabled" || s == "disabled")
                    .map(|(p, _)| p.rsplit('/').next().unwrap_or(&p).to_string())
                    .collect();
                let (_, ch) = install::preset(&self.cfg, &names, &mode, runtime, force)
                    .map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "GetDefaultTarget" => {
                self.mac_check(c, msg, None, "status").await?;
                let t = install::get_default_target(&self.cfg)
                    .ok_or_else(|| err(E_NO_UNIT, "No default target"))?;
                self.reply(c, msg, &(t,)).await
            }
            "SetDefaultTarget" => {
                self.mac_check(c, msg, None, "enable").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files")
                    .await?;
                let (name, _force): (String, bool) = Self::args(msg)?;
                let ch =
                    install::set_default_target(&self.cfg, &name).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "Dump" => {
                self.mac_check(c, msg, None, "status").await?;
                self.reply(
                    c,
                    msg,
                    &("uinitconverter does not implement Dump\n".to_string(),),
                )
                .await
            }
            _ => Err(err(
                E_UNKNOWN_METHOD,
                format!("Unknown method {member} or interface {MANAGER_IFACE}."),
            )),
        }
    }
}
