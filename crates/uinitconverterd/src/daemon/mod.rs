//! uinitconverterd: serves org.freedesktop.systemd1 on the system bus and
//! carries out requests with s6-rc. The real `systemctl`, libsystemd and other
//! D-Bus clients talk to it as if it were systemd.
//!
//! Dispatch is done by hand (no zbus ObjectServer) because systemd clients
//! rely on details such as Properties.GetAll("") returning every interface.

mod access;
mod devices;
mod dynamic;
mod introspect;
mod private;
mod props;
mod s6;
mod transient;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use futures_util::StreamExt;
use zbus::message::Type as MsgType;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, Message, MessageStream};

use crate::config::Config;
use crate::gen::{self, Index, Kind};
use crate::install;
use crate::loader::{LoadState, Loader, Unit};
use crate::state::{self, keys};
use crate::unit::UnitFile;
use crate::util;
use props::{Ifaces, Props, MANAGER_IFACE, UNIT_IFACE};

/// systemd version we claim API compatibility with.
pub const COMPAT_VERSION: &str = "259";

const BUS_NAME: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const UNIT_PREFIX: &str = "/org/freedesktop/systemd1/unit/";
const JOB_PREFIX: &str = "/org/freedesktop/systemd1/job/";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";

const E_NO_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const E_NO_JOB: &str = "org.freedesktop.systemd1.NoSuchJob";
const E_MASKED: &str = "org.freedesktop.systemd1.UnitMasked";
const E_LOAD_FAILED: &str = "org.freedesktop.systemd1.LoadFailed";
const E_NOT_APPLICABLE: &str = "org.freedesktop.systemd1.JobTypeNotApplicable";
const E_INACTIVE: &str = "org.freedesktop.systemd1.UnitInactive";
const E_ONLY_BY_DEP: &str = "org.freedesktop.systemd1.OnlyByDependency";
const E_NO_UNIT_FOR_PID: &str = "org.freedesktop.systemd1.NoUnitForPID";
const E_DESTRUCTIVE: &str = "org.freedesktop.systemd1.TransactionIsDestructive";
const E_ACCESS: &str = "org.freedesktop.DBus.Error.AccessDenied";
const E_UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const E_UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";
const E_UNKNOWN_PROPERTY: &str = "org.freedesktop.DBus.Error.UnknownProperty";
const E_INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const E_READ_ONLY: &str = "org.freedesktop.DBus.Error.PropertyReadOnly";
const E_NOT_SUPPORTED: &str = "org.freedesktop.DBus.Error.NotSupported";
const E_FAILED: &str = "org.freedesktop.DBus.Error.Failed";
use uinitconverter_client::UINITCONVERTER_IFACE;

pub struct Job {
    pub id: u32,
    pub unit: String,
    pub jtype: String,
    /// "waiting" (queued behind another job on the same unit) or "running"
    pub state: &'static str,
    pub canceled: bool,
}

/// The connection a request arrived on: the system bus, or a peer-to-peer
/// connection on the private socket (which carries the peer's credentials).
#[derive(Clone)]
pub struct Peer {
    pub conn: Connection,
    pub uid: Option<u32>,
    pub pid: Option<u32>,
    /// SO_PEERSEC of a private socket peer (bus callers: asked of the bus).
    pub label: Option<String>,
}

#[derive(Clone)]
pub struct Transient {
    /// "scope" or "slice"
    pub kind: &'static str,
    pub cgroup: PathBuf,
    pub send_sighup: bool,
    pub slice: String,
    pub description: String,
}

pub struct Daemon {
    pub cfg: Config,
    conn: Connection,
    pub index: RwLock<Index>,
    pub jobs: Mutex<BTreeMap<u32, Job>>,
    pub next_job: AtomicU32,
    /// The s6 backend's locks and caches (see s6.rs).
    pub s6: s6::State,
    pub started_targets: Mutex<BTreeSet<String>>,
    pub transient: Mutex<BTreeMap<String, Transient>>,
    pub manager_env: Mutex<Vec<String>>,
    uid_cache: Mutex<HashMap<String, u32>>,
    /// Bus callers' SELinux labels, by unique name (see access.rs).
    label_cache: Mutex<HashMap<String, String>>,
    pub started: (u64, u64),
    pub boot_done: AtomicBool,
    pub boot_finished: Mutex<Option<(u64, u64)>>,
    /// Peer-to-peer connections on the private socket (signals go to these too).
    pub private: Mutex<Vec<(u64, Connection)>>,
    /// Template instances started on demand (see dynamic.rs).
    pub dynamic: Mutex<BTreeMap<String, dynamic::Dynamic>>,
    /// Unit markers (needs-restart, needs-reload) set by package scriptlets
    /// through SetUnitProperties, acted on by EnqueueMarkedJobs.
    pub markers: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// Jobs on the same unit run one after another.
    unit_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Parsed unit files (cleared by Reload and unit file operations).
    pub unit_cache: Mutex<HashMap<String, Unit>>,
    /// UnitFileState values (cleared like unit_cache, and when the s6 set
    /// changes: see file_state_cache_check()).
    pub file_state_cache: Mutex<HashMap<String, String>>,
    /// The s6 set's directories' mtimes the cache was filled under.
    file_state_set_stamp: Mutex<Vec<Option<std::time::SystemTime>>>,
    pub fdstore: crate::fdstore::Store,
}

struct DErr {
    name: &'static str,
    msg: String,
}

type R<T> = Result<T, DErr>;

fn err(name: &'static str, msg: impl Into<String>) -> DErr {
    DErr { name, msg: msg.into() }
}

fn zerr(e: zbus::Error) -> DErr {
    err(E_FAILED, e.to_string())
}

pub fn unit_path(name: &str) -> String {
    format!("{UNIT_PREFIX}{}", util::bus_label_escape(name))
}

pub fn job_path(id: u32) -> String {
    format!("{JOB_PREFIX}{id}")
}

/// Apply a Markers= assignment with systemd's syntax: "+m"/"-m" add or
/// remove single markers, a plain list replaces them all.
fn apply_markers(mut cur: BTreeSet<String>, words: &[String]) -> Result<BTreeSet<String>, String> {
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

/// Targets/services that mean "shut the machine down" (logind starts these).
/// The service an Accept=no socket unit activates.
fn socket_service(id: &str, unit: &Unit) -> Option<String> {
    if !id.ends_with(".socket") || unit.get_bool("Socket", "Accept", false) {
        return None;
    }
    Some(unit.get("Socket", "Service").unwrap_or_else(|| format!("{}.service", id.trim_end_matches(".socket"))))
}

fn shutdown_action(id: &str) -> Option<&'static str> {
    match id {
        "poweroff.target" | "systemd-poweroff.service" => Some("poweroff"),
        "reboot.target" | "systemd-reboot.service" | "ctrl-alt-del.target" | "kexec.target"
        | "systemd-kexec.service" | "soft-reboot.target" | "systemd-soft-reboot.service" => Some("reboot"),
        "halt.target" | "systemd-halt.service" => Some("halt"),
        _ => None,
    }
}

/// AttachProcessesToUnit's subcgroup, relative to the unit's cgroup, as
/// systemd's path_is_normalized() takes it: a leading or trailing '/' is
/// fine, but no ".", ".." or empty component, which could leave the unit's
/// cgroup. None if it isn't normalized.
fn subcgroup(sub: &str) -> Option<&str> {
    let rel = sub.trim_start_matches('/').trim_end_matches('/');
    if rel.is_empty() {
        return Some("");
    }
    rel.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\0')).then_some(rel)
}

/// The unit a cgroup belongs to, as the system manager sees it: the
/// outermost .service or .scope (below it is a delegated tree, such as
/// user@1000.service/app.slice/x.service), else the innermost slice.
fn unit_of_cgroup(cg: &str) -> Option<String> {
    let parts: Vec<&str> = cg.split('/').filter(|c| !c.is_empty()).collect();
    parts
        .iter()
        .find(|c| c.ends_with(".service") || c.ends_with(".scope"))
        .or_else(|| parts.iter().rev().find(|c| c.ends_with(".slice")))
        .map(|c| c.to_string())
}

/// PIDs behind the pidfds of a PIDFDs (ah) property.
fn pids_from_pidfds(v: &OwnedValue) -> Vec<u32> {
    use std::os::fd::AsRawFd;
    let mut out = Vec::new();
    if let Value::Array(a) = &**v {
        for item in a.iter() {
            if let Value::Fd(fd) = item {
                let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).unwrap_or_default();
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

fn op(p: String) -> ObjectPath<'static> {
    ObjectPath::try_from(p).unwrap_or_else(|_| ObjectPath::try_from("/").unwrap())
}

type UnitEntry = (
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

type Changes = Vec<(String, String, String)>;

fn changes(c: Vec<install::Change>) -> Changes {
    c.into_iter().map(|c| (c.kind, c.path, c.source)).collect()
}

const FLAG_RUNTIME: u64 = 1;
const FLAG_FORCE: u64 = 2;

impl Daemon {
    fn args<T>(msg: &Message) -> R<T>
    where
        T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        msg.body()
            .deserialize::<T>()
            .map_err(|e| err(E_INVALID_ARGS, e.to_string()))
    }

    async fn reply<B>(&self, c: &Peer, msg: &Message, body: &B) -> R<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        c.conn.reply(&msg.header(), body).await.map_err(zerr)
    }

    async fn caller_uid(&self, c: &Peer, msg: &Message) -> Option<u32> {
        if c.uid.is_some() {
            return c.uid;
        }
        let hdr = msg.header();
        let sender = hdr.sender()?.to_string();
        if let Some(u) = self.uid_cache.lock().unwrap().get(&sender) {
            return Some(*u);
        }
        let proxy = zbus::fdo::DBusProxy::new(&self.conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender.clone()).ok()?;
        let uid = proxy.get_connection_unix_user(name).await.ok()?;
        let mut cache = self.uid_cache.lock().unwrap();
        // Unique names are never reused, but they pile up: keep it bounded.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(sender, uid);
        Some(uid)
    }

    async fn caller_pid(&self, c: &Peer, msg: &Message) -> Option<u32> {
        if c.pid.is_some() {
            return c.pid;
        }
        let hdr = msg.header();
        let sender = hdr.sender()?.to_string();
        let proxy = zbus::fdo::DBusProxy::new(&self.conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender).ok()?;
        proxy.get_connection_unix_process_id(name).await.ok()
    }

    async fn require_privileged(&self, c: &Peer, msg: &Message) -> R<()> {
        self.authorize_with(c, msg, "org.freedesktop.systemd1.manage-units", HashMap::new()).await
    }

    /// manage-units for one unit, with the details systemd passes to polkit:
    /// rules such as `action.lookup("unit") == "foo.service"` match on them.
    async fn require_unit(&self, c: &Peer, msg: &Message, unit: &str, verb: &str) -> R<()> {
        let details = HashMap::from([
            ("unit".to_string(), unit.to_string()),
            ("verb".to_string(), verb.to_string()),
            ("polkit.message".to_string(), format!("Authentication is required to {verb} '{unit}'.")),
            ("polkit.gettext_domain".to_string(), "systemd".to_string()),
        ]);
        self.authorize_with(c, msg, "org.freedesktop.systemd1.manage-units", details).await
    }

    async fn authorize(&self, c: &Peer, msg: &Message, action: &str) -> R<()> {
        self.authorize_with(c, msg, action, HashMap::new()).await
    }

    /// Root and ALLOW_UIDS pass; other callers on the bus are checked with
    /// polkit using systemd's action IDs (the policy ships with systemd).
    async fn authorize_with(&self, c: &Peer, msg: &Message, action: &str, details: HashMap<String, String>) -> R<()> {
        match self.caller_uid(c, msg).await {
            Some(0) => return Ok(()),
            Some(u) if self.cfg.allow_uids.contains(&u) => return Ok(()),
            None => return Err(err(E_ACCESS, "Access denied")),
            Some(_) => {}
        }
        // Peer-to-peer callers are root-only; polkit needs a bus name.
        let hdr = msg.header();
        let Some(sender) = hdr.sender().map(|s| s.to_string()).filter(|_| c.uid.is_none()) else {
            return Err(err(E_ACCESS, "Access denied"));
        };
        let interactive = hdr.primary().flags().contains(zbus::message::Flags::AllowInteractiveAuth);
        drop(hdr);
        let mut subject_details: HashMap<String, Value<'static>> = HashMap::new();
        subject_details.insert("name".into(), Value::from(sender));
        let subject = ("system-bus-name".to_string(), subject_details);
        let reply = self
            .conn
            .call_method(
                Some("org.freedesktop.PolicyKit1"),
                "/org/freedesktop/PolicyKit1/Authority",
                Some("org.freedesktop.PolicyKit1.Authority"),
                "CheckAuthorization",
                &(subject, action, details, u32::from(interactive), ""),
            )
            .await;
        let Ok(reply) = reply else {
            // No polkit: fail closed.
            return Err(err(E_ACCESS, "Access denied"));
        };
        match reply.body().deserialize::<((bool, bool, HashMap<String, String>),)>() {
            Ok(((true, _, _),)) => Ok(()),
            Ok(((false, true, _),)) => Err(err(
                "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
                "Interactive authentication required.",
            )),
            _ => Err(err(E_ACCESS, "Access denied")),
        }
    }

    /// Resolve aliases and load the unit file.
    fn load_unit(&self, name: &str) -> (String, Unit) {
        let resolved = self.index.read().unwrap().resolve(name).to_string();
        if let Some(t) = self.transient.lock().unwrap().get(&resolved) {
            let mut file = UnitFile::default();
            file.entries.push(("Unit".into(), "Description".into(), t.description.clone()));
            return (
                resolved.clone(),
                Unit {
                    id: resolved.clone(),
                    names: vec![resolved],
                    fragment: None,
                    dropins: Vec::new(),
                    load_state: LoadState::Loaded,
                    load_error: None,
                    file,
                },
            );
        }
        if let Some(u) = self.unit_cache.lock().unwrap().get(&resolved) {
            return (u.id.clone(), u.clone());
        }
        let unit = Loader::new(&self.cfg).load(&resolved);
        // Not-found units are re-read every time (systemd retries them too).
        if unit.load_state != LoadState::NotFound {
            self.unit_cache.lock().unwrap().insert(resolved, unit.clone());
        }
        (unit.id.clone(), unit)
    }

    /// UnitFileState follows the s6 set, which `s6 set enable|disable`
    /// changes without telling the daemon: drop the cached states when a
    /// directory of the set changed.
    pub(super) fn file_state_cache_check(&self) {
        let set = self.cfg.repo_dir.join("sources").join(&self.cfg.set_name);
        let stamp: Vec<_> = ["", "always", "active", "usable", "masked"]
            .iter()
            .map(|d| std::fs::metadata(set.join(d)).and_then(|m| m.modified()).ok())
            .collect();
        let mut last = self.file_state_set_stamp.lock().unwrap();
        if *last != stamp {
            *last = stamp;
            self.file_state_cache.lock().unwrap().clear();
        }
    }

    pub(super) fn invalidate_caches(&self) {
        self.unit_cache.lock().unwrap().clear();
        self.file_state_cache.lock().unwrap().clear();
        self.s6.clear_caches();
    }

    /// The unit an object path names. Checked after unescaping: `_2e_2e_2f`
    /// decodes to "../", and an invalid name stays the unit's id, which
    /// names its state directory (RUN_DIR/units/ID): any caller could have
    /// made the daemon read files from other directories and return them
    /// as properties.
    fn unit_from_path(&self, path: &str) -> Option<String> {
        let label = path.strip_prefix(UNIT_PREFIX)?;
        if label.contains('/') {
            return None;
        }
        let name = util::bus_label_unescape(label);
        crate::loader::parse_name(&name)?;
        Some(name)
    }

    /// For methods that take a unit name, as systemd answers them.
    fn valid_name(name: &str) -> R<()> {
        match crate::loader::parse_name(name) {
            Some(_) => Ok(()),
            None => Err(err(E_INVALID_ARGS, format!("Unit name {name} is not valid."))),
        }
    }

    async fn emit(&self, path: &str, iface: &str, member: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) {
        if let Err(e) = self.conn.emit_signal(None::<&str>, path, iface, member, body).await {
            eprintln!("uinitconverterd: emit {member}: {e}");
        }
        let peers: Vec<(u64, Connection)> = self.private.lock().unwrap().clone();
        for (id, p) in peers {
            // A peer that went away is dropped by its own reader task. One
            // that stopped reading (its socket buffer is full) would block
            // every later signal and so every job: drop it, as systemd drops
            // a client whose queue overflows. It reconnects when it needs to.
            let send = p.emit_signal(None::<&str>, path, iface, member, body);
            if tokio::time::timeout(std::time::Duration::from_millis(500), send).await.is_err() {
                eprintln!("uinitconverterd: a private-socket client is not reading its messages; disconnecting it");
                self.private.lock().unwrap().retain(|(i, _)| *i != id);
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), p.close()).await;
                });
            }
        }
    }

    async fn reload_db(&self) -> Result<(), String> {
        self.emit(MANAGER_PATH, MANAGER_IFACE, "Reloading", &(true,)).await;
        let cfg = self.cfg.clone();
        let (plan, moving) = tokio::task::spawn_blocking(move || {
            let plan = gen::plan(&cfg);
            let moving = gen::auto_mask_moving(&cfg, &plan);
            (plan, moving)
        })
        .await
        .map_err(|e| e.to_string())?;
        // AUTO_MASK= moves these into or out of the database: no job runs
        // on them until the new database and index are in place (a start
        // from the old database would be stopped by s6-rc-update, a dynamic
        // instance would meet the database's service of the same name).
        // Taken before s6.lock, as jobs do.
        let unit_guards = self.lock_units(&moving).await;
        // A dynamic instance of one going back to the database: stop it now
        // and start it from the database after the update.
        let mut restart = Vec::new();
        for id in &moving {
            if self.dynamic_kind(id).is_none() {
                continue;
            }
            let before = self.active_dynamic().await;
            let (_, unit) = self.load_unit(id);
            self.dyn_stop(id, &unit).await;
            let after = self.active_dynamic().await;
            restart.extend(before.difference(&after).cloned());
        }
        if !restart.is_empty() {
            eprintln!("uinitconverterd: {}: back to the database, restarted from it", restart.join(" "));
        }
        let cfg = self.cfg.clone();
        let res = {
            let _guard = self.s6.lock.lock().await;
            // s6-rc-update must see those units as up, or it keeps them down.
            self.reconcile_beside_locked().await;
            let res = tokio::task::spawn_blocking(move || gen::regenerate_plan(&cfg, plan, true))
                .await
                .map_err(|e| e.to_string())?;
            // Before s6.lock and the units' locks go: a job must find each
            // unit where the new database has it.
            *self.index.write().unwrap() = Index::load(&self.cfg);
            res
        };
        drop(unit_guards);
        if let Ok(r) = &res {
            eprintln!(
                "uinitconverterd: reloaded: {} units, database {}",
                r.plan.units.len(),
                r.db_dir.display()
            );
            for c in &r.set_changes {
                eprintln!("uinitconverterd: set {}: {c}", self.cfg.set_name);
            }
        }
        self.invalidate_caches();
        self.s6.database_changed();
        for id in &restart {
            let r = self.start_any(id).await;
            if r != "done" {
                eprintln!("uinitconverterd: {id}: start from the database: {r}");
            }
        }
        self.emit(MANAGER_PATH, MANAGER_IFACE, "Reloading", &(false,)).await;
        res.map(|_| ())
    }

    /// The locks of all `ids`, taken without waiting while holding some: a
    /// job holding one may be starting a dependency whose lock we'd hold.
    async fn lock_units(&self, ids: &BTreeSet<String>) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        loop {
            let guards: Vec<_> = ids.iter().map_while(|id| self.unit_lock(id).try_lock_owned().ok()).collect();
            if guards.len() == ids.len() {
                return guards;
            }
            drop(guards);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Dynamic instances that are up: longruns whose supervisor says so,
    /// active oneshots.
    async fn active_dynamic(&self) -> BTreeSet<String> {
        let all: Vec<(String, Kind, bool)> =
            self.dynamic.lock().unwrap().iter().map(|(n, d)| (n.clone(), d.kind, d.active)).collect();
        let mut up = BTreeSet::new();
        for (id, kind, active) in all {
            let on = if kind == Kind::Oneshot { active } else { self.svstat(&id).await.map(|s| s.up).unwrap_or(false) };
            if on {
                up.insert(id);
            }
        }
        up
    }

    // ---- jobs ----

    fn new_job(&self, unit: &str, jtype: &str) -> u32 {
        let id = self.next_job.fetch_add(1, Ordering::SeqCst) + 1;
        self.jobs
            .lock()
            .unwrap()
            .insert(id, Job { id, unit: unit.into(), jtype: jtype.into(), state: "waiting", canceled: false });
        id
    }

    /// Validate a job request; returns the canonical unit id. `manual`: asked
    /// for by a client, so RefuseManualStart=/RefuseManualStop= apply.
    async fn check_job(&self, name: &str, jtype: &str, allow_reload: bool, manual: bool) -> R<(String, Unit)> {
        let (id, unit) = self.load_unit(name);
        if jtype == "start" && shutdown_action(&id).is_some() {
            return Ok((id, unit));
        }
        match unit.load_state {
            LoadState::NotFound => return Err(err(E_NO_UNIT, format!("Unit {name} not found."))),
            LoadState::Masked => return Err(err(E_MASKED, format!("Unit {name} is masked."))),
            LoadState::BadSetting => {
                return Err(err(E_LOAD_FAILED, unit.load_error.clone().unwrap_or_default()))
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
                format!("Transaction for {id}/{jtype} is destructive (the system is shutting down)."),
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
            return Err(err(E_NOT_APPLICABLE, format!("Job type reload is not applicable for unit {name}.")));
        }
        // As systemd, a reload behind a queued or running start-type job is
        // accepted and runs after it (jobs on a unit are serialized);
        // unit_status() would say "activating" for that job and refuse.
        if jtype == "reload" {
            let behind_job = self.jobs.lock().unwrap().values().any(|j| j.unit == id && j.jtype != "stop");
            let active = self.active_atoms().await;
            if !behind_job && self.unit_status(&id, &active, &unit).await.active != "active" {
                return Err(err(E_INACTIVE, format!("Unit {name} is not active.")));
            }
        }
        let transient = self.transient.lock().unwrap().contains_key(&id);
        let dynamic = self.dynamic_kind(&id).is_some() || self.can_be_dynamic(&id, &unit);
        if !transient && !dynamic && self.kind_of(&id).is_none() && jtype != "stop" {
            // Masked in the s6 set by the admin (AUTO_MASK='s are dynamic).
            if crate::install::set_rx(&self.cfg, &id) == Some("masked") {
                return Err(err(E_MASKED, format!("Unit {name} is masked in the s6 set ({}).", self.cfg.set_name)));
            }
            // New unit file since the last reload: translate it now (not for
            // bus activation, which any user can trigger).
            if allow_reload && !self.cfg.is_excluded(&id) {
                let _ = self.reload_db().await;
            }
            // AUTO_MASK= may have left it out of the database: on demand.
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
                return Err(err(E_LOAD_FAILED, format!("Unit {name} cannot be managed: {why}.")));
            }
        }
        Ok((id, unit))
    }

    async fn run_job(&self, id: &str, unit: &Unit, jtype: &str, mode: &str) -> &'static str {
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
                let _ = tokio::process::Command::new("/bin/sh").arg("-c").arg(cmd).spawn();
                return "done";
            }
        }
        if self.dynamic_kind(id).is_some() || self.can_be_dynamic(id, unit) {
            // Not unit_status(): it reports this very job as "activating", so
            // try-restart would start an inactive instance (user@UID after
            // logout).
            let active = || async {
                match self.dynamic_kind(id) {
                    Some(Kind::Oneshot) => self.dynamic.lock().unwrap().get(id).map(|d| d.active).unwrap_or(false),
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
                    self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitRemoved", &(id.to_string(), op(unit_path(id)))).await;
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
    async fn reload_if_active(&self, id: &str, unit: &Unit) -> &'static str {
        let active = self.active_atoms().await;
        if !matches!(self.unit_status(id, &active, unit).await.active, "active" | "reloading") {
            return "invalid";
        }
        self.do_reload(id).await
    }

    async fn finish_job(&self, job: u32, id: &str, result: &str) {
        self.jobs.lock().unwrap().remove(&job);
        self.forget_unit_state(id);
        self.emit(MANAGER_PATH, MANAGER_IFACE, "JobRemoved", &(job, op(job_path(job)), id.to_string(), result.to_string()))
            .await;
        let (_, unit) = self.load_unit(id);
        let active = self.active_atoms().await;
        let st = self.unit_status(id, &active, &unit).await;
        let mut changed: HashMap<String, Value<'static>> = HashMap::new();
        changed.insert("ActiveState".into(), Value::from(st.active.to_string()));
        changed.insert("SubState".into(), Value::from(st.sub.to_string()));
        self.emit(&unit_path(id), PROPS_IFACE, "PropertiesChanged", &(UNIT_IFACE, changed, Vec::<String>::new()))
            .await;
    }

    pub(super) fn unit_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.unit_locks.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    /// Run a registered job: wait for earlier jobs on the same unit, honour
    /// CancelJob while waiting, then emit JobRemoved.
    async fn execute_job(&self, job: u32, id: &str, unit: &Unit, jtype: &str, mode: &str) -> &'static str {
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
        let result = if canceled { "canceled" } else { self.run_job(id, unit, jtype, mode).await };
        self.finish_job(job, id, result).await;
        result
    }

    /// Reply with the job path; if the caller is already gone, drop the job
    /// instead of leaving it registered forever.
    async fn reply_job(&self, c: &Peer, msg: &Message, job: u32) -> R<()> {
        if let Err(e) = self.reply(c, msg, &(op(job_path(job)),)).await {
            self.jobs.lock().unwrap().remove(&job);
            return Err(e);
        }
        Ok(())
    }

    async fn job_method(&self, c: &Peer, msg: &Message, name: &str, mode: &str, jtype: &str) -> R<()> {
        Self::valid_name(name)?;
        let id = self.index.read().unwrap().resolve(name).to_string();
        self.mac_unit(c, msg, &id, access::job_access(jtype)).await?;
        self.require_unit(c, msg, &id, jtype).await?;
        let (id, unit) = self.check_job(name, jtype, true, true).await?;
        let job = self.new_job(&id, jtype);
        self.reply_job(c, msg, job).await?;
        self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(job, op(job_path(job)), id.clone())).await;
        self.execute_job(job, &id, &unit, jtype, mode).await;
        Ok(())
    }

    /// Runtime property changes: only Markers= is kept (package scriptlets
    /// mark units for EnqueueMarkedJobs); the others are accepted and ignored.
    async fn set_unit_properties(&self, c: &Peer, msg: &Message, name: &str, props: &[(String, OwnedValue)]) -> R<()> {
        Self::valid_name(name)?;
        let id = self.index.read().unwrap().resolve(name).to_string();
        self.mac_unit(c, msg, &id, "start").await?;
        self.require_unit(c, msg, &id, "set-property").await?;
        for (key, v) in props {
            if key != "Markers" {
                continue;
            }
            let words: Vec<String> = match &**v {
                Value::Array(a) => a.iter().filter_map(|x| <&str>::try_from(x).ok().map(String::from)).collect(),
                _ => return Err(err(E_INVALID_ARGS, "Markers= takes a string array")),
            };
            let (id, unit) = self.load_unit(name);
            if unit.load_state == LoadState::NotFound {
                return Err(err(E_NO_UNIT, format!("Unit {name} not found.")));
            }
            let mut all = self.markers.lock().unwrap();
            let m = apply_markers(all.get(&id).cloned().unwrap_or_default(), &words).map_err(|e| err(E_INVALID_ARGS, e))?;
            if m.is_empty() {
                all.remove(&id);
            } else {
                all.insert(id, m);
            }
        }
        self.reply(c, msg, &()).await
    }

    /// Like systemd's: try-restart the units marked needs-restart, reload the
    /// active ones marked only needs-reload, and clear the markers. Package
    /// updates restart their services this way (systemd-update-helper).
    async fn enqueue_marked(&self, c: &Peer, msg: &Message) -> R<()> {
        self.mac_check(c, msg, None, "start").await?;
        self.require_privileged(c, msg).await?;
        let marked = std::mem::take(&mut *self.markers.lock().unwrap());
        let mut queued = Vec::new();
        for (name, m) in marked {
            let jtype = if m.contains("needs-restart") { "try-restart" } else { "reload" };
            // As systemd: a unit the caller may not act on is skipped.
            if let Err(e) = self.mac_unit(c, msg, &name, access::job_access(jtype)).await {
                eprintln!("uinitconverterd: {name}: failed to enqueue marked job: {}", e.msg);
                continue;
            }
            match self.check_job(&name, jtype, true, false).await {
                Ok((id, unit)) => queued.push((self.new_job(&id, jtype), id, unit, jtype)),
                // A reload of an inactive unit is a no-op for systemd too.
                Err(e) if e.name == E_INACTIVE => {}
                Err(e) => eprintln!("uinitconverterd: marked {name}: {}", e.msg),
            }
        }
        let paths: Vec<ObjectPath<'static>> = queued.iter().map(|(job, ..)| op(job_path(*job))).collect();
        if let Err(e) = self.reply(c, msg, &(paths,)).await {
            let mut jobs = self.jobs.lock().unwrap();
            for (job, ..) in &queued {
                jobs.remove(job);
            }
            return Err(e);
        }
        for (job, id, ..) in &queued {
            self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(*job, op(job_path(*job)), id.clone())).await;
        }
        futures_util::future::join_all(
            queued.iter().map(|(job, id, unit, jtype)| self.execute_job(*job, id, unit, jtype, "fail")),
        )
        .await;
        Ok(())
    }

    /// dbus-daemon --systemd-activation asks us to start a service.
    async fn handle_activation(&self, name: String) {
        let res = match self.check_job(&name, "start", false, true).await {
            Ok((id, unit)) => {
                let job = self.new_job(&id, "start");
                self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(job, op(job_path(job)), id.clone())).await;
                let r = self.execute_job(job, &id, &unit, "start", "replace").await;
                if r == "done" {
                    Ok(())
                } else {
                    Err(err(E_FAILED, format!("Unit {name} failed to start ({r}).")))
                }
            }
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            eprintln!("uinitconverterd: activation of {name} failed: {}", e.msg);
            let r = self
                .conn
                .emit_signal(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.systemd1.Activator",
                    "ActivationFailure",
                    &(name, e.name.to_string(), e.msg),
                )
                .await;
            if let Err(e) = r {
                eprintln!("uinitconverterd: ActivationFailure: {e}");
            }
        }
    }

    // ---- transient scopes and slices ----

    /// Stop a transient unit. Scopes: SIGTERM (+SIGHUP), SIGKILL after a grace
    /// period. Slices: stop everything inside, then remove the cgroup.
    async fn stop_transient(&self, id: &str) {
        let Some((kind, dir, hup)) = self
            .transient
            .lock()
            .unwrap()
            .get(id)
            .map(|t| (t.kind, t.cgroup.clone(), t.send_sighup))
        else {
            return;
        };
        if dir == std::path::Path::new(crate::cgroup::CGROUP_ROOT) {
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
                self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitRemoved", &(n.clone(), op(unit_path(&n)))).await;
            }
            let scopes: Vec<String> = self
                .transient
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, t)| t.kind == "scope" && t.cgroup.starts_with(&dir) && n.as_str() != id)
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
            self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitRemoved", &(id.to_string(), op(unit_path(id)))).await;
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

    async fn start_transient(&self, c: &Peer, msg: &Message) -> R<()> {
        self.mac_check(c, msg, None, "start").await?;
        self.require_privileged(c, msg).await?;
        type Aux = Vec<(String, Vec<(String, OwnedValue)>)>;
        let (name, mode, props, aux): (String, String, Vec<(String, OwnedValue)>, Aux) = Self::args(msg)?;
        // The name becomes a cgroup and a state directory: as for the job
        // methods, only valid unit names (no '/', no "..").
        Self::valid_name(&name)?;
        if name.ends_with(".service") || name.ends_with(".timer") || name.ends_with(".path") {
            return self.start_transient_file(c, msg, &name, &mode, &props, &aux).await;
        }
        let kind: &'static str = if name.ends_with(".scope") {
            "scope"
        } else if name.ends_with(".slice") {
            if crate::cgroup::slice_path(&name).as_os_str().is_empty() {
                return Err(err(E_INVALID_ARGS, "The root slice cannot be created as a transient unit."));
            }
            "slice"
        } else {
            return Err(err(
                E_NOT_SUPPORTED,
                format!("Transient units of this type are not supported by uinitconverter: {name}"),
            ));
        };
        if self.transient.lock().unwrap().contains_key(&name) {
            return Err(err("org.freedesktop.systemd1.UnitExists", format!("Unit {name} already exists.")));
        }
        let mut pids: Vec<u32> = Vec::new();
        let mut slice = if kind == "scope" { "system.slice".to_string() } else { String::new() };
        let mut description = name.clone();
        let mut send_sighup = false;
        for (k, v) in &props {
            match k.as_str() {
                "PIDs" => {
                    if let Ok(list) = <Vec<u32>>::try_from(v.try_clone().map_err(|e| err(E_INVALID_ARGS, e.to_string()))?) {
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
            std::path::Path::new(crate::cgroup::CGROUP_ROOT).join(crate::cgroup::slice_path(&name))
        } else {
            if pids.is_empty() {
                if let Some(p) = self.caller_pid(c, msg).await {
                    pids.push(p);
                }
            }
            crate::cgroup::unit_cgroup_dir(&name, &slice)
        };
        std::fs::create_dir_all(&dir).map_err(|e| err(E_FAILED, format!("{}: {e}", dir.display())))?;
        for p in &pids {
            if let Err(e) = std::fs::write(dir.join("cgroup.procs"), p.to_string()) {
                return Err(err(E_FAILED, format!("cannot move PID {p} into {}: {e}", dir.display())));
            }
        }
        let st = state::for_unit(&self.cfg.run_dir, &name);
        st.stamp(keys::ACTIVE_ENTER);
        if kind == "slice" {
            slice = crate::cgroup::parent_slice(&name);
        }
        let t = Transient { kind, cgroup: dir, send_sighup, slice, description };
        transient::save_scope(&self.cfg.run_dir, &name, &t);
        self.transient.lock().unwrap().insert(name.clone(), t);
        let job = self.new_job(&name, "start");
        self.reply_job(c, msg, job).await?;
        self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitNew", &(name.clone(), op(unit_path(&name)))).await;
        self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(job, op(job_path(job)), name.clone())).await;
        self.finish_job(job, &name, "done").await;
        Ok(())
    }

    /// systemd-run: write the unit file(s), then start the main unit on demand.
    async fn start_transient_file(
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
                return Err(err("org.freedesktop.systemd1.UnitExists", format!("Unit {n} already exists.")));
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
            return Err(err(E_LOAD_FAILED, unit.load_error.unwrap_or_else(|| format!("Unit {name} failed to load."))));
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
        self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitNew", &(id.clone(), op(unit_path(&id)))).await;
        self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(job, op(job_path(job)), id.clone())).await;
        self.execute_job(job, &id, &unit, "start", mode).await;
        Ok(())
    }

    /// AttachProcessesToUnit(unit, subcgroup, pids)
    /// As systemd, "" names the caller's own unit (systemd --user moves its
    /// processes within user@UID.service so). An unprivileged caller may
    /// move its own processes inside its own delegated unit; anything else
    /// needs manage-units.
    async fn attach_processes(&self, c: &Peer, msg: &Message) -> R<()> {
        let (name, sub, pids): (String, String, Vec<u32>) = Self::args(msg)?;
        let Some(sub) = subcgroup(&sub) else {
            return Err(err(E_INVALID_ARGS, format!("Control group path is not normalized: {sub}")));
        };
        let name = if name.is_empty() {
            let pid = self.caller_pid(c, msg).await.unwrap_or(0);
            util::proc_cgroup(pid)
                .and_then(|cg| unit_of_cgroup(&cg))
                .ok_or_else(|| err(E_NO_UNIT_FOR_PID, format!("PID {pid} does not belong to any loaded unit.")))?
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
            std::fs::metadata(format!("/proc/{pid}")).ok().map(|m| m.uid()) == caller
                && util::proc_cgroup(*pid).and_then(|cg| unit_of_cgroup(&cg)).as_deref() == Some(id.as_str())
        };
        let delegated = caller.is_some_and(|u| u != 0)
            && caller_unit.as_deref() == Some(id.as_str())
            && (unit.get_bool("Service", "Delegate", false) || unit.get_bool("Scope", "Delegate", false))
            && pids.iter().all(own);
        if !delegated {
            self.require_privileged(c, msg).await?;
        }
        let mut dir = self.cgroup_dir_of(&id, &unit);
        if !sub.is_empty() {
            dir = dir.join(sub);
        }
        std::fs::create_dir_all(&dir).map_err(|e| err(E_FAILED, format!("{}: {e}", dir.display())))?;
        for p in pids {
            std::fs::write(dir.join("cgroup.procs"), p.to_string())
                .map_err(|e| err(E_FAILED, format!("cannot move PID {p}: {e}")))?;
        }
        self.reply(c, msg, &()).await
    }

    async fn gc_scopes(&self) {
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
            self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitRemoved", &(name.clone(), op(unit_path(&name)))).await;
        }
    }

    // ---- listing ----

    async fn unit_entry(&self, id: &str, unit: &Unit, active: &BTreeSet<String>) -> UnitEntry {
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
            op(if job_id > 0 { job_path(job_id) } else { "/".into() }),
        )
    }

    async fn list_units(&self, states: &[String], patterns: &[String], names: Option<&[String]>) -> Vec<UnitEntry> {
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
            selected.iter().map(|(id, unit)| self.unit_entry(id, unit, &active)),
        )
        .await;
        entries
            .into_iter()
            .filter(|e| states.is_empty() || states.iter().any(|s| *s == e.2 || *s == e.3 || *s == e.4))
            .collect()
    }

    async fn count_failed(&self) -> u32 {
        let ids = self.known_units();
        let mut candidates = Vec::new();
        for id in ids {
            let st = state::for_unit(&self.cfg.run_dir, &id);
            if st.get(keys::RESULT).map(|r| !matches!(r.as_str(), "success" | "running")).unwrap_or(false) {
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

    // ---- properties ----

    /// `only`: the one interface asked for ("" or None for all).
    async fn object_props(&self, path: &str, only: Option<&str>) -> R<Ifaces> {
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
        if let Some(job) = path.strip_prefix(JOB_PREFIX).and_then(|j| j.parse::<u32>().ok()) {
            // A finished job's object is gone: systemd (sd-bus) answers
            // UnknownObject here, not NoSuchJob, and logind's
            // manager_job_is_active() only treats UnknownObject as "done";
            // anything else keeps the user in "closing" forever.
            let p = self.job_props(job).ok_or_else(|| err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")))?;
            let mut m = Ifaces::new();
            m.insert("org.freedesktop.systemd1.Job".into(), p);
            return Ok(m);
        }
        Err(err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'.")))
    }

    async fn properties(&self, c: &Peer, msg: &Message, path: &str, member: &str) -> R<()> {
        match member {
            "GetAll" => {
                let (iface,): (String,) = Self::args(msg)?;
                let all = self.object_props(path, Some(&iface)).await?;
                let mut out: Props = Props::new();
                for (name, props) in all {
                    if iface.is_empty() || iface == name {
                        out.extend(props);
                    }
                }
                let out: HashMap<String, Value<'static>> = out.into_iter().collect();
                self.reply(c, msg, &(out,)).await
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
                Err(err(E_UNKNOWN_PROPERTY, format!("Unknown property {iface}.{prop}")))
            }
            "Set" => Err(err(E_READ_ONLY, "Properties are read-only in uinitconverter")),
            _ => Err(err(E_UNKNOWN_METHOD, format!("Unknown method {member}"))),
        }
    }

    async fn introspect(&self, c: &Peer, msg: &Message, path: &str) -> R<()> {
        let xml = match path {
            "/" => introspect::node("", &["org".into()]),
            "/org" => introspect::node("", &["freedesktop".into()]),
            "/org/freedesktop" => introspect::node("", &["systemd1".into()]),
            MANAGER_PATH => introspect::node(introspect::MANAGER, &["unit".into(), "job".into()]),
            "/org/freedesktop/systemd1/unit" => {
                let kids: Vec<String> = self.known_units().iter().map(|u| util::bus_label_escape(u)).collect();
                introspect::node("", &kids)
            }
            "/org/freedesktop/systemd1/job" => {
                let kids: Vec<String> = self.jobs.lock().unwrap().keys().map(|k| k.to_string()).collect();
                introspect::node("", &kids)
            }
            p if p.starts_with(UNIT_PREFIX) => introspect::node(introspect::UNIT, &[]),
            p if p.starts_with(JOB_PREFIX) => introspect::node("", &[]),
            _ => return Err(err(E_UNKNOWN_OBJECT, format!("Unknown object '{path}'."))),
        };
        self.reply(c, msg, &(xml,)).await
    }

    // ---- method dispatch ----

    async fn handle_call(self: Arc<Self>, c: Peer, msg: Message) {
        let hdr = msg.header();
        let path = hdr.path().map(|p| p.as_str().to_string()).unwrap_or_default();
        let iface = hdr.interface().map(|i| i.as_str().to_string()).unwrap_or_default();
        let member = hdr.member().map(|m| m.as_str().to_string()).unwrap_or_default();
        drop(hdr);
        let r = self.dispatch(&c, &msg, &path, &iface, &member).await;
        if let Err(e) = r {
            let _ = c.conn.reply_error(&msg.header(), e.name, &(e.msg,)).await;
        }
    }

    async fn dispatch(&self, c: &Peer, msg: &Message, path: &str, iface: &str, member: &str) -> R<()> {
        // systemd's mac_selinux_filter: the built-in interfaces of every
        // object below the manager are checked against the manager.
        let builtin = matches!(iface, PROPS_IFACE | "org.freedesktop.DBus.Introspectable" | "org.freedesktop.DBus.Peer");
        if builtin && access::path_below(path, MANAGER_PATH) {
            let perm = if iface == PROPS_IFACE && member == "Set" { "reload" } else { "status" };
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
        if let Some(job) = path.strip_prefix(JOB_PREFIX).and_then(|j| j.parse::<u32>().ok()) {
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
        Err(err(E_UNKNOWN_METHOD, format!("Unknown method {member} or interface {iface}.")))
    }

    async fn unit_method(&self, c: &Peer, msg: &Message, name: &str, member: &str) -> R<()> {
        let jtype = match member {
            "Start" => "start",
            "Stop" => "stop",
            "Reload" => "reload",
            "Restart" => "restart",
            "TryRestart" => "try-restart",
            "ReloadOrRestart" => "reload-or-restart",
            "ReloadOrTryRestart" => "reload-or-try-restart",
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

    /// Queued jobs can be cancelled; running ones finish (s6-rc transitions
    /// cannot be interrupted safely).
    fn cancel_job(&self, id: u32) -> R<()> {
        let mut jobs = self.jobs.lock().unwrap();
        match jobs.get_mut(&id) {
            None => Err(err(E_NO_JOB, format!("Job {id} does not exist."))),
            Some(j) if j.state == "waiting" => {
                j.canceled = true;
                Ok(())
            }
            Some(_) => Err(err(E_NOT_SUPPORTED, "Job is already running and cannot be cancelled")),
        }
    }

    fn reset_failed(&self, name: &str) {
        let (id, _) = self.load_unit(name);
        let st = state::for_unit(&self.cfg.run_dir, &id);
        if st.get(keys::RESULT).map(|r| r != "success").unwrap_or(false) {
            st.set(keys::RESULT, "success");
        }
        // A soft-failed start: inactive from now on, but still restarted by
        // the next start like any parked unit.
        if st.take(keys::START_FAILED) {
            st.set(keys::PARKED, "1");
        }
        st.clear(keys::RESTART_TIMES);
    }

    async fn kill_unit(&self, c: &Peer, msg: &Message, name: &str, whom: &str, sig: i32) -> R<()> {
        Self::valid_name(name)?;
        let (id, unit) = self.load_unit(name);
        self.mac_check(c, msg, Some(&unit), "stop").await?;
        self.require_unit(c, msg, &id, "kill").await?;
        // The supervised pid is a wrapper for PAMName=, Type=forking and
        // MAINPID=; the runner records the real main process.
        let main = if self.kind_of(&id) == Some(Kind::Longrun) {
            self.svstat(&id).await.map(|s| s.pid).filter(|&p| p > 0).map(|p| {
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

    /// install::list_unit_files() with the states as the s6 set has them.
    fn list_unit_files(&self) -> Vec<(String, String)> {
        install::list_unit_files(&self.cfg)
            .into_iter()
            .map(|(p, st)| {
                let st = install::set_file_state(&self.cfg, p.rsplit('/').next().unwrap_or(&p), st);
                (p, st)
            })
            .collect()
    }

    async fn file_op_done(&self) {
        self.invalidate_caches();
        self.emit(MANAGER_PATH, MANAGER_IFACE, "UnitFilesChanged", &()).await;
    }

    fn write_manager_env(&self) {
        let env = self.manager_env.lock().unwrap().clone();
        let mut text = String::new();
        for e in env {
            if let Some((k, v)) = e.split_once('=') {
                let v = v.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`");
                text.push_str(&format!("{k}=\"{v}\"\n"));
            }
        }
        let _ = util::write_atomic(&self.cfg.run_dir.join("manager.env"), text.as_bytes());
    }

    async fn manager_method(&self, c: &Peer, msg: &Message, member: &str) -> R<()> {
        match member {
            "GetUnit" => {
                let (name,): (String,) = Self::args(msg)?;
                let (id, unit) = self.load_unit(&name);
                let known = self.kind_of(&id).is_some() || self.transient.lock().unwrap().contains_key(&id);
                if !known && (unit.load_state == LoadState::NotFound || crate::loader::parse_name(&name).is_none()) {
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
                match util::proc_cgroup(pid).and_then(|cg| unit_of_cgroup(&cg)) {
                    Some(u) => {
                        self.mac_unit(c, msg, &u, "status").await?;
                        self.reply(c, msg, &(op(unit_path(&u)),)).await
                    }
                    None => Err(err(E_NO_UNIT_FOR_PID, format!("PID {pid} does not belong to any loaded unit."))),
                }
            }
            "StartUnit" | "StopUnit" | "ReloadUnit" | "RestartUnit" | "TryRestartUnit"
            | "ReloadOrRestartUnit" | "ReloadOrTryRestartUnit" => {
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
                    .map(|p| (util::proc_cgroup(p).unwrap_or_default(), p, util::proc_cmdline(p)))
                    .collect();
                self.reply(c, msg, &(procs,)).await
            }
            "StartTransientUnit" => self.start_transient(c, msg).await,
            "AttachProcessesToUnit" => self.attach_processes(c, msg).await,
            "SetUnitProperties" => {
                let (name, _runtime, props): (String, bool, Vec<(String, OwnedValue)>) = Self::args(msg)?;
                self.set_unit_properties(c, msg, &name, &props).await
            }
            "EnqueueMarkedJobs" => self.enqueue_marked(c, msg).await,
            // Reference counting is accepted and ignored.
            "RefUnit" | "UnrefUnit" | "AbandonScope" => {
                let (name,): (String,) = Self::args(msg)?;
                self.mac_unit(c, msg, &name, if member == "AbandonScope" { "stop" } else { "start" }).await?;
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
                let jobs: Vec<(u32, String, String, String, ObjectPath<'static>, ObjectPath<'static>)> = self
                    .jobs
                    .lock()
                    .unwrap()
                    .values()
                    .map(|j| (j.id, j.unit.clone(), j.jtype.clone(), j.state.to_string(), op(job_path(j.id)), op(unit_path(&j.unit))))
                    .collect();
                self.reply(c, msg, &(jobs,)).await
            }
            "ListUnits" => {
                let v = self.visible_units(c, msg, self.list_units(&[], &[], None).await).await;
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitsFiltered" => {
                let (states,): (Vec<String>,) = Self::args(msg)?;
                let v = self.visible_units(c, msg, self.list_units(&states, &[], None).await).await;
                self.reply(c, msg, &(v,)).await
            }
            "ListUnitsByPatterns" => {
                let (states, patterns): (Vec<String>, Vec<String>) = Self::args(msg)?;
                let v = self.visible_units(c, msg, self.list_units(&states, &patterns, None).await).await;
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
                self.authorize(c, msg, "org.freedesktop.systemd1.reload-daemon").await?;
                self.reload_db().await.map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &()).await
            }
            "PowerOff" | "Reboot" | "Halt" => {
                self.mac_check(c, msg, None, if member == "Reboot" { "reboot" } else { "halt" }).await?;
                self.require_privileged(c, msg).await?;
                let cmd = match member {
                    "PowerOff" => self.cfg.poweroff_cmd.clone(),
                    "Reboot" => self.cfg.reboot_cmd.clone(),
                    _ => self.cfg.halt_cmd.clone(),
                };
                self.reply(c, msg, &()).await?;
                let _ = tokio::process::Command::new("/bin/sh").arg("-c").arg(cmd).spawn();
                Ok(())
            }
            "KExec" | "SoftReboot" | "SwitchRoot" | "Exit" => {
                Err(err(E_NOT_SUPPORTED, format!("{member} is not supported by uinitconverter")))
            }
            "SetEnvironment" | "UnsetEnvironment" | "UnsetAndSetEnvironment" => {
                self.mac_check(c, msg, None, "reload").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.set-environment").await?;
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
                            && (patterns.is_empty() || patterns.iter().any(|pat| util::fnmatch(pat, n)))
                    })
                    .collect();
                self.reply(c, msg, &(v,)).await
            }
            "GetUnitFileState" => {
                self.mac_check(c, msg, None, "status").await?;
                let (name,): (String,) = Self::args(msg)?;
                let st = install::unit_file_state(&self.cfg, &name).map_err(|e| err(E_NO_UNIT, e))?;
                let st = install::set_file_state(&self.cfg, &name, st);
                self.reply(c, msg, &(st,)).await
            }
            "EnableUnitFiles" | "ReenableUnitFiles" | "EnableUnitFilesWithFlags" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (files, runtime, force) = if member == "EnableUnitFilesWithFlags" {
                    let (f, flags): (Vec<String>, u64) = Self::args(msg)?;
                    (f, flags & FLAG_RUNTIME != 0, flags & FLAG_FORCE != 0)
                } else {
                    Self::args::<(Vec<String>, bool, bool)>(msg)?
                };
                let mut all = Vec::new();
                if member == "ReenableUnitFiles" {
                    all.extend(install::disable(&self.cfg, &files, runtime).map_err(|e| err(E_FAILED, e))?);
                }
                let (info, ch) = install::enable(&self.cfg, &files, runtime, force).map_err(|e| err(E_FAILED, e))?;
                all.extend(ch);
                self.reply(c, msg, &(info, changes(all))).await?;
                self.file_op_done().await;
                Ok(())
            }
            "DisableUnitFiles" | "DisableUnitFilesWithFlags" | "DisableUnitFilesWithFlagsAndInstallInfo" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (files, runtime) = if member == "DisableUnitFiles" {
                    Self::args::<(Vec<String>, bool)>(msg)?
                } else {
                    let (f, flags): (Vec<String>, u64) = Self::args(msg)?;
                    (f, flags & FLAG_RUNTIME != 0)
                };
                let ch = changes(install::disable(&self.cfg, &files, runtime).map_err(|e| err(E_FAILED, e))?);
                if member == "DisableUnitFilesWithFlagsAndInstallInfo" {
                    self.reply(c, msg, &(true, ch)).await?;
                } else {
                    self.reply(c, msg, &(ch,)).await?;
                }
                self.file_op_done().await;
                Ok(())
            }
            "MaskUnitFiles" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (files, runtime, force): (Vec<String>, bool, bool) = Self::args(msg)?;
                let ch = install::mask(&self.cfg, &files, runtime, force).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "UnmaskUnitFiles" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (files, runtime): (Vec<String>, bool) = Self::args(msg)?;
                let ch = install::unmask(&self.cfg, &files, runtime).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "PresetUnitFiles" | "PresetUnitFilesWithMode" => {
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (files, mode, runtime, force) = if member == "PresetUnitFiles" {
                    let (f, r, fo): (Vec<String>, bool, bool) = Self::args(msg)?;
                    (f, "full".to_string(), r, fo)
                } else {
                    Self::args::<(Vec<String>, String, bool, bool)>(msg)?
                };
                let (info, ch) = install::preset(&self.cfg, &files, &mode, runtime, force).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(info, changes(ch))).await?;
                self.file_op_done().await;
                Ok(())
            }
            "PresetAllUnitFiles" => {
                self.mac_check(c, msg, None, "enable").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (mode, runtime, force): (String, bool, bool) = Self::args(msg)?;
                let names: Vec<String> = install::list_unit_files(&self.cfg)
                    .into_iter()
                    .filter(|(_, s)| s == "enabled" || s == "disabled")
                    .map(|(p, _)| p.rsplit('/').next().unwrap_or(&p).to_string())
                    .collect();
                let (_, ch) = install::preset(&self.cfg, &names, &mode, runtime, force).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "GetDefaultTarget" => {
                self.mac_check(c, msg, None, "status").await?;
                let t = install::get_default_target(&self.cfg).ok_or_else(|| err(E_NO_UNIT, "No default target"))?;
                self.reply(c, msg, &(t,)).await
            }
            "SetDefaultTarget" => {
                self.mac_check(c, msg, None, "enable").await?;
                self.authorize(c, msg, "org.freedesktop.systemd1.manage-unit-files").await?;
                let (name, _force): (String, bool) = Self::args(msg)?;
                let ch = install::set_default_target(&self.cfg, &name).map_err(|e| err(E_FAILED, e))?;
                self.reply(c, msg, &(changes(ch),)).await?;
                self.file_op_done().await;
                Ok(())
            }
            "Dump" => {
                self.mac_check(c, msg, None, "status").await?;
                self.reply(c, msg, &("uinitconverter does not implement Dump\n".to_string(),)).await
            }
            _ => Err(err(E_UNKNOWN_METHOD, format!("Unknown method {member} or interface {MANAGER_IFACE}."))),
        }
    }
}

pub enum Bus {
    System,
    Session,
    Address(String),
}

pub async fn run(cfg: Config, bus: Bus, ready_fd: Option<i32>) -> Result<(), String> {
    let builder = match &bus {
        Bus::System => zbus::connection::Builder::system(),
        Bus::Session => zbus::connection::Builder::session(),
        Bus::Address(a) => zbus::connection::Builder::address(a.as_str()),
    }
    .map_err(|e| e.to_string())?;
    let conn = builder.build().await.map_err(|e| format!("connecting to D-Bus: {e}"))?;
    let mut stream = MessageStream::from(&conn);

    let dbus = zbus::fdo::DBusProxy::new(&conn).await.map_err(|e| e.to_string())?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MsgType::Signal)
        .interface("org.freedesktop.systemd1.Activator")
        .and_then(|b| b.member("ActivationRequest"))
        .map_err(|e| e.to_string())?
        .build();
    if let Err(e) = dbus.add_match_rule(rule).await {
        eprintln!("uinitconverterd: cannot watch ActivationRequest: {e}");
    }
    conn.request_name(BUS_NAME)
        .await
        .map_err(|e| format!("cannot own {BUS_NAME}: {e}"))?;

    let manager_env = crate::envfile::load(&cfg.run_dir.join("manager.env"))
        .map(|v| v.into_iter().map(|(k, v)| format!("{k}={v}")).collect())
        .unwrap_or_default();
    let daemon = Arc::new(Daemon {
        index: RwLock::new(Index::load(&cfg)),
        cfg,
        conn,
        jobs: Mutex::new(BTreeMap::new()),
        next_job: AtomicU32::new(0),
        s6: s6::State::default(),
        started_targets: Mutex::new(BTreeSet::new()),
        transient: Mutex::new(BTreeMap::new()),
        manager_env: Mutex::new(manager_env),
        uid_cache: Mutex::new(HashMap::new()),
        label_cache: Mutex::new(HashMap::new()),
        started: (util::now_realtime_usec(), util::now_monotonic_usec()),
        boot_done: AtomicBool::new(true),
        boot_finished: Mutex::new(Some((util::now_realtime_usec(), util::now_monotonic_usec()))),
        private: Mutex::new(Vec::new()),
        dynamic: Mutex::new(BTreeMap::new()),
        markers: Mutex::new(BTreeMap::new()),
        unit_locks: Mutex::new(HashMap::new()),
        unit_cache: Mutex::new(HashMap::new()),
        file_state_cache: Mutex::new(HashMap::new()),
        file_state_set_stamp: Mutex::new(Vec::new()),
        fdstore: crate::fdstore::Store::default(),
    });
    daemon.verify_state_reader().await;
    daemon.load_dynamic();
    daemon.transient.lock().unwrap().extend(transient::load_scopes(&daemon.cfg.run_dir));
    if let Some(def) = daemon.index.read().unwrap().default_target.clone() {
        daemon.started_targets.lock().unwrap().insert(def);
    }
    if daemon.index.read().unwrap().units.is_empty() {
        eprintln!(
            "uinitconverterd: no generated database in {} (run `uinitconverter-gen reload`)",
            daemon.cfg.state_dir.display()
        );
    }

    if let Some(flag) = &daemon.cfg.booted_flag {
        if let Err(e) = std::fs::create_dir_all(flag) {
            eprintln!("uinitconverterd: cannot create {}: {e}", flag.display());
        }
    }
    eprintln!("uinitconverterd: ready, serving {BUS_NAME}");
    if let Some(fd) = ready_fd {
        // s6 readiness notification (notification-fd).
        unsafe {
            libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(fd);
        }
    }

    // The uinitconverter-fdstore service keeps the fds across our own restarts;
    // without it, serve the store here (they then go with the daemon).
    let store_path = daemon.cfg.run_dir.join("fdstore");
    if !crate::fdstore::is_served(&store_path) {
        let cfg = daemon.cfg.clone();
        if let Err(e) = daemon.fdstore.serve(&store_path, move |key| crate::fdstore::limit(&cfg, key)) {
            eprintln!("uinitconverterd: fd store unavailable: {e}");
        }
    }

    if let Some(sock) = daemon.cfg.private_socket.clone() {
        tokio::spawn(private::listen(daemon.clone(), sock));
    }

    let watcher = daemon.clone();
    tokio::spawn(async move {
        let mut last = HashMap::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            watcher.watch_dynamic(&mut last).await;
        }
    });

    let rec = daemon.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            rec.reconcile_beside().await;
            rec.reconcile_stop_atoms().await;
        }
    });

    tokio::spawn(daemon.clone().watch_devices());

    let gc = daemon.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            gc.gc_scopes().await;
        }
    });

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    let mut stopping: Option<tokio::task::JoinHandle<()>> = None;
    let mut stopped = false;
    loop {
        tokio::select! {
            _ = term.recv(), if stopping.is_none() => {
                if !daemon.cfg.run_dir.join("shutting-down").exists() {
                    break;
                }
                // Keep answering the bus while the units stop: their
                // ExecStop=, a PAM session close or a daemon stopping beside
                // them may call us, and the bus would hold such a caller
                // for its 25 s timeout (NetworkManager did, at power-off).
                let d = daemon.clone();
                stopping = Some(tokio::spawn(async move { d.stop_dynamic_at_shutdown().await }));
            }
            _ = async { stopping.as_mut().unwrap().await }, if stopping.is_some() => {
                stopped = true;
                break;
            }
            m = stream.next() => {
                let Some(m) = m else { break };
                let Ok(msg) = m else { continue };
                match msg.message_type() {
                    MsgType::MethodCall => {
                        let d = daemon.clone();
                        let peer = Peer { conn: d.conn.clone(), uid: None, pid: None, label: None };
                        tokio::spawn(d.handle_call(peer, msg));
                    }
                    MsgType::Signal => {
                        let hdr = msg.header();
                        // Only the bus daemon may ask for activation: the bus
                        // sets the sender field, clients cannot forge it.
                        let is_activation = hdr.interface().map(|i| i.as_str()) == Some("org.freedesktop.systemd1.Activator")
                            && hdr.member().map(|m| m.as_str()) == Some("ActivationRequest")
                            && hdr.sender().map(|s| s.as_str()) == Some("org.freedesktop.DBus");
                        drop(hdr);
                        if is_activation {
                            if let Ok((name,)) = msg.body().deserialize::<(String,)>() {
                                let d = daemon.clone();
                                tokio::spawn(async move { d.handle_activation(name).await });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // The bus went away (stream ended): still finish, or do, the shutdown
    // stop. A finished JoinHandle must not be awaited again.
    match stopping {
        Some(h) if !stopped => {
            let _ = h.await;
        }
        Some(_) => {}
        None if daemon.cfg.run_dir.join("shutting-down").exists() => daemon.stop_dynamic_at_shutdown().await,
        None => {}
    }
    if let Some(flag) = &daemon.cfg.booted_flag {
        let _ = std::fs::remove_dir(flag);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn markers_syntax() {
        let m = apply_markers(set(&["needs-reload"]), &words(&["+needs-restart"])).unwrap();
        assert_eq!(m, set(&["needs-reload", "needs-restart"]));
        assert_eq!(apply_markers(m.clone(), &words(&["-needs-reload"])).unwrap(), set(&["needs-restart"]));
        assert_eq!(apply_markers(m.clone(), &words(&["needs-reload"])).unwrap(), set(&["needs-reload"]));
        assert_eq!(apply_markers(m, &[]).unwrap(), set(&[]));
        assert!(apply_markers(set(&[]), &words(&["+needs-restart", "needs-reload"])).is_err());
        assert!(apply_markers(set(&[]), &words(&["+bogus"])).is_err());
    }

    #[test]
    fn subcgroup_normalized() {
        assert_eq!(subcgroup(""), Some(""));
        assert_eq!(subcgroup("/"), Some(""));
        assert_eq!(subcgroup("payload"), Some("payload"));
        assert_eq!(subcgroup("/a/b/"), Some("a/b"));
        assert_eq!(subcgroup("a..b/.c"), Some("a..b/.c"));
        for bad in ["..", "../x", "a/../../b", "a/./b", "a//b", "/.", "x/.."] {
            assert_eq!(subcgroup(bad), None, "{bad}");
        }
    }
}
