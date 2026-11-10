//! Routes D-Bus calls by object path and interface; org.freedesktop.systemd1.Unit methods.

use super::*;

impl Daemon {
    /// `only`: the one interface asked for ("" or None for all).
    pub(super) async fn object_props(&self, path: &str, only: Option<&str>) -> R<Ifaces> {
        let only = only.filter(|o| !o.is_empty());
        if path == MANAGER_PATH {
            let mut m = Ifaces::new();
            let nnames = self.known_units().len() as u32;
            let nfailed = self.count_failed().await;
            m.insert(MANAGER_IFACE.into(), self.manager_props(nfailed, nnames));
            return Ok(m);
        }
        if let Some(name) = self.unit_from_path(path) {
            let (id, unit) = self.load_unit(&name);
            let active = self.active_atoms().await;
            return Ok(self.unit_props(&id, &unit, &active, only).await);
        }
        if let Some(job) = path
            .strip_prefix(JOB_PREFIX)
            .and_then(|j| j.parse::<u32>().ok())
        {
            // A finished job's object is gone: systemd (sd-bus) answers
            // UnknownObject here, not NoSuchJob, and logind's
            // manager_job_is_active() only treats UnknownObject as "done";
            // anything else keeps the user in "closing" forever.
            let p = self
                .job_props(job)
                .ok_or_else(|| err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")))?;
            let mut m = Ifaces::new();
            m.insert("org.freedesktop.systemd1.Job".into(), p);
            return Ok(m);
        }
        Err(err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")))
    }

    pub(super) async fn properties(
        &self,
        c: &Peer,
        msg: &Message,
        path: &str,
        member: &str,
    ) -> R<()> {
        match member {
            "GetAll" => {
                let (iface,): (String,) = Self::args(msg)?;
                let mut all = self.object_props(path, Some(&iface)).await?;
                all.retain(|name, _| iface.is_empty() || *name == iface);
                self.reply(c, msg, &(super::props::ordered(all),)).await
            }
            "Get" => {
                let (iface, prop): (String, String) = Self::args(msg)?;
                let all = self.object_props(path, Some(&iface)).await?;
                for (name, props) in all {
                    if iface.is_empty() || iface == name {
                        if let Some(v) = props.get(&prop) {
                            return self.reply(c, msg, &(v,)).await;
                        }
                    }
                }
                Err(err(
                    E_UNKNOWN_PROPERTY,
                    format!("Unknown property {iface}.{prop}"),
                ))
            }
            "Set" => Err(err(
                E_READ_ONLY,
                "Properties are read-only in uinitconverter",
            )),
            _ => Err(err(E_UNKNOWN_METHOD, format!("Unknown method {member}"))),
        }
    }

    pub(super) async fn introspect(&self, c: &Peer, msg: &Message, path: &str) -> R<()> {
        let xml = match path {
            "/" => introspect::node("", &["org".into()]),
            "/org" => introspect::node("", &["freedesktop".into()]),
            "/org/freedesktop" => introspect::node("", &["systemd1".into()]),
            MANAGER_PATH => introspect::node(introspect::MANAGER, &["unit".into(), "job".into()]),
            "/org/freedesktop/systemd1/unit" => {
                let kids: Vec<String> = self
                    .known_units()
                    .iter()
                    .map(|u| util::bus_label_escape(u))
                    .collect();
                introspect::node("", &kids)
            }
            "/org/freedesktop/systemd1/job" => {
                let kids: Vec<String> = self
                    .jobs
                    .lock()
                    .unwrap()
                    .keys()
                    .map(|k| k.to_string())
                    .collect();
                introspect::node("", &kids)
            }
            p if p.starts_with(UNIT_PREFIX) => introspect::node(introspect::UNIT, &[]),
            p if p.starts_with(JOB_PREFIX) => introspect::node("", &[]),
            _ => return Err(err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'."))),
        };
        self.reply(c, msg, &(xml,)).await
    }

    pub(super) async fn handle_call(self: Arc<Self>, c: Peer, msg: Message) {
        let hdr = msg.header();
        let path = hdr
            .path()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let iface = hdr
            .interface()
            .map(|i| i.as_str().to_string())
            .unwrap_or_default();
        let member = hdr
            .member()
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        drop(hdr);
        let r = self.dispatch(&c, &msg, &path, &iface, &member).await;
        if let Err(e) = r {
            let _ = c.conn.reply_error(&msg.header(), e.name, &(e.msg,)).await;
        }
    }

    pub(super) async fn dispatch(
        &self,
        c: &Peer,
        msg: &Message,
        path: &str,
        iface: &str,
        member: &str,
    ) -> R<()> {
        let own;
        let path = if path == SELF_UNIT_PATH {
            let pid = self.caller_pid(c, msg).await.unwrap_or(0);
            own = self
                .unit_of_pid(pid)
                .map(|u| unit_path(&u))
                .ok_or_else(|| err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")))?;
            own.as_str()
        } else {
            path
        };
        // systemd's mac_selinux_filter: the built-in interfaces of every
        // object below the manager are checked against the manager.
        let builtin = matches!(
            iface,
            PROPS_IFACE | "org.freedesktop.DBus.Introspectable" | "org.freedesktop.DBus.Peer"
        );
        if builtin && access::path_below(path, MANAGER_PATH) {
            let perm = if iface == PROPS_IFACE && member == "Set" {
                "reload"
            } else {
                "status"
            };
            self.mac_check(c, msg, None, perm).await?;
        }
        match iface {
            PROPS_IFACE => return self.properties(c, msg, path, member).await,
            "org.freedesktop.DBus.Introspectable" => return self.introspect(c, msg, path).await,
            "org.freedesktop.DBus.Peer" => {
                return match member {
                    "Ping" => self.reply(c, msg, &()).await,
                    "GetMachineId" => self.reply(c, msg, &(util::machine_id(),)).await,
                    _ => Err(err(E_UNKNOWN_METHOD, member.to_string())),
                };
            }
            _ => {}
        }
        if path == MANAGER_PATH && iface == UINITCONVERTER_IFACE && member == "StopInstances" {
            // rc.shutdown, before s6-rc stops anything: the on-demand
            // instances are ordered after s6-rc services (user@ and
            // user-runtime-dir@ After=systemd-logind.service), which s6-rc
            // doesn't know. Stopped after logind, user@'s PAM close waited
            // 45 s on logind's varlink socket, per user.
            self.mac_check(c, msg, None, "halt").await?;
            self.require_privileged(c, msg).await?;
            if !self.cfg.run_dir.join("shutting-down").exists() {
                return Err(err(E_FAILED, "the system is not shutting down".to_string()));
            }
            self.stop_dynamic_at_shutdown().await;
            return self.reply(c, msg, &()).await;
        }
        if path == MANAGER_PATH && (iface.is_empty() || iface == MANAGER_IFACE) {
            return self.manager_method(c, msg, member).await;
        }
        if let Some(name) = self.unit_from_path(path) {
            if iface == "org.freedesktop.systemd1.Scope" && member == "Abandon" {
                self.mac_unit(c, msg, &name, "stop").await?;
                self.require_privileged(c, msg).await?;
                return self.reply(c, msg, &()).await;
            }
            if iface.is_empty() || iface == UNIT_IFACE {
                return self.unit_method(c, msg, &name, member).await;
            }
        }
        if let Some(job) = path
            .strip_prefix(JOB_PREFIX)
            .and_then(|j| j.parse::<u32>().ok())
        {
            if !self.jobs.lock().unwrap().contains_key(&job) {
                return Err(err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")));
            }
            if member == "Cancel" {
                if let Some(u) = self.job_unit(job) {
                    self.mac_unit(c, msg, &u, "stop").await?;
                }
                self.require_privileged(c, msg).await?;
                self.cancel_job(job)?;
                return self.reply(c, msg, &()).await;
            }
        }
        Err(err(
            E_UNKNOWN_METHOD,
            format!("Unknown method {member} or interface {iface}."),
        ))
    }

    pub(super) async fn unit_method(
        &self,
        c: &Peer,
        msg: &Message,
        name: &str,
        member: &str,
    ) -> R<()> {
        let jtype = match member {
            "Start" => "start",
            "Stop" => "stop",
            "Reload" => "reload",
            "Restart" => "restart",
            "Kill" => {
                let (whom, sig): (String, i32) = Self::args(msg)?;
                return self.kill_unit(c, msg, name, &whom, sig).await;
            }
            "ResetFailed" => {
                let id = self.index.read().unwrap().resolve(name).to_string();
                self.mac_unit(c, msg, &id, "reload").await?;
                self.require_unit(c, msg, &id, "reset-failed").await?;
                self.reset_failed(name);
                return self.reply(c, msg, &()).await;
            }
            "SetProperties" => {
                let (_runtime, props): (bool, Vec<(String, OwnedValue)>) = Self::args(msg)?;
                return self.set_unit_properties(c, msg, name, &props).await;
            }
            // Reference counting is accepted and ignored.
            "Ref" | "Unref" => {
                self.mac_unit(c, msg, name, "start").await?;
                self.require_privileged(c, msg).await?;
                return self.reply(c, msg, &()).await;
            }
            _ => return Err(err(E_UNKNOWN_METHOD, format!("Unknown method {member}"))),
        };
        let (mode,): (String,) = Self::args(msg)?;
        self.job_method(c, msg, name, &mode, jtype).await
    }

    pub(super) fn reset_failed(&self, name: &str) {
        let (id, _) = self.load_unit(name);
        let st = state::for_unit(&self.cfg.run_dir, &id);
        if st
            .get(keys::RESULT)
            .map(|r| result::is_failure(&r))
            .unwrap_or(false)
        {
            st.set(keys::RESULT, result::SUCCESS);
        }
        // A soft-failed start: inactive from now on, but still restarted by
        // the next start like any parked unit.
        if st.take(keys::START_FAILED) {
            st.set(keys::PARKED, "1");
        }
        st.clear(keys::RESTART_TIMES);
    }

    pub(super) async fn kill_unit(
        &self,
        c: &Peer,
        msg: &Message,
        name: &str,
        whom: &str,
        sig: i32,
    ) -> R<()> {
        Self::valid_name(name)?;
        let (id, unit) = self.load_unit(name);
        self.mac_check(c, msg, Some(&unit), "stop").await?;
        self.require_unit(c, msg, &id, "kill").await?;
        // The supervised pid is a wrapper for PAMName=, Type=forking and
        // MAINPID=; the runner records the real main process.
        let main = if self.kind_of(&id) == Some(Kind::Longrun) {
            self.svstat(&id)
                .await
                .map(|s| s.pid)
                .filter(|&p| p > 0)
                .map(|p| {
                    state::for_unit(&self.cfg.run_dir, &id)
                        .get(keys::MAIN_PID)
                        .and_then(|m| m.parse::<u32>().ok())
                        .filter(|&m| m > 1 && std::path::Path::new(&format!("/proc/{m}")).exists())
                        .unwrap_or(p)
                })
        } else {
            None
        };
        match whom {
            "main" => match main {
                Some(p) => unsafe {
                    libc::kill(p as i32, sig);
                },
                None => return Err(err(E_FAILED, format!("No main process to kill for {id}"))),
            },
            _ => {
                let dir = self.cgroup_dir_of(&id, &unit);
                if crate::cgroup::kill_all(&dir, sig, 0) == 0 {
                    if let Some(p) = main {
                        unsafe { libc::kill(p as i32, sig) };
                    }
                }
            }
        }
        self.reply(c, msg, &()).await
    }
}
