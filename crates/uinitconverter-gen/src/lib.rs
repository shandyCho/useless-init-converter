//! Translate systemd units into a dependency plan (plan(): init-agnostic),
//! then hand it to the INIT= backend (s6.rs: an s6-rc source tree, compiled
//! and installed as the boot and live database).
//!
//! Mapping:
//!   .service (long-running)  -> longrun  (run/finish call `uinitconverter-runner run|finish`)
//!   .service Type=oneshot    -> oneshot  (up/down call `uinitconverter-runner up|down`)
//!   .target                  -> bundle   (contents = Wants/Requires + .wants/.requires dirs)
//!   enabled .socket          -> its service is pulled in instead (sockets are bound by the runner);
//!                               pulled in only that way, it starts on the first activity (on-demand)
//!   dependencies             -> Requires/Requisite/BindsTo, plus Wants that are also ordered After
//!   long-running ExecStop=   -> also a oneshot UNIT.stop that depends on it; its `down` runs ExecStop=


use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use uinitconverter_base::config::Config;
use uinitconverter_base::tools;
pub use uinitconverter_base::index::{stop_atom, unit_arg, Index, Kind};
use uinitconverter_base::loader::{self, LoadState, Loader, Unit};
use uinitconverter_base::util;
use uinitconverter_install as install;

mod s6;
pub use s6::{longrun_files, stores, write_source};

#[derive(Debug, Clone)]
pub struct Planned {
    pub id: String,
    pub kind: Kind,
    /// s6-rc dependencies (atoms) or contents (bundles).
    pub deps: BTreeSet<String>,
    /// Name of the s6-log consumer, when logging is enabled for this unit.
    pub log_atom: Option<String>,
    /// The oneshot that runs a longrun's ExecStop= when s6-rc stops it
    /// (see index::stop_atom).
    pub stop_atom: Option<String>,
    /// Dependencies reached only through a .socket name (Requires=X.socket,
    /// sockets.target's wants): systemd starts the socket, not the service.
    pub socket_deps: BTreeSet<String>,
    /// A service with sockets that the boot pulls in only through them: as
    /// under systemd it is started on the first activity, so its runner
    /// binds the sockets and waits (`run --on-demand`).
    pub on_demand: bool,
    pub warnings: Vec<String>,
    pub timings: Timings,
}

/// What s6 needs of a unit's timeouts (milliseconds, 0 = none) and stop
/// signal; gen's database and the daemon's dynamic instances both use it.
#[derive(Debug, Clone, Default)]
pub struct Timings {
    /// s6-rc's timeout-up: only a backstop, the runner enforces
    /// TimeoutStartSec= itself (soft failure). 32-bit for s6-rc-compile.
    pub up_ms: u64,
    pub down_ms: u64,
    pub finish_ms: u64,
    pub down_signal: Option<String>,
}

impl Timings {
    pub fn of(unit: &Unit) -> Timings {
        let start = unit.start_timeout_usec();
        let restart_sec = unit
            .get("Service", "RestartSec")
            .and_then(|v| uinitconverter_base::unit::parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(100_000);
        let down_signal = unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .filter(|&s| s != libc::SIGTERM)
            // s6-supervise reads down-signal with skalibs' sig0_scan(), which
            // takes a number too (SIGRTMIN+n has no name there).
            .map(|s| match util::signal_name(s) {
                n if n.parse::<i32>().is_ok() => n,
                n => format!("SIG{n}"),
            });
        // Type=notify: none. EXTEND_TIMEOUT_USEC= can push the start past
        // any fixed backstop (s6-rc would then bring the service down), and
        // the notify helper enforces the timeout, extended or not, and
        // releases s6-rc either way.
        let notify = matches!(unit.get("Service", "Type").as_deref(), Some("notify" | "notify-reload"));
        Timings {
            up_ms: if start == u64::MAX || notify { 0 } else { (ms(start) + 15_000).min(u32::MAX as u64) },
            down_ms: ms(unit.stop_timeout_usec()).min(u32::MAX as u64),
            finish_ms: ms(restart_sec).saturating_add(5000),
            down_signal,
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub units: BTreeMap<String, Planned>,
    /// alias name -> canonical id
    pub aliases: BTreeMap<String, String>,
    /// unit -> reason it was not translated
    pub skipped: BTreeMap<String, String>,
    pub default_target: Option<String>,
    /// Names provided by hand-written extra sources.
    pub extra_names: BTreeSet<String>,
    pub broken_cycles: Vec<String>,
    /// timer -> unit it triggers
    pub triggers: BTreeMap<String, String>,
    /// What systemd would start at boot: default.target and everything it
    /// pulls in. These get flag-recommended, so a repository set first
    /// sees them as active; after that the set decides.
    pub boot: BTreeSet<String>,
}

/// The s6 service (dist/s6) that serves the fd store.
const FDSTORE_SERVICE: &str = "uinitconverter-fdstore";

const UNIT_KEYS: &[&str] = &[
    "Description", "Documentation", "Requires", "Requisite", "Wants", "BindsTo", "PartOf",
    "Upholds", "Before", "After", "DefaultDependencies", "RefuseManualStart",
    "RefuseManualStop", "AllowIsolate", "IgnoreOnIsolate", "StopWhenUnneeded",
    "StartLimitIntervalSec", "StartLimitBurst", "SourcePath", "Conflicts", "OnFailure", "OnSuccess",
    "SuccessAction", "FailureAction", "RequiresMountsFor", "OnFailureJobMode",
];

const TIMER_KEYS: &[&str] = &[
    "OnCalendar", "OnActiveSec", "OnBootSec", "OnStartupSec", "OnUnitActiveSec",
    "OnUnitInactiveSec", "Unit", "Persistent", "RandomizedDelaySec", "FixedRandomDelay",
    "AccuracySec", "RemainAfterElapse", "WakeSystem", "OnClockChange", "OnTimezoneChange",
];

const PATH_KEYS: &[&str] = &[
    "PathExists", "PathExistsGlob", "PathChanged", "PathModified", "DirectoryNotEmpty", "Unit",
    "MakeDirectory", "DirectoryMode", "TriggerLimitBurst", "TriggerLimitIntervalSec",
];
const MOUNT_KEYS: &[&str] = &[
    "What", "Where", "Type", "Options", "SloppyOptions", "LazyUnmount", "ForceUnmount",
    "DirectoryMode", "TimeoutSec", "ReadWriteOnly",
];
const SWAP_KEYS: &[&str] = &["What", "Priority", "Options", "TimeoutSec"];

const SERVICE_KEYS: &[&str] = &[
    "Type", "RemainAfterExit", "GuessMainPID", "PIDFile", "BusName", "ExecStart",
    "ExecStartPre", "ExecStartPost", "ExecCondition", "ExecReload", "ExecStop", "ExecStopPost",
    "RestartSec", "TimeoutStartSec", "TimeoutStopSec", "TimeoutSec", "Restart",
    "SuccessExitStatus", "RestartPreventExitStatus", "RestartForceExitStatus", "NotifyAccess",
    "Sockets", "KillMode", "KillSignal", "ReloadSignal", "SendSIGKILL", "User", "Group",
    "SupplementaryGroups", "DynamicUser", "WorkingDirectory", "RootDirectory", "UMask", "Nice",
    "OOMScoreAdjust", "Environment", "EnvironmentFile", "PassEnvironment", "UnsetEnvironment",
    "StandardInput", "StandardOutput", "StandardError", "TTYPath", "TTYReset", "TTYVHangup",
    "TTYVTDisallocate", "SyslogIdentifier", "SyslogFacility", "SyslogLevel", "RuntimeDirectory",
    "StateDirectory", "CacheDirectory", "LogsDirectory", "ConfigurationDirectory",
    "RuntimeDirectoryMode", "StateDirectoryMode", "CacheDirectoryMode", "LogsDirectoryMode",
    "ConfigurationDirectoryMode", "RuntimeDirectoryPreserve", "PrivateTmp", "ProtectSystem",
    "ProtectHome", "ReadWritePaths", "ReadOnlyPaths", "InaccessiblePaths", "NoNewPrivileges",
    "CapabilityBoundingSet", "AmbientCapabilities", "Slice", "MemoryMax", "MemoryHigh",
    "MemoryLimit", "CPUQuota", "CPUWeight", "TasksMax", "IOWeight", "IgnoreSIGPIPE",
    "StartLimitIntervalSec", "StartLimitBurst", "UtmpIdentifier", "UtmpMode", "Delegate",
    "RestartMode", "PAMName", "DelegateSubgroup", "LimitCPU", "LimitFSIZE", "LimitDATA", "LimitSTACK", "LimitCORE", "LimitRSS",
    "LimitNOFILE", "LimitAS", "LimitNPROC", "LimitMEMLOCK", "LimitLOCKS", "LimitSIGPENDING",
    "LimitMSGQUEUE", "LimitNICE", "LimitRTPRIO", "LimitRTTIME", "SendSIGHUP", "TimeoutAbortSec",
    "FileDescriptorStoreMax", "SystemCallFilter", "SystemCallErrorNumber", "SystemCallArchitectures",
    "RestrictAddressFamilies", "RestrictNamespaces", "MemoryDenyWriteExecute", "LockPersonality",
    "RestrictRealtime", "RestrictSUIDSGID", "ProtectClock", "ProtectKernelModules", "ProtectKernelLogs",
    "ProtectKernelTunables", "ProtectControlGroups", "ProtectHostname", "ProtectProc", "ProcSubset",
    "PrivateDevices", "PrivateNetwork", "PrivateIPC", "PrivateMounts", "DevicePolicy", "DeviceAllow",
    "IPAddressAllow", "IPAddressDeny", "MemoryPressureWatch", "MemoryPressureThresholdSec",
];

fn longrun_type(unit: &Unit) -> bool {
    let ty = unit.get("Service", "Type").unwrap_or_else(|| {
        if unit.file.get("Service", "ExecStart").is_none() {
            "oneshot".into()
        } else if unit.file.get("Service", "BusName").is_some() {
            "dbus".into()
        } else {
            "simple".into()
        }
    });
    ty != "oneshot"
}

/// Names of s6-rc services defined in the extra source directories.
fn extra_names(cfg: &Config) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for dir in &cfg.extra_source {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.path().join("type").is_file() {
                    out.insert(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    out
}

fn ms(usec: u64) -> u64 {
    if usec == u64::MAX {
        0
    } else {
        usec / 1000
    }
}

pub fn plan(cfg: &Config) -> Plan {
    let ld = Loader::new(cfg);
    let mut plan = Plan { extra_names: extra_names(cfg), ..Default::default() };

    // Seed: every concrete service and target, plus the default target.
    let mut queue: Vec<String> = ld
        .list_names()
        .into_iter()
        .filter(|n| {
            let ty = loader::unit_type(n);
            matches!(ty, "service" | "target" | "timer" | "path" | "mount" | "swap" | "automount")
                && !n.contains("@.")
        })
        .collect();
    let def = ld.load("default.target");
    if def.is_loaded() {
        plan.default_target = Some(def.id.clone());
        queue.push(def.id.clone());
    }

    let mut loaded: BTreeMap<String, Unit> = BTreeMap::new();
    // Raw dependency names before filtering: id -> (requires, wants, after, before)
    type Raw = (Vec<String>, Vec<String>, Vec<String>, Vec<String>);
    let mut raw: BTreeMap<String, Raw> = BTreeMap::new();
    // socket name -> service it activates
    let mut socket_service: BTreeMap<String, String> = BTreeMap::new();

    while let Some(name) = queue.pop() {
        if loaded.contains_key(&name) || plan.skipped.contains_key(&name) || plan.aliases.contains_key(&name) {
            continue;
        }
        if plan.extra_names.contains(&name) {
            plan.skipped.insert(name, "provided by extra s6-rc source".into());
            continue;
        }
        let unit = ld.load(&name);
        match unit.load_state {
            LoadState::Loaded => {}
            LoadState::Masked => {
                plan.skipped.insert(name, "masked".into());
                continue;
            }
            LoadState::NotFound => {
                plan.skipped.insert(name, "not found".into());
                continue;
            }
            LoadState::BadSetting => {
                plan.skipped.insert(name, unit.load_error.clone().unwrap_or_default());
                continue;
            }
        }
        if unit.id != name {
            plan.aliases.insert(name, unit.id.clone());
            queue.push(unit.id.clone());
            continue;
        }
        if cfg.is_excluded(&unit.id) {
            plan.skipped.insert(name, "excluded by configuration".into());
            continue;
        }
        // Transient units (systemd-run) are supervised by the daemon from
        // RUN_DIR/dyn; in the database they would collide with that service
        // directory and make s6-rc-update fail.
        if unit.fragment.as_ref().map(|f| f.starts_with(&cfg.transient_dir)).unwrap_or(false) {
            plan.skipped.insert(name, "transient (supervised on demand by the daemon)".into());
            continue;
        }
        if plan.extra_names.contains(&unit.id) {
            plan.skipped.insert(name, "provided by extra s6-rc source".into());
            continue;
        }

        match unit.utype() {
            "service" | "target" | "mount" | "swap" => {}
            "path" => {
                let t = uinitconverter_base::pathspec::trigger_unit(&unit);
                plan.triggers.insert(unit.id.clone(), t.clone());
                queue.push(t);
            }
            "automount" => {
                // No autofs: the matching .mount is mounted eagerly instead.
                let m = format!("{}.mount", unit.id.trim_end_matches(".automount"));
                socket_service.insert(unit.id.clone(), m.clone());
                queue.push(m);
                plan.skipped.insert(name, "automount: the .mount is mounted at boot instead".into());
                continue;
            }
            "timer" => {
                let t = uinitconverter_base::timerspec::trigger_unit(&unit);
                plan.triggers.insert(unit.id.clone(), t.clone());
                queue.push(t);
            }
            "socket" => {
                let svc = unit.get("Socket", "Service").unwrap_or_else(|| {
                    let stem = unit.id.trim_end_matches(".socket");
                    if unit.get_bool("Socket", "Accept", false) {
                        format!("{stem}@.service")
                    } else {
                        format!("{stem}.service")
                    }
                });
                if svc.contains("@.") {
                    plan.skipped
                        .insert(name, "Accept=yes sockets are not supported yet".into());
                } else {
                    socket_service.insert(unit.id.clone(), svc.clone());
                    queue.push(svc);
                    plan.skipped.insert(name, "socket is bound by its service's runner".into());
                }
                continue;
            }
            other => {
                plan.skipped.insert(name, format!("unit type .{other} is not translated"));
                continue;
            }
        }

        let mut requires = unit.get_words("Unit", "Requires");
        requires.extend(unit.get_words("Unit", "Requisite"));
        requires.extend(unit.get_words("Unit", "BindsTo"));
        requires.extend(ld.dir_deps(&unit, "requires"));
        let mut wants = unit.get_words("Unit", "Wants");
        wants.extend(unit.get_words("Unit", "Upholds"));
        wants.extend(ld.dir_deps(&unit, "wants"));
        wants.extend(ld.dir_deps(&unit, "upholds"));
        let mut after = unit.get_words("Unit", "After");
        let before = unit.get_words("Unit", "Before");
        if matches!(unit.utype(), "service" | "timer" | "path") && unit.get_bool("Unit", "DefaultDependencies", true) {
            requires.push("sysinit.target".into());
            after.push("basic.target".into());
        }
        // Type=dbus implies Requires=/After=dbus.socket.
        if unit.utype() == "service" && (unit.get("Service", "Type").as_deref() == Some("dbus")
            || (unit.get("Service", "Type").is_none() && unit.get("Service", "BusName").is_some()))
        {
            requires.push("dbus.socket".into());
        } else if unit.utype() == "service" && unit.get("Service", "BusName").is_some() {
            // systemd starts BusName= services (hostnamed, logind, polkit) by
            // bus activation, so the bus is up first. Here a socket unit or
            // WantedBy= may start them at boot before dbus-daemon has made
            // its socket; sd-bus then waits with an inotify watch on
            // /run/dbus, which their SELinux domains may not have, and the
            // daemon runs without a bus. Wants= + After= dbus.socket.
            wants.push("dbus.socket".into());
            after.push("dbus.socket".into());
        }
        for n in requires.iter().chain(wants.iter()) {
            queue.push(n.clone());
        }
        raw.insert(unit.id.clone(), (requires, wants, after, before));
        loaded.insert(unit.id.clone(), unit);
    }

    let resolve = |n: &str, plan: &Plan| -> Option<String> {
        if let Some(m) = cfg.mapped_dep(n) {
            if plan.extra_names.contains(m) {
                return Some(m.to_string());
            }
        }
        let n = plan.aliases.get(n).map(String::as_str).unwrap_or(n);
        let n = socket_service.get(n).map(String::as_str).unwrap_or(n);
        let n = plan.aliases.get(n).map(String::as_str).unwrap_or(n);
        (loaded.contains_key(n) || plan.extra_names.contains(n)).then(|| n.to_string())
    };
    // A dependency on a socket, which resolve() turns into its service.
    let via_socket = |n: &str, plan: &Plan| -> bool {
        let n = plan.aliases.get(n).map(String::as_str).unwrap_or(n);
        n.ends_with(".socket") && socket_service.contains_key(n)
    };
    // Services whose runner can hold sockets: on_demand candidates.
    let mut socket_activated: BTreeSet<String> = BTreeSet::new();

    // Ordering: After= plus reversed Before= of other units.
    let mut after_sets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, (_, _, after, before)) in &raw {
        for a in after {
            if let Some(a) = resolve(a, &plan) {
                after_sets.entry(id.clone()).or_default().insert(a);
            }
        }
        for b in before {
            if let Some(b) = resolve(b, &plan) {
                after_sets.entry(b).or_default().insert(id.clone());
            }
        }
    }

    let mount_points: Vec<(String, String)> = loaded
        .iter()
        .filter(|(_, u)| u.utype() == "mount")
        .map(|(id, u)| (id.clone(), uinitconverter_base::mountunit::where_of(u)))
        .collect();

    for (id, unit) in &loaded {
        let (requires, wants, _, _) = &raw[id];
        let after = after_sets.get(id).cloned().unwrap_or_default();
        let mut deps = BTreeSet::new();
        let (mut by_socket, mut by_name) = (BTreeSet::new(), BTreeSet::new());
        let mut warnings = Vec::new();
        let is_target = unit.utype() == "target";

        for r in requires {
            if let Some(d) = resolve(r, &plan) {
                if via_socket(r, &plan) { &mut by_socket } else { &mut by_name }.insert(d.clone());
                deps.insert(d);
            }
        }
        for w in wants {
            if let Some(d) = resolve(w, &plan) {
                // Targets pull in everything they want; services only wait for
                // wanted units they are also ordered after.
                if is_target || after.contains(&d) {
                    if via_socket(w, &plan) { &mut by_socket } else { &mut by_name }.insert(d.clone());
                    deps.insert(d);
                }
            }
        }
        let socket_deps: BTreeSet<String> = by_socket.difference(&by_name).cloned().collect();
        // RequiresMountsFor=: depend on the mount units covering those paths.
        for path in unit.get_words("Unit", "RequiresMountsFor") {
            for (m, w) in &mount_points {
                if w != "/" && (path == *w || path.starts_with(&format!("{w}/"))) {
                    deps.insert(m.clone());
                }
            }
        }
        deps.remove(id);
        // Timers and path units start their unit through the daemon.
        if matches!(unit.utype(), "timer" | "path") && plan.extra_names.contains(&cfg.daemon_service) {
            deps.insert(cfg.daemon_service.clone());
        }
        // FDSTORE=1 goes to the uinitconverter-fdstore service. journald starts
        // before it otherwise (DefaultDependencies=no): "fd store not reachable".
        // So do the sockets the runner binds: unheld, a service that exits
        // (dmeventd -R) is not started again on activity.
        let fdstore_max: usize =
            unit.get("Service", "FileDescriptorStoreMax").and_then(|v| v.parse().ok()).unwrap_or(0);
        // Service= may name an alias of the service.
        let has_sockets = socket_service.iter().any(|(s, svc)| {
            s.ends_with(".socket") && plan.aliases.get(svc).unwrap_or(svc) == id
        });
        if unit.utype() == "service"
            && (fdstore_max > 0 || has_sockets)
            && plan.extra_names.contains(FDSTORE_SERVICE)
        {
            deps.insert(FDSTORE_SERVICE.into());
        }

        let kind = if is_target {
            if *id == "sysinit.target" && plan.extra_names.contains(&cfg.sysinit_bundle) {
                deps.insert(cfg.sysinit_bundle.clone());
            }
            // fstab is mounted by the early mount-fs service.
            if *id == "local-fs.target" && plan.extra_names.contains("mount-fs") {
                deps.insert("mount-fs".into());
            }
            if *id == "sysinit.target" && cfg.journal && loaded.contains_key("systemd-journald.service") {
                deps.insert("systemd-journald.service".into());
            }
            Kind::Bundle
        } else if matches!(unit.utype(), "mount" | "swap") {
            Kind::Oneshot
        } else if matches!(unit.utype(), "timer" | "path") || longrun_type(unit) {
            Kind::Longrun
        } else {
            Kind::Oneshot
        };

        if !is_target {
            for key in unit.file.keys_in("Unit") {
                if !UNIT_KEYS.contains(&key) && !key.starts_with("Condition") && !key.starts_with("Assert") {
                    warnings.push(format!("[Unit] {key}= is ignored"));
                }
            }
            for key in unit.file.keys_in("Service") {
                if !SERVICE_KEYS.contains(&key) {
                    warnings.push(format!("[Service] {key}= is ignored"));
                }
            }
            for key in unit.file.keys_in("Timer") {
                if !TIMER_KEYS.contains(&key) {
                    warnings.push(format!("[Timer] {key}= is ignored"));
                }
            }
            for (sec, known) in [("Path", PATH_KEYS), ("Mount", MOUNT_KEYS), ("Swap", SWAP_KEYS)] {
                for key in unit.file.keys_in(sec) {
                    if !known.contains(&key) {
                        warnings.push(format!("[{sec}] {key}= is ignored"));
                    }
                }
            }
            for (raw, spec) in uinitconverter_base::timerspec::calendars(unit) {
                if spec.is_none() {
                    warnings.push(format!("OnCalendar={raw} cannot be parsed"));
                }
            }
            for sec in unit.file.sections() {
                if !["Unit", "Service", "Timer", "Path", "Mount", "Swap", "Install"].contains(&sec) && !sec.starts_with("X-") {
                    warnings.push(format!("[{sec}] section is ignored"));
                }
            }
            if unit.get("Unit", "Conflicts").is_some() {
                warnings.push("Conflicts= is not enforced".into());
            }
            let ty = unit.get("Service", "Type").unwrap_or_default();
            if ty == "forking" && unit.get("Service", "PIDFile").is_none() {
                warnings.push("Type=forking without PIDFile=: main PID is guessed".into());
            }
        }

        let log_atom = (kind == Kind::Longrun
            && unit.utype() == "service"
            && !cfg.journal
            && cfg.log_dir.is_some()
            && !uses_tty_or_file(unit))
            .then(|| format!("{id}.log"));
        let stop_atom = (kind == Kind::Longrun
            && unit.utype() == "service"
            && !unit.get_all("Service", "ExecStop").is_empty())
        .then(|| stop_atom(id))
        .filter(|n| !plan.extra_names.contains(n));
        // Without an fd store to hold the sockets, the runner just starts it.
        if kind == Kind::Longrun && unit.utype() == "service" && has_sockets {
            socket_activated.insert(id.clone());
        }

        plan.units.insert(
            id.clone(),
            Planned {
                id: id.clone(),
                kind,
                deps,
                log_atom,
                stop_atom,
                socket_deps,
                on_demand: false,
                warnings,
                timings: Timings::of(unit),
            },
        );
    }

    break_cycles(&mut plan);
    let aliases = plan.aliases.clone();
    resolve_conflicts(&mut plan, &loaded, &aliases);
    plan.boot = boot_closure(&plan);
    mark_on_demand(&mut plan, &socket_activated);
    plan
}

/// Planned::on_demand: of the services that hold sockets, those the boot
/// reaches only through socket dependencies (sockets.target wants
/// virtlogd.socket; nothing wants virtlogd.service itself). What such a
/// service requires does not count, as systemd does not start it at boot
/// either (iscsiuio Requires=iscsid); the runner starts it on activation.
fn mark_on_demand(plan: &mut Plan, socket_activated: &BTreeSet<String>) {
    let mut started = BTreeSet::new();
    let mut listening = BTreeSet::new();
    let mut stack: Vec<String> = plan.default_target.iter().cloned().collect();
    while let Some(n) = stack.pop() {
        let Some(p) = plan.units.get(&n) else { continue };
        if !started.insert(n) {
            continue;
        }
        for d in &p.deps {
            if p.socket_deps.contains(d) && socket_activated.contains(d) {
                listening.insert(d.clone());
            } else {
                stack.push(d.clone());
            }
        }
    }
    for id in listening.difference(&started) {
        if let Some(p) = plan.units.get_mut(id) {
            p.on_demand = true;
        }
    }
}

/// default.target and everything it reaches through bundle contents and
/// dependencies, with their loggers and `.stop` oneshots.
fn boot_closure(plan: &Plan) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = plan.default_target.iter().cloned().collect();
    while let Some(n) = stack.pop() {
        let Some(p) = plan.units.get(&n) else { continue };
        if !seen.insert(n) {
            continue;
        }
        stack.extend(p.deps.iter().cloned());
        if let Some(log) = &p.log_atom {
            seen.insert(log.clone());
        }
        if let Some(stop) = &p.stop_atom {
            seen.insert(stop.clone());
        }
    }
    seen
}

/// Conflicts=: if both units would start at boot, keep the one declaring the
/// conflict (e.g. gdm.service over getty@tty1.service) and drop the other
/// from every target, like systemd's transaction would.
fn resolve_conflicts(plan: &mut Plan, loaded: &BTreeMap<String, Unit>, aliases: &BTreeMap<String, String>) {
    let Some(def) = plan.default_target.clone() else { return };
    let mut boot: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![def];
    while let Some(n) = stack.pop() {
        if !boot.insert(n.clone()) {
            continue;
        }
        if let Some(p) = plan.units.get(&n) {
            stack.extend(p.deps.iter().cloned());
        }
    }
    // A unit already dropped neither keeps nor drops: of a mutual
    // Conflicts= pair one stays (the first by name), not neither.
    let mut drops: Vec<(String, String)> = Vec::new();
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    for (id, unit) in loaded {
        if !boot.contains(id) || dropped.contains(id) {
            continue;
        }
        for c in unit.get_words("Unit", "Conflicts") {
            let c = aliases.get(&c).cloned().unwrap_or(c);
            if c != *id && boot.contains(&c) && plan.units.contains_key(&c) && dropped.insert(c.clone()) {
                drops.push((id.clone(), c));
            }
        }
    }
    for (keeper, dropped) in drops {
        for p in plan.units.values_mut() {
            if p.kind == Kind::Bundle {
                p.deps.remove(&dropped);
            }
        }
        if let Some(p) = plan.units.get_mut(&dropped) {
            p.warnings.push(format!("not started at boot: conflicts with {keeper}"));
        }
    }
}

fn uses_tty_or_file(unit: &Unit) -> bool {
    let out = unit.get("Service", "StandardOutput").unwrap_or_default();
    let input = unit.get("Service", "StandardInput").unwrap_or_default();
    input.starts_with("tty")
        || out.starts_with("tty")
        || out.starts_with("file:")
        || out.starts_with("append:")
        || out.starts_with("truncate:")
        || out == "null"
}

/// s6-rc refuses dependency cycles; systemd resolves them at runtime by
/// dropping jobs. Drop the edge that closes each cycle and report it.
fn break_cycles(plan: &mut Plan) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Active,
        Done,
    }
    let names: Vec<String> = plan.units.keys().cloned().collect();
    let mut mark: BTreeMap<String, Mark> = names.iter().map(|n| (n.clone(), Mark::New)).collect();
    let mut removed: Vec<(String, String)> = Vec::new();

    fn visit(
        n: &str,
        plan: &Plan,
        mark: &mut BTreeMap<String, Mark>,
        removed: &mut Vec<(String, String)>,
        stack: &mut Vec<String>,
    ) {
        mark.insert(n.to_string(), Mark::Active);
        stack.push(n.to_string());
        if let Some(p) = plan.units.get(n) {
            for d in &p.deps {
                if removed.iter().any(|(a, b)| a == n && b == d) {
                    continue;
                }
                match mark.get(d).copied() {
                    Some(Mark::New) => visit(d, plan, mark, removed, stack),
                    Some(Mark::Active) => removed.push((n.to_string(), d.clone())),
                    _ => {}
                }
            }
        }
        stack.pop();
        mark.insert(n.to_string(), Mark::Done);
    }

    for n in &names {
        if mark[n] == Mark::New {
            visit(n, plan, &mut mark, &mut removed, &mut Vec::new());
        }
    }
    for (a, b) in removed {
        if let Some(p) = plan.units.get_mut(&a) {
            p.deps.remove(&b);
            p.warnings.push(format!("dependency on {b} dropped to break an ordering cycle"));
        }
        plan.broken_cycles.push(format!("{a} -> {b}"));
    }
}

/// Plan the units and hand the plan to the INIT= backend, which writes
/// its service definitions and installs them as the boot database (and
/// the live one when `update_live`).
pub fn regenerate(cfg: &Config, update_live: bool) -> Result<GenResult, String> {
    let plan = plan(cfg);
    // Config::load accepts only config::INITS, which is just "s6" so far;
    // another backend is chosen here by cfg.init.
    s6::regenerate(cfg, plan, update_live)
}

pub fn report(plan: &Plan) -> String {
    let mut s = String::new();
    let count = |k: Kind| plan.units.values().filter(|p| p.kind == k).count();
    let clean = plan
        .units
        .values()
        .filter(|p| p.kind != Kind::Bundle && p.warnings.is_empty())
        .count();
    let _ = writeln!(
        s,
        "translated: {} longrun, {} oneshot, {} bundle ({} services without warnings)",
        count(Kind::Longrun),
        count(Kind::Oneshot),
        count(Kind::Bundle),
        clean
    );
    let _ = writeln!(s, "skipped: {}", plan.skipped.len());
    if !plan.broken_cycles.is_empty() {
        let _ = writeln!(s, "cycles broken: {}", plan.broken_cycles.join(", "));
    }
    let _ = writeln!(s);
    for p in plan.units.values() {
        let _ = writeln!(s, "{} [{}]", p.id, p.kind.as_str());
        if p.on_demand {
            let _ = writeln!(s, "    on demand: started on the first activity on its sockets");
        }
        if !p.deps.is_empty() {
            let label = if p.kind == Kind::Bundle { "contents" } else { "depends" };
            let deps: Vec<&str> = p.deps.iter().map(String::as_str).collect();
            let _ = writeln!(s, "    {label}: {}", deps.join(" "));
        }
        for w in &p.warnings {
            let _ = writeln!(s, "    warning: {w}");
        }
    }
    let _ = writeln!(s, "\nskipped units:");
    for (n, why) in &plan.skipped {
        let _ = writeln!(s, "    {n}: {why}");
    }
    s
}

pub struct GenResult {
    pub db_dir: PathBuf,
    pub plan: Plan,
    pub updated_live: bool,
    /// What the repository commands changed in the set on their own
    /// (s6-rc-set-fix moving services, essentials following their flag).
    pub set_changes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_name_orders_after_dbus() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (units, extra) = (tmp.join("units"), tmp.join("extra"));
        fs::create_dir_all(&units).unwrap();
        fs::create_dir_all(extra.join("dbus")).unwrap();
        fs::write(extra.join("dbus/type"), "longrun\n").unwrap();
        let write = |n: &str, s: &str| fs::write(units.join(n), s).unwrap();
        // hostnamed: Type=notify, BusName=, started by its socket at boot.
        write("named.service", "[Unit]\nDefaultDependencies=no\n[Service]\nType=notify\nBusName=org.example.N\nExecStart=/bin/true\n");
        write("plain.service", "[Unit]\nDefaultDependencies=no\n[Service]\nType=notify\nExecStart=/bin/true\n");
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units];
        cfg.extra_source = vec![extra];

        let plan = plan(&cfg);
        assert!(plan.units["named.service"].deps.contains("dbus"));
        assert!(!plan.units["plain.service"].deps.contains("dbus"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A service with a .socket holds it in the fd store, so it starts
    /// after the store even with DefaultDependencies=no (dm-event).
    #[test]
    fn socket_service_orders_after_fdstore() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-sock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (units, extra) = (tmp.join("units"), tmp.join("extra"));
        fs::create_dir_all(&units).unwrap();
        fs::create_dir_all(extra.join(FDSTORE_SERVICE)).unwrap();
        fs::write(extra.join(FDSTORE_SERVICE).join("type"), "longrun\n").unwrap();
        let write = |n: &str, s: &str| fs::write(units.join(n), s).unwrap();
        write("ev.socket", "[Unit]\nDefaultDependencies=no\n[Socket]\nListenFIFO=/run/ev\n");
        write("ev.service", "[Unit]\nDefaultDependencies=no\nRequires=ev.socket\n[Service]\nExecStart=/bin/true\n");
        write("plain.service", "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/bin/true\n");
        // Service= naming an alias of the service.
        write("al.socket", "[Unit]\nDefaultDependencies=no\n[Socket]\nListenFIFO=/run/al\nService=al-alias.service\n");
        write("al.service", "[Unit]\nDefaultDependencies=no\nRequires=al.socket\n[Service]\nExecStart=/bin/true\n");
        std::os::unix::fs::symlink("al.service", units.join("al-alias.service")).unwrap();
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units];
        cfg.extra_source = vec![extra];

        let plan = plan(&cfg);
        assert!(plan.units["ev.service"].deps.contains(FDSTORE_SERVICE));
        assert!(plan.units["al.service"].deps.contains(FDSTORE_SERVICE));
        assert!(!plan.units["plain.service"].deps.contains(FDSTORE_SERVICE));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A service the boot reaches only through its enabled socket waits
    /// for activity; one also wanted by name starts at boot. What an
    /// on-demand service requires does not count.
    #[test]
    fn socket_only_services_are_on_demand() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-ondemand-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (units, extra) = (tmp.join("units"), tmp.join("extra"));
        fs::create_dir_all(units.join("sockets.target.wants")).unwrap();
        fs::create_dir_all(units.join("multi-user.target.wants")).unwrap();
        fs::create_dir_all(extra.join(FDSTORE_SERVICE)).unwrap();
        fs::write(extra.join(FDSTORE_SERVICE).join("type"), "longrun\n").unwrap();
        let write = |n: &str, s: &str| fs::write(units.join(n), s).unwrap();
        let enable = |t: &str, n: &str| {
            std::os::unix::fs::symlink(format!("../{n}"), units.join(format!("{t}.wants/{n}"))).unwrap()
        };
        write("multi-user.target", "[Unit]\nWants=sockets.target\n");
        write("sockets.target", "[Unit]\nDescription=s\n");
        std::os::unix::fs::symlink("multi-user.target", units.join("default.target")).unwrap();
        for n in ["lazy", "named", "req", "user"] {
            write(&format!("{n}.socket"), &format!("[Unit]\nDefaultDependencies=no\n[Socket]\nListenFIFO=/run/{n}\n"));
            enable("sockets.target", &format!("{n}.socket"));
        }
        let svc = "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/bin/true\n";
        write("lazy.service", svc);
        write("named.service", svc);
        enable("multi-user.target", "named.service");
        // Started by name (enabled), requires lazy.socket only: lazy stays on demand.
        write("starter.service", "[Unit]\nDefaultDependencies=no\nRequires=lazy.socket\n[Service]\nExecStart=/bin/true\n");
        enable("multi-user.target", "starter.service");
        // user is on demand and requires req.service by name: req stays on demand.
        write("user.service", "[Unit]\nDefaultDependencies=no\nRequires=req.service\n[Service]\nExecStart=/bin/true\n");
        write("req.service", svc);
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units];
        cfg.extra_source = vec![extra];

        let p = plan(&cfg);
        for u in ["lazy.service", "user.service", "req.service"] {
            assert!(p.units[u].on_demand, "{u} should be on demand");
        }
        for u in ["named.service", "starter.service"] {
            assert!(!p.units[u].on_demand, "{u} should start at boot");
        }
        assert!(p.boot.contains("lazy.service"));
        let run = |id: &str| {
            let u = &p.units[id];
            longrun_files(&cfg, id, &u.timings, u.on_demand)[0].1.clone()
        };
        assert!(run("lazy.service").contains(" run --on-demand "), "{}", run("lazy.service"));
        assert!(!run("named.service").contains("--on-demand"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// flag-recommended goes to what default.target reaches (enabled
    /// units and what they pull in), not to units nothing at boot wants.
    #[test]
    fn boot_closure_marks_recommended() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-boot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let units = tmp.join("units");
        fs::create_dir_all(units.join("multi-user.target.wants")).unwrap();
        let write = |n: &str, s: &str| fs::write(units.join(n), s).unwrap();
        write("multi-user.target", "[Unit]\nDescription=m\n");
        std::os::unix::fs::symlink("multi-user.target", units.join("default.target")).unwrap();
        write("on.service", "[Unit]\nDefaultDependencies=no\nRequires=helper.service\nAfter=helper.service\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target\n");
        write("helper.service", "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/bin/true\n");
        write("off.service", "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target\n");
        std::os::unix::fs::symlink("../on.service", units.join("multi-user.target.wants/on.service")).unwrap();
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units];
        cfg.extra_source = vec![];

        let plan = plan(&cfg);
        for u in ["multi-user.target", "on.service", "helper.service"] {
            assert!(plan.boot.contains(u), "{u} not in the boot closure");
        }
        assert!(!plan.boot.contains("off.service"));
        let src = tmp.join("source");
        write_source(&cfg, &plan, &src).unwrap();
        assert!(src.join("on.service/flag-recommended").exists());
        assert!(src.join("helper.service/flag-recommended").exists());
        assert!(!src.join("off.service/flag-recommended").exists());
        assert!(!src.join("multi-user.target/flag-recommended").exists());
        assert!(!src.join(&cfg.default_bundle).exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A longrun with ExecStop= gets UNIT.stop: a oneshot that depends on
    /// it, runs `stop-atom` as its down, and is in the bundles beside it.
    /// Nothing depends on it, and oneshots and ExecStop=-less longruns get
    /// none.
    #[test]
    fn stop_atom_for_longrun_exec_stop() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-stop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let units = tmp.join("units");
        fs::create_dir_all(units.join("multi-user.target.wants")).unwrap();
        let write = |n: &str, s: &str| fs::write(units.join(n), s).unwrap();
        write("multi-user.target", "[Unit]\nDescription=m\n");
        std::os::unix::fs::symlink("multi-user.target", units.join("default.target")).unwrap();
        write("db.service", "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/bin/sleep 9\nExecStop=/bin/true\nTimeoutStopSec=7\n");
        write("app.service", "[Unit]\nDefaultDependencies=no\nRequires=db.service\nAfter=db.service\n[Service]\nExecStart=/bin/sleep 9\n");
        write("once.service", "[Unit]\nDefaultDependencies=no\n[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/bin/true\nExecStop=/bin/true\n");
        for u in ["db.service", "app.service", "once.service"] {
            std::os::unix::fs::symlink(format!("../{u}"), units.join("multi-user.target.wants").join(u)).unwrap();
        }
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units];
        cfg.extra_source = vec![];

        let plan = plan(&cfg);
        assert_eq!(plan.units["db.service"].stop_atom.as_deref(), Some("db.service.stop"));
        assert_eq!(plan.units["app.service"].stop_atom, None);
        assert_eq!(plan.units["once.service"].stop_atom, None);
        assert!(plan.boot.contains("db.service.stop"));
        let src = tmp.join("source");
        write_source(&cfg, &plan, &src).unwrap();
        let stop = src.join("db.service.stop");
        assert_eq!(fs::read_to_string(stop.join("type")).unwrap(), "oneshot\n");
        assert!(stop.join("dependencies.d/db.service").exists());
        assert_eq!(fs::read_dir(stop.join("dependencies.d")).unwrap().count(), 1);
        assert_eq!(fs::read_to_string(stop.join("up")).unwrap(), "");
        assert!(fs::read_to_string(stop.join("down")).unwrap().contains(" stop-atom -- db.service\n"));
        assert_eq!(fs::read_to_string(stop.join("timeout-down")).unwrap(), "17000\n");
        assert!(stop.join("flag-recommended").exists());
        assert!(src.join("multi-user.target/contents.d/db.service.stop").exists());
        assert!(src.join("app.service/dependencies.d/db.service").exists());
        assert!(!src.join("app.service/dependencies.d/db.service.stop").exists());
        assert!(!src.join("once.service.stop").exists());
        let _ = fs::remove_dir_all(&tmp);
    }
}
