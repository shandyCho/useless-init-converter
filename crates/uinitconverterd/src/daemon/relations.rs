//! systemd's job side effects that s6-rc's dependency graph doesn't give:
//! units that conflict with a starting unit in either direction, stop
//! propagation along PartOf=, BindsTo= and Requires= (also across database
//! units and dynamic instances), restart propagation along PartOf=, and
//! StopWhenUnneeded=. The GNOME session's start and teardown rely on these
//! (work_history part 137).

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use zbus::zvariant::Value;

use super::Daemon;
use crate::loader::{Loader, Unit};

/// Relations whose reverse is also read from `.wants`/`.requires` dirs.
fn dir_kind(key: &str) -> Option<&'static str> {
    match key {
        "Wants" => Some("wants"),
        "Requires" => Some("requires"),
        _ => None,
    }
}

impl Daemon {
    /// Known units whose `key=` names `id` (any of its names).
    pub(super) fn reverse(&self, id: &str, key: &str) -> Vec<(String, Unit)> {
        let (_, target) = self.load_unit(id);
        let mut names: BTreeSet<String> = target.names.iter().cloned().collect();
        names.insert(id.to_string());
        let ld = Loader::new(&self.cfg);
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for n in self.known_units() {
            if !seen.insert(n.clone()) || names.contains(&n) {
                continue;
            }
            let (uid, u) = self.load_unit(&n);
            if names.contains(&uid) || !u.is_loaded() {
                continue;
            }
            let mut refs = u.get_words("Unit", key);
            if let Some(kind) = dir_kind(key) {
                refs.extend(ld.dir_deps(&u, kind));
            }
            if refs.iter().any(|r| names.contains(r)) {
                out.push((uid, u));
            }
        }
        out
    }

    /// A start pulls in the targets it requires or wants: those count as
    /// started again (see Daemon::stopped_targets).
    pub(super) fn unmark_stopped_targets(&self, id: &str) {
        if self.stopped_targets.lock().unwrap().is_empty() {
            return;
        }
        let ld = Loader::new(&self.cfg);
        let mut stack = vec![(id.to_string(), 0u32)];
        let mut seen = BTreeSet::new();
        while let Some((n, depth)) = stack.pop() {
            if depth > 8 || !seen.insert(n.clone()) {
                continue;
            }
            let (uid, u) = self.load_unit(&n);
            self.stopped_targets.lock().unwrap().remove(&uid);
            let mut deps = u.get_words("Unit", "Requires");
            deps.extend(u.get_words("Unit", "Wants"));
            deps.extend(u.get_words("Unit", "BindsTo"));
            deps.extend(ld.dir_deps(&u, "wants"));
            deps.extend(ld.dir_deps(&u, "requires"));
            stack.extend(deps.into_iter().map(|d| (d, depth + 1)));
        }
    }

    /// After a target's start: its direct Requires=/Wants= that s6-rc
    /// already had up but that are not active (a oneshot without
    /// RemainAfterExit= that ran, a service that exited) start again, as a
    /// target's start does under systemd. gnome-session-shutdown.target's
    /// Wants= runs at every logout, not only the first.
    /// `before`: s6-rc's active atoms before this start; what s6-rc brought
    /// up just now ran already.
    pub(super) async fn start_inactive_members(
        &self,
        id: &str,
        unit: &Unit,
        before: &BTreeSet<String>,
    ) {
        let ld = Loader::new(&self.cfg);
        let mut deps = unit.get_words("Unit", "Requires");
        deps.extend(unit.get_words("Unit", "Wants"));
        deps.extend(ld.dir_deps(unit, "wants"));
        deps.extend(ld.dir_deps(unit, "requires"));
        let mut seen = BTreeSet::new();
        for d in deps {
            let (did, du) = self.load_unit(&d);
            if did == id || !seen.insert(did.clone()) || !before.contains(&did) {
                continue;
            }
            if !matches!(
                self.kind_of(&did),
                Some(crate::gen::Kind::Oneshot | crate::gen::Kind::Longrun)
            ) {
                continue;
            }
            if !self.is_up(&did, &du).await {
                Box::pin(self.do_start(&did, &du, false)).await;
            }
        }
    }

    pub(super) async fn is_up(&self, id: &str, unit: &Unit) -> bool {
        let active = self.active_atoms().await;
        matches!(
            self.unit_status(id, &active, unit).await.active,
            "active" | "activating" | "reloading"
        )
    }

    /// After a stop or restart of `id`: the same for the units that depend
    /// on it that way. Stop: PartOf=, BindsTo=, Requires= (s6-rc already
    /// stops its own dependents; the others are found here). Restart:
    /// PartOf= only. systemd propagates restarts along Requires= too, which
    /// here would restart everything that needs dbus when dbus restarts;
    /// left out on purpose.
    pub(super) async fn propagate(&self, id: &str, jtype: &str, seen: &mut BTreeSet<String>) {
        let (keys, then): (&[&str], &str) = match jtype {
            "stop" => (&["PartOf", "BindsTo", "Requires"], "stop"),
            "restart" | "try-restart" => (&["PartOf"], "try-restart"),
            _ => return,
        };
        for key in keys {
            for (did, du) in self.reverse(id, key) {
                if !seen.insert(did.clone()) || !self.is_up(&did, &du).await {
                    continue;
                }
                eprintln!("uinitconverterd: {then} {did} ({key}={id})");
                let r = Box::pin(self.run_job_once(&did, &du, then, "replace")).await;
                if r == "done" {
                    Box::pin(self.propagate(&did, then, seen)).await;
                }
            }
        }
    }

    /// StopWhenUnneeded=: stop running units with it that no running unit
    /// needs (Requires=, Requisite=, Wants=, BindsTo=, or a .wants/.requires
    /// link) and that have no job of their own. Also one started explicitly,
    /// as systemd does.
    pub(super) async fn stop_unneeded(&self, depth: u32) {
        if depth > 4 {
            return;
        }
        let mut stopped = false;
        let mut seen = BTreeSet::new();
        for n in self.known_units() {
            if !seen.insert(n.clone()) {
                continue;
            }
            let (id, u) = self.load_unit(&n);
            if !u.get_bool("Unit", "StopWhenUnneeded", false) || !self.is_up(&id, &u).await {
                continue;
            }
            if self.jobs.lock().unwrap().values().any(|j| j.unit == id) {
                continue;
            }
            let mut needed = false;
            'keys: for key in ["Requires", "Requisite", "Wants", "BindsTo"] {
                for (nid, nu) in self.reverse(&id, key) {
                    if self.is_up(&nid, &nu).await {
                        needed = true;
                        break 'keys;
                    }
                }
            }
            if !needed {
                eprintln!("uinitconverterd: stopping {id} (StopWhenUnneeded=, not needed)");
                if Box::pin(self.run_job_once(&id, &u, "stop", "replace")).await == "done" {
                    Box::pin(self.propagate(&id, "stop", &mut BTreeSet::from([id.clone()]))).await;
                }
                stopped = true;
            }
        }
        // What a stop made unneeded in turn.
        if stopped {
            Box::pin(self.stop_unneeded(depth + 1)).await;
        }
    }

    /// PropertiesChanged (ActiveState, SubState) for a unit now. Targets
    /// call it from their start and stop; units with a runner are found by
    /// watch_states().
    pub(super) async fn emit_state(&self, id: &str) {
        let (_, unit) = self.load_unit(id);
        let active = self.active_atoms().await;
        let st = self.unit_status(id, &active, &unit).await;
        let mut changed: HashMap<String, Value<'static>> = HashMap::new();
        changed.insert("ActiveState".into(), Value::from(st.active.to_string()));
        changed.insert("SubState".into(), Value::from(st.sub.to_string()));
        self.emit(
            &super::unit_path(id),
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.systemd1.Unit",
                changed,
                Vec::<String>::new(),
            ),
        )
        .await;
    }

    /// State changes not caused by a job on that unit (a service that
    /// exited, a stop by Conflicts= or PartOf=): the runner's state files
    /// are replaced by rename, so a unit's directory mtime moves at every
    /// change. Each tick, the units whose directory changed get their state
    /// recomputed and, if it differs from the last one sent (`last`, shared
    /// with watch_dynamic), a PropertiesChanged. The leader of a GNOME
    /// session waits on exactly that for graphical-session-pre.target.
    pub(super) async fn watch_states(
        &self,
        mtimes: &mut HashMap<String, SystemTime>,
        last: &mut HashMap<String, (String, String)>,
    ) {
        let Ok(rd) = std::fs::read_dir(self.cfg.run_dir.join("units")) else {
            return;
        };
        let first = mtimes.is_empty();
        let mut changed = Vec::new();
        for e in rd.flatten() {
            let Ok(m) = e.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let name = e.file_name().to_string_lossy().into_owned();
            if mtimes.insert(name.clone(), m) != Some(m) && !first {
                changed.push(name);
            }
        }
        if changed.is_empty() {
            return;
        }
        let mut active = self.active_atoms().await;
        for name in changed {
            let (id, unit) = self.load_unit(&name);
            if !unit.is_loaded() || self.dynamic_kind(&id).is_some() {
                continue; // watch_dynamic reports those
            }
            if crate::state::for_unit(&self.cfg.run_dir, &id).take(crate::state::keys::STOPPING)
                && self.is_up(&id, &unit).await
            {
                eprintln!("uinitconverterd: {id} sent STOPPING=1, stopping it");
                let lock = self.unit_lock(&id);
                let _guard = lock.lock().await;
                Box::pin(self.run_job(&id, &unit, "stop", "replace")).await;
                active = self.active_atoms().await;
            }
            let st = self.unit_status(&id, &active, &unit).await;
            let cur = (st.active.to_string(), st.sub.to_string());
            if last.get(&id) == Some(&cur) {
                continue;
            }
            last.insert(id.clone(), cur);
            self.emit_state(&id).await;
        }
    }
}
