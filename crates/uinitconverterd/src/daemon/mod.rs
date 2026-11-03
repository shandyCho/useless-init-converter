//! initconvertd: serves org.freedesktop.systemd1 on the system bus and
//! carries out requests with s6-rc. The real `systemctl`, libsystemd and other
//! D-Bus clients talk to it as if it were systemd.
//!
//! Dispatch is done by hand (no zbus ObjectServer) because systemd clients
//! rely on details such as Properties.GetAll("") returning every interface.

mod accept;
mod access;
mod caller;
mod devices;
mod dispatch;
mod dynamic;
mod errors;
mod introspect;
mod jobs;
mod manager;
mod oom;
mod private;
mod props;
mod relations;
mod s6;
mod scopes;
mod transient;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

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

use uinitconverter_client::UINITCONVERTER_IFACE;

use errors::*;
use jobs::*;
use manager::*;
use scopes::*;

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

/// Why the user manager falls back to systemd --user, once it does.
static FALLBACK: Mutex<Option<String>> = Mutex::new(None);

/// The fallback reason, for the launcher after the daemon returned.
pub fn fallback_reason() -> Option<String> {
    FALLBACK.lock().unwrap().clone()
}

pub struct Daemon {
    pub cfg: Config,
    /// The bus. A user manager runs before its bus exists and outlives a
    /// bus restart (gnome-session-ctl --restart-dbus): None meanwhile.
    conn: RwLock<Option<Connection>>,
    /// The user manager is exiting: a stopped bus is not armed again.
    pub exiting: AtomicBool,
    /// A user manager was asked to exit (Manager.Exit, an exit action, a
    /// fallback): the main loop stops every unit and returns, as on SIGTERM.
    exit_request: tokio::sync::Notify,
    /// The last start job of each unit, by when it finished (start_seq),
    /// and its result: a start merged into it (execute_job) gets the same.
    last_start: Mutex<HashMap<String, (u64, &'static str)>>,
    /// Finished start jobs, counted.
    start_seq: std::sync::atomic::AtomicU64,
    /// Units a user transaction is going to start (count of transactions),
    /// until their start is done: a Requisite= on one waits for it.
    pub pending_starts: Mutex<BTreeMap<String, usize>>,
    pub index: RwLock<Index>,
    pub jobs: Mutex<BTreeMap<u32, Job>>,
    pub next_job: AtomicU32,
    /// The s6 backend's locks and caches (see s6.rs).
    pub s6: s6::State,
    pub started_targets: Mutex<BTreeSet<String>>,
    /// Targets stopped by a job: inactive until started again, though the
    /// units they pulled in may still run (as systemd, a target's state is
    /// its own, not its members').
    pub stopped_targets: Mutex<BTreeSet<String>>,
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

pub fn unit_path(name: &str) -> String {
    format!("{UNIT_PREFIX}{}", util::bus_label_escape(name))
}

pub fn job_path(id: u32) -> String {
    format!("{JOB_PREFIX}{id}")
}

/// The service an Accept=no socket unit activates.
fn socket_service(id: &str, unit: &Unit) -> Option<String> {
    if !id.ends_with(".socket") || unit.get_bool("Socket", "Accept", false) {
        return None;
    }
    Some(
        unit.get("Socket", "Service")
            .unwrap_or_else(|| format!("{}.service", id.trim_end_matches(".socket"))),
    )
}

fn shutdown_action(id: &str) -> Option<&'static str> {
    match id {
        "poweroff.target" | "systemd-poweroff.service" => Some("poweroff"),
        "reboot.target"
        | "systemd-reboot.service"
        | "ctrl-alt-del.target"
        | "kexec.target"
        | "systemd-kexec.service"
        | "soft-reboot.target"
        | "systemd-soft-reboot.service" => Some("reboot"),
        "halt.target" | "systemd-halt.service" => Some("halt"),
        _ => None,
    }
}

fn op(p: String) -> ObjectPath<'static> {
    ObjectPath::try_from(p).unwrap_or_else(|_| ObjectPath::try_from("/").unwrap())
}

impl Daemon {
    /// Resolve aliases and load the unit file.
    fn load_unit(&self, name: &str) -> (String, Unit) {
        let resolved = self.index.read().unwrap().resolve(name).to_string();
        if let Some(t) = self.transient.lock().unwrap().get(&resolved) {
            let mut file = UnitFile::default();
            file.entries
                .push(("Unit".into(), "Description".into(), t.description.clone()));
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
            self.unit_cache
                .lock()
                .unwrap()
                .insert(resolved, unit.clone());
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
            .map(|d| {
                std::fs::metadata(set.join(d))
                    .and_then(|m| m.modified())
                    .ok()
            })
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
            None => Err(err(
                E_INVALID_ARGS,
                format!("Unit name {name} is not valid."),
            )),
        }
    }

    async fn emit(
        &self,
        path: &str,
        iface: &str,
        member: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) {
        let bus = self.bus();
        if let Some(conn) = bus {
            if let Err(e) = conn
                .emit_signal(None::<&str>, path, iface, member, body)
                .await
            {
                eprintln!("initconvertd: emit {member}: {e}");
            }
        }
        let peers: Vec<(u64, Connection)> = self.private.lock().unwrap().clone();
        for (id, p) in peers {
            // A peer that went away is dropped by its own reader task. One
            // that stopped reading (its socket buffer is full) would block
            // every later signal and so every job: drop it, as systemd drops
            // a client whose queue overflows. It reconnects when it needs to.
            let send = p.emit_signal(None::<&str>, path, iface, member, body);
            if tokio::time::timeout(std::time::Duration::from_millis(500), send)
                .await
                .is_err()
            {
                eprintln!("initconvertd: a private-socket client is not reading its messages; disconnecting it");
                self.private.lock().unwrap().retain(|(i, _)| *i != id);
                tokio::spawn(async move {
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(1), p.close()).await;
                });
            }
        }
    }

    /// A user manager asks to exit (Manager.Exit, SuccessAction=exit):
    /// the main loop stops every unit and returns.
    pub(super) fn request_exit(&self) {
        self.exit_request.notify_one();
    }

    /// A user manager hands its user over to systemd --user (the launcher
    /// does it once the daemon has returned; see user.rs). The first
    /// reason wins.
    pub(super) fn request_fallback(&self, reason: &str) {
        if !self.cfg.user_mode {
            return;
        }
        {
            let mut f = FALLBACK.lock().unwrap();
            if f.is_some() {
                return;
            }
            *f = Some(reason.to_string());
        }
        eprintln!("initconvertd: user mode: {reason}: handing over to systemd --user");
        self.request_exit();
    }

    /// A runner's request: a unit failed into a USER_FALLBACK_ON= unit
    /// (the runner checked that no logout was under way). Not while the
    /// manager is exiting: units fail as they go down.
    fn fallback_on_failure(&self, reason: &str) {
        if self.exiting.load(Ordering::Relaxed) {
            return;
        }
        self.request_fallback(reason);
    }

    /// user@.service is Type=notify-reload: the reload is announced and
    /// its end told with READY=1, as systemd --user does.
    async fn user_reload(&self) {
        util::sd_notify(&format!(
            "RELOADING=1\nMONOTONIC_USEC={}",
            util::now_monotonic_usec()
        ));
        let _ = self.reload_db().await;
        util::sd_notify("READY=1");
    }

    pub(super) fn bus(&self) -> Option<Connection> {
        self.conn.read().unwrap().clone()
    }

    async fn reload_db(&self) -> Result<(), String> {
        self.emit(MANAGER_PATH, MANAGER_IFACE, "Reloading", &(true,))
            .await;
        // A user manager has no database: a reload rereads the unit files.
        if self.cfg.user_mode {
            let run = self.cfg.run_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
                if runtime.starts_with('/') {
                    crate::user::run_unit_generators(&runtime, &crate::user::written_env(&run));
                }
            })
            .await;
            self.invalidate_caches();
            self.emit(MANAGER_PATH, MANAGER_IFACE, "Reloading", &(false,))
                .await;
            return Ok(());
        }
        let cfg = self.cfg.clone();
        let (plan, moving) = tokio::task::spawn_blocking(move || {
            let plan = gen::plan(&cfg);
            let moving = gen::auto_masked(&cfg);
            (plan, moving)
        })
        .await
        .map_err(|e| e.to_string())?;
        // What AUTO_MASK= masked goes back into the database: no job runs
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
            eprintln!(
                "initconvertd: {}: back to the database, restarted from it",
                restart.join(" ")
            );
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
                "initconvertd: reloaded: {} units, database {}",
                r.plan.units.len(),
                r.db_dir.display()
            );
            for c in &r.set_changes {
                eprintln!("initconvertd: set {}: {c}", self.cfg.set_name);
            }
        }
        self.invalidate_caches();
        self.s6.database_changed();
        for id in &restart {
            let r = self.start_any(id).await;
            if r != "done" {
                eprintln!("initconvertd: {id}: start from the database: {r}");
            }
        }
        self.emit(MANAGER_PATH, MANAGER_IFACE, "Reloading", &(false,))
            .await;
        res.map(|_| ())
    }

    // ---- jobs ----

    pub(super) fn unit_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.unit_locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// dbus-daemon --systemd-activation asks us to start a service.
    async fn handle_activation(&self, name: String) {
        let res = match self.check_job(&name, "start", false, true).await {
            Ok((id, unit)) => {
                let job = self.new_job(&id, "start");
                self.emit(
                    MANAGER_PATH,
                    MANAGER_IFACE,
                    "JobNew",
                    &(job, op(job_path(job)), id.clone()),
                )
                .await;
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
            eprintln!("initconvertd: activation of {name} failed: {}", e.msg);
            let Some(conn) = self.bus() else { return };
            let r = conn
                .emit_signal(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.systemd1.Activator",
                    "ActivationFailure",
                    &(name, e.name.to_string(), e.msg),
                )
                .await;
            if let Err(e) = r {
                eprintln!("initconvertd: ActivationFailure: {e}");
            }
        }
    }

    // ---- transient scopes and slices ----

    // ---- listing ----

    // ---- properties ----

    // ---- method dispatch ----
}

pub enum Bus {
    System,
    Session,
    Address(String),
}

/// Connect, watch ActivationRequest and own BUS_NAME. The stream is made
/// before the name is owned, so no call to it is missed.
async fn connect_bus(bus: &Bus) -> Result<(Connection, MessageStream), String> {
    let builder = match bus {
        Bus::System => zbus::connection::Builder::system(),
        Bus::Session => zbus::connection::Builder::session(),
        Bus::Address(a) => zbus::connection::Builder::address(a.as_str()),
    }
    .map_err(|e| e.to_string())?;
    let conn = builder
        .build()
        .await
        .map_err(|e| format!("connecting to D-Bus: {e}"))?;
    let stream = MessageStream::from(&conn);

    let dbus = zbus::fdo::DBusProxy::new(&conn)
        .await
        .map_err(|e| e.to_string())?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MsgType::Signal)
        .interface("org.freedesktop.systemd1.Activator")
        .and_then(|b| b.member("ActivationRequest"))
        .map_err(|e| e.to_string())?
        .build();
    if let Err(e) = dbus.add_match_rule(rule).await {
        eprintln!("initconvertd: cannot watch ActivationRequest: {e}");
    }
    conn.request_name(BUS_NAME)
        .await
        .map_err(|e| format!("cannot own {BUS_NAME}: {e}"))?;
    Ok((conn, stream))
}

/// The user bus's socket, for an address of the form unix:path=P.
fn bus_socket(bus: &Bus) -> Option<PathBuf> {
    let Bus::Address(a) = bus else { return None };
    a.split(';')
        .next()?
        .strip_prefix("unix:")?
        .split(',')
        .find_map(|kv| kv.strip_prefix("path="))
        .map(PathBuf::from)
}

pub async fn run(cfg: Config, bus: Bus, ready_fd: Option<i32>) -> Result<(), String> {
    // The system manager needs its bus to start. A user manager starts the
    // bus itself (dbus.socket, then dbus.service), so it connects once the
    // bus is there, and again after the bus restarts.
    let (bus_tx, mut bus_rx) =
        tokio::sync::mpsc::unbounded_channel::<(Connection, MessageStream)>();
    let first = if cfg.user_mode {
        None
    } else {
        Some(connect_bus(&bus).await?)
    };

    let manager_env = crate::envfile::load(&cfg.run_dir.join("manager.env"))
        .map(|v| v.into_iter().map(|(k, v)| format!("{k}={v}")).collect())
        .unwrap_or_default();
    let daemon = Arc::new(Daemon {
        index: RwLock::new(Index::load(&cfg)),
        cfg,
        conn: RwLock::new(first.as_ref().map(|(c, _)| c.clone())),
        exiting: AtomicBool::new(false),
        exit_request: tokio::sync::Notify::new(),
        last_start: Mutex::new(HashMap::new()),
        start_seq: std::sync::atomic::AtomicU64::new(0),
        pending_starts: Mutex::new(BTreeMap::new()),
        jobs: Mutex::new(BTreeMap::new()),
        next_job: AtomicU32::new(0),
        s6: s6::State::default(),
        started_targets: Mutex::new(BTreeSet::new()),
        stopped_targets: Mutex::new(BTreeSet::new()),
        transient: Mutex::new(BTreeMap::new()),
        manager_env: Mutex::new(manager_env),
        uid_cache: Mutex::new(HashMap::new()),
        label_cache: Mutex::new(HashMap::new()),
        started: (util::now_realtime_usec(), util::now_monotonic_usec()),
        boot_done: AtomicBool::new(true),
        boot_finished: Mutex::new(Some((
            util::now_realtime_usec(),
            util::now_monotonic_usec(),
        ))),
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
    daemon
        .transient
        .lock()
        .unwrap()
        .extend(transient::load_scopes(&daemon.cfg.run_dir));
    if let Some(def) = daemon.index.read().unwrap().default_target.clone() {
        daemon.started_targets.lock().unwrap().insert(def);
    }
    if daemon.index.read().unwrap().units.is_empty() && !daemon.cfg.user_mode {
        eprintln!(
            "initconvertd: no generated database in {} (run `initconvert-gen reload`)",
            daemon.cfg.state_dir.display()
        );
    }

    if let Some(flag) = &daemon.cfg.booted_flag {
        if let Err(e) = std::fs::create_dir_all(flag) {
            eprintln!("initconvertd: cannot create {}: {e}", flag.display());
        }
    }
    eprintln!("initconvertd: ready, serving {BUS_NAME}");
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
        if let Err(e) = daemon
            .fdstore
            .serve(&store_path, move |key| crate::fdstore::limit(&cfg, key))
        {
            eprintln!("initconvertd: fd store unavailable: {e}");
        }
    }

    if let Some(sock) = daemon.cfg.private_socket.clone() {
        tokio::spawn(private::listen(daemon.clone(), sock));
    }

    let watcher = daemon.clone();
    tokio::spawn(async move {
        let mut last = HashMap::new();
        let mut mtimes = HashMap::new();
        let mut tick: u32 = 0;
        // A user manager with its own cgroup reports its slices to
        // systemd-oomd (oom.rs), every 5 s if anything changed.
        let mut oomd = (watcher.cfg.user_mode && watcher.cfg.cgroup_base.is_some())
            .then(|| std::sync::Arc::new(std::sync::Mutex::new(oom::Reporter::default())));
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            watcher.watch_dynamic(&mut last).await;
            watcher.watch_states(&mut mtimes, &mut last).await;
            if watcher.cfg.user_mode {
                let req = watcher.cfg.run_dir.join(crate::state::FALLBACK_REQUEST);
                if let Ok(text) = std::fs::read_to_string(&req) {
                    let _ = std::fs::remove_file(&req);
                    let reason = text.split_once('\n').map(|(_, r)| r).unwrap_or(&text);
                    watcher.fallback_on_failure(reason.trim());
                }
            }
            tick = tick.wrapping_add(1);
            if let Some(r) = oomd.as_mut().filter(|_| tick % 5 == 0) {
                let (cfg, r) = (watcher.cfg.clone(), r.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    let now = oom::scan(&cfg, &crate::cgroup::base());
                    r.lock().unwrap().update(now);
                })
                .await;
            }
        }
    });

    let mut current: Option<(Connection, MessageStream)> = first;
    // A user manager has no s6-rc boot: it starts default.target itself,
    // and tells user@UID.service it is ready once that is done.
    if daemon.cfg.user_mode {
        let d = daemon.clone();
        tokio::spawn(async move {
            let r = d.start_any("default.target").await;
            eprintln!("initconvertd: user mode: default.target {r}");
            util::sd_notify("READY=1\nSTATUS=Startup finished.");
        });
        let d = daemon.clone();
        let tx = bus_tx.clone();
        let sock = bus_socket(&bus);
        tokio::spawn(async move {
            let mut said = false;
            loop {
                let present = sock.as_ref().map(|p| p.exists()).unwrap_or(true);
                if d.bus().is_none() && present && !d.exiting.load(Ordering::Relaxed) {
                    match connect_bus(&bus).await {
                        Ok((c, s)) => {
                            eprintln!("initconvertd: user mode: on the bus, serving {BUS_NAME}");
                            *d.conn.write().unwrap() = Some(c.clone());
                            said = false;
                            if tx.send((c, s)).is_err() {
                                return;
                            }
                        }
                        Err(e) if !said => {
                            eprintln!("initconvertd: user mode: {e} (trying again)");
                            said = true;
                        }
                        Err(_) => {}
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
    }

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
    // systemd --user reloads on SIGHUP and re-executes on SIGRTMIN+25
    // (user@.service's ExecReload=): both reread the unit files here.
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(|e| e.to_string())?;
    let mut rtmin25 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
        libc::SIGRTMIN() + 25,
    ))
    .map_err(|e| e.to_string())?;
    let mut stopping: Option<tokio::task::JoinHandle<()>> = None;
    let mut stopped = false;
    loop {
        tokio::select! {
            _ = hup.recv(), if daemon.cfg.user_mode => daemon.user_reload().await,
            _ = rtmin25.recv(), if daemon.cfg.user_mode => daemon.user_reload().await,
            c = bus_rx.recv() => {
                if let Some(c) = c {
                    current = Some(c);
                }
            }
            _ = daemon.exit_request.notified(), if stopping.is_none() && daemon.cfg.user_mode => {
                daemon.exiting.store(true, Ordering::Relaxed);
                let d = daemon.clone();
                stopping = Some(tokio::spawn(async move { d.stop_all_user().await }));
            }
            _ = term.recv(), if stopping.is_none() => {
                if daemon.cfg.user_mode {
                    // The user's session ends: stop every unit, as systemd
                    // --user does before it exits.
                    daemon.exiting.store(true, Ordering::Relaxed);
                    let d = daemon.clone();
                    stopping = Some(tokio::spawn(async move { d.stop_all_user().await }));
                    continue;
                }
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
            m = async { current.as_mut().unwrap().1.next().await }, if current.is_some() => {
                let Some(m) = m else {
                    if !daemon.cfg.user_mode {
                        break;
                    }
                    // The bus stopped (a restart, or it failed): serve the
                    // private socket meanwhile, connect again when it is back.
                    eprintln!("initconvertd: user mode: the bus went away");
                    *daemon.conn.write().unwrap() = None;
                    current = None;
                    continue;
                };
                let Ok(msg) = m else { continue };
                match msg.message_type() {
                    MsgType::MethodCall => {
                        let d = daemon.clone();
                        let conn = current.as_ref().unwrap().0.clone();
                        let peer = Peer { conn, uid: None, pid: None, label: None };
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
        None if daemon.cfg.run_dir.join("shutting-down").exists() && !daemon.cfg.user_mode => {
            daemon.stop_dynamic_at_shutdown().await
        }
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
        assert_eq!(
            apply_markers(m.clone(), &words(&["-needs-reload"])).unwrap(),
            set(&["needs-restart"])
        );
        assert_eq!(
            apply_markers(m.clone(), &words(&["needs-reload"])).unwrap(),
            set(&["needs-reload"])
        );
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

    #[test]
    fn scopes_are_waited_for() {
        let d = std::env::temp_dir().join(format!("uinitconverter-wait-{}", std::process::id()));
        let (busy, empty) = (d.join("busy.scope"), d.join("empty.scope"));
        std::fs::create_dir_all(&busy).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(busy.join("cgroup.procs"), "123\n").unwrap();
        std::fs::write(empty.join("cgroup.procs"), "").unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dirs = [busy.clone(), empty.clone()];
            // Nothing to wait for: at once.
            let t = std::time::Instant::now();
            assert_eq!(
                wait_cgroups_empty(std::slice::from_ref(&empty), Duration::from_secs(5)).await,
                0
            );
            assert!(t.elapsed() < Duration::from_secs(1));
            // Never empties: gives up at the limit and says how many.
            let t = std::time::Instant::now();
            assert_eq!(
                wait_cgroups_empty(&dirs, Duration::from_millis(100)).await,
                1
            );
            assert!(t.elapsed() >= Duration::from_millis(100));
            // Waits until the last process has left.
            let p = busy.join("cgroup.procs");
            let writer = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::fs::write(p, "").unwrap();
            });
            let t = std::time::Instant::now();
            assert_eq!(wait_cgroups_empty(&dirs, Duration::from_secs(5)).await, 0);
            assert!(t.elapsed() >= Duration::from_millis(100));
            writer.await.unwrap();
        });
        let _ = std::fs::remove_dir_all(&d);
    }
}
