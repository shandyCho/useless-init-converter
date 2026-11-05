//! systemd's job side effects that s6-rc's dependency graph doesn't give:
//! units that conflict with a starting unit in either direction, stop
//! propagation along PartOf=, BindsTo= and Requires= (also across database
//! units and dynamic instances), restart propagation along PartOf=, and
//! StopWhenUnneeded=. The GNOME session's start and teardown rely on these
//! (work_history part 137).

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
        self.reverse_in(&self.reverse_index(&[key]), 0, id)
    }

    /// The loaded known units, each with the names that its `keys[k]=`
    /// (and that key's `.wants`/`.requires` entries) refers to. Built once
    /// per stop transaction: reverse() for each unit of the set and each
    /// key read every known unit's directories again, 0.5 s before a
    /// logout's first signal (work_history part 182).
    fn reverse_index(&self, keys: &[&str]) -> Vec<(String, Unit, Vec<BTreeSet<String>>)> {
        let ld = Loader::new(&self.cfg);
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for n in self.known_units() {
            let (uid, u) = self.load_unit(&n);
            if !u.is_loaded() || !seen.insert(uid.clone()) {
                continue;
            }
            let refs = keys
                .iter()
                .map(|key| {
                    let mut r: BTreeSet<String> = u.deps(key).into_iter().collect();
                    if let Some(kind) = dir_kind(key) {
                        r.extend(ld.dir_deps(&u, kind));
                    }
                    r
                })
                .collect();
            out.push((uid, u, refs));
        }
        out
    }

    /// The units of `index` whose `keys[k]=` names `id` (any of its names).
    fn reverse_in(
        &self,
        index: &[(String, Unit, Vec<BTreeSet<String>>)],
        k: usize,
        id: &str,
    ) -> Vec<(String, Unit)> {
        let (_, target) = self.load_unit(id);
        let mut names: BTreeSet<String> = target.names.iter().cloned().collect();
        names.insert(id.to_string());
        index
            .iter()
            .filter(|(uid, _, refs)| {
                !names.contains(uid) && refs[k].iter().any(|r| names.contains(r))
            })
            .map(|(uid, u, _)| (uid.clone(), u.clone()))
            .collect()
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
            let mut deps = u.deps("Requires");
            deps.extend(u.deps("Wants"));
            deps.extend(u.deps("BindsTo"));
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
    /// PartOf= here, BindsTo= in run_job() (found before the restart).
    /// systemd propagates restarts along Requires= too, which here would
    /// restart everything that needs dbus when dbus restarts; left out on
    /// purpose.
    pub(super) async fn propagate(&self, id: &str, jtype: &str, seen: &mut BTreeSet<String>) {
        let (keys, then): (&[&str], &str) = match jtype {
            "stop" => (&["PartOf", "BindsTo", "Requires"], "stop"),
            "restart" | "try-restart" => (&["PartOf"], "try-restart"),
            _ => return,
        };
        for key in keys {
            let t = std::time::Instant::now();
            let found = self.reverse(id, key);
            if t.elapsed() >= std::time::Duration::from_secs(1) {
                eprintln!(
                    "initconvertd: finding the {key}= units of {id} took {:.1} s",
                    t.elapsed().as_secs_f64()
                );
            }
            for (did, du) in found {
                if !seen.insert(did.clone()) || !self.is_up(&did, &du).await {
                    continue;
                }
                eprintln!("initconvertd: {then} {did} ({key}={id})");
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
                eprintln!("initconvertd: stopping {id} (StopWhenUnneeded=, not needed)");
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
                eprintln!("initconvertd: {id} sent STOPPING=1, stopping it");
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

    /// A user manager's stop, as systemd's stop transaction: the unit and,
    /// transitively, the running units that must go with it (Requires=,
    /// BindsTo=, Requisite= on it, and PartOf=), stopped together: a unit
    /// ordered after another (After=/Before=) stops first, the rest at
    /// once (stop_in_waves). One at a time and only the needing units first,
    /// a logout took seconds: gnome-shell ended while units PartOf=
    /// graphical-session.target (xdg-desktop-portal-gtk) still ran, and
    /// they failed on the closed display; units PartOf= a dependent
    /// (gnome-session-monitor.service) were not stopped at all.
    pub(super) async fn user_stop_transaction(&self, id: &str, unit: &Unit) -> &'static str {
        let mut set: Vec<(String, Unit)> = vec![(id.to_string(), unit.clone())];
        let mut seen: BTreeSet<String> = BTreeSet::from([id.to_string()]);
        const KEYS: [&str; 4] = ["Requires", "BindsTo", "Requisite", "PartOf"];
        let index = self.reverse_index(&KEYS);
        let mut i = 0;
        while i < set.len() {
            let cur = set[i].0.clone();
            i += 1;
            for k in 0..KEYS.len() {
                for (d, du) in self.reverse_in(&index, k, &cur) {
                    if seen.contains(&d) || self.dynamic_kind(&d).is_none() {
                        continue;
                    }
                    if self.is_up(&d, &du).await {
                        seen.insert(d.clone());
                        set.push((d, du));
                    }
                }
            }
        }
        if set.len() > 1 {
            let names: Vec<&str> = set[1..].iter().map(|(n, _)| n.as_str()).collect();
            eprintln!("initconvertd: stop {id} with {}", names.join(" "));
        }
        self.stop_in_waves(set, Some(id)).await.unwrap_or("done")
    }

    /// Stop `set` (each unit alone, dyn_stop_alone): a unit waits while a
    /// unit of the set ordered after it (After=/Before=) is still to stop or
    /// stopping, the others at once; an ordering cycle stops the rest
    /// together. Returns `top`'s result.
    pub(super) async fn stop_in_waves(
        &self,
        set: Vec<(String, Unit)>,
        top: Option<&str>,
    ) -> Option<&'static str> {
        // Ordering within the set, by canonical names.
        let after: BTreeMap<String, BTreeSet<String>> = set
            .iter()
            .map(|(n, u)| {
                let mut a: BTreeSet<String> = u
                    .deps("After")
                    .iter()
                    .map(|d| self.load_unit(d).0)
                    .collect();
                // X Before=Y is Y After=X.
                for (m, mu) in &set {
                    if mu.deps("Before").iter().any(|d| self.load_unit(d).0 == *n) {
                        a.insert(m.clone());
                    }
                }
                (n.clone(), a)
            })
            .collect();
        // Each unit as soon as no unit ordered after it is still pending or
        // stopping, as systemd's jobs; in whole waves, one slow unit held
        // back every later one (gnome-shell's stop came 0.5 s after the
        // services it uses had gone, work_history part 182).
        use futures_util::stream::{FuturesUnordered, StreamExt};
        let mut pending = set;
        let mut stopping: BTreeSet<String> = BTreeSet::new();
        let mut running = FuturesUnordered::new();
        let mut result = None;
        loop {
            let blocked = |n: &String, pending: &[(String, Unit)], stopping: &BTreeSet<String>| {
                pending
                    .iter()
                    .map(|(m, _)| m)
                    .chain(stopping.iter())
                    .any(|m| m != n && after.get(m).is_some_and(|a| a.contains(n)))
            };
            let mut ready: Vec<usize> = (0..pending.len())
                .filter(|&k| !blocked(&pending[k].0, &pending, &stopping))
                .collect();
            if ready.is_empty() && running.is_empty() {
                // An ordering cycle: the rest together.
                ready = (0..pending.len()).collect();
            }
            for k in ready.into_iter().rev() {
                let (n, u) = pending.remove(k);
                stopping.insert(n.clone());
                running.push(async move {
                    let t = std::time::Instant::now();
                    let r = self.dyn_stop_alone(&n, &u).await;
                    (n, r, t.elapsed())
                });
            }
            let Some((n, r, took)) = running.next().await else {
                break;
            };
            if took >= std::time::Duration::from_secs(1) {
                eprintln!(
                    "initconvertd: stopping {n} took {:.1} s",
                    took.as_secs_f64()
                );
            }
            if Some(n.as_str()) == top {
                result = Some(r);
            }
            stopping.remove(&n);
        }
        result
    }
}
