//! Translate systemd units into an s6-rc source tree, compile it and switch
//! the live database to it.
//!
//! Mapping:
//!   .service (long-running)  -> longrun  (run/finish call `uinitconverter-runner run|finish`)
//!   .service Type=oneshot    -> oneshot  (up/down call `uinitconverter-runner up|down`)
//!   .target                  -> bundle   (contents = Wants/Requires + .wants/.requires dirs)
//!   enabled .socket          -> its service is pulled in instead (sockets are bound by the runner)
//!   dependencies             -> Requires/Requisite/BindsTo, plus Wants that are also ordered After
//!   long-running ExecStop=   -> also a oneshot UNIT.stop that depends on it; its `down` runs ExecStop=


use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use uinitconverter_base::config::Config;
pub use uinitconverter_base::index::{stop_atom, unit_arg, Index, Kind};
use uinitconverter_base::loader::{self, LoadState, Loader, Unit};
use uinitconverter_base::util;
use uinitconverter_install as install;

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

/// The files of a longrun's directory that s6-rc's source format and a
/// plain s6 service directory share: (name, contents, executable).
pub fn longrun_files(cfg: &Config, id: &str, t: &Timings) -> Vec<(&'static str, String, bool)> {
    let args = cfg.runner_args();
    let arg = unit_arg(id);
    if id.ends_with(".timer") || id.ends_with(".path") {
        let sub = if id.ends_with(".timer") { "timer-run" } else { "path-run" };
        let timer_runner = cfg.timer_runner.display();
        return vec![
            ("run", format!("#!/bin/sh\nexec {timer_runner} {sub}{args} {arg}\n"), true),
            ("notification-fd", "3\n".into(), false),
        ];
    }
    let runner = cfg.runner.display();
    let mut out = vec![
        ("run", format!("#!/bin/sh\nexec {runner} run{args} {arg}\n"), true),
        ("finish", format!("#!/bin/sh\nexec {runner} finish{args} {arg} \"$@\"\n"), true),
        ("notification-fd", "3\n".into(), false),
        ("timeout-up", format!("{}\n", t.up_ms), false),
        ("timeout-down", format!("{}\n", t.down_ms), false),
        ("timeout-kill", format!("{}\n", t.down_ms), false),
        ("timeout-finish", format!("{}\n", t.finish_ms), false),
    ];
    if let Some(sig) = &t.down_signal {
        out.push(("down-signal", format!("{sig}\n"), false));
    }
    out
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
        let mut warnings = Vec::new();
        let is_target = unit.utype() == "target";

        for r in requires {
            if let Some(r) = resolve(r, &plan) {
                deps.insert(r);
            }
        }
        for w in wants {
            if let Some(w) = resolve(w, &plan) {
                // Targets pull in everything they want; services only wait for
                // wanted units they are also ordered after.
                if is_target || after.contains(&w) {
                    deps.insert(w);
                }
            }
        }
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

        plan.units.insert(
            id.clone(),
            Planned {
                id: id.clone(),
                kind,
                deps,
                log_atom,
                stop_atom,
                warnings,
                timings: Timings::of(unit),
            },
        );
    }

    break_cycles(&mut plan);
    let aliases = plan.aliases.clone();
    resolve_conflicts(&mut plan, &loaded, &aliases);
    plan.boot = boot_closure(&plan);
    plan
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

fn write_file(path: &Path, content: &str, exec: bool) -> std::io::Result<()> {
    fs::write(path, content)?;
    if exec {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub fn write_source(cfg: &Config, plan: &Plan, dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let runner = cfg.runner.display().to_string();
    let args = cfg.runner_args();

    for p in plan.units.values() {
        let sdir = dir.join(&p.id);
        fs::create_dir_all(&sdir)?;
        write_file(&sdir.join("type"), &format!("{}\n", p.kind.as_str()), false)?;
        let list_dir = if p.kind == Kind::Bundle { "contents.d" } else { "dependencies.d" };
        fs::create_dir_all(sdir.join(list_dir))?;
        for d in &p.deps {
            fs::write(sdir.join(list_dir).join(d), "")?;
            // A bundle brings up the `.stop` oneshots of its longruns too:
            // `s6-rc -p change` would otherwise take one down alone and run
            // the ExecStop= of a service that stays up.
            if p.kind == Kind::Bundle {
                if let Some(stop) = plan.units.get(d).and_then(|u| u.stop_atom.as_ref()) {
                    fs::write(sdir.join(list_dir).join(stop), "")?;
                }
            }
        }

        if p.kind != Kind::Bundle && plan.boot.contains(&p.id) {
            fs::write(sdir.join("flag-recommended"), "")?;
        }

        match p.kind {
            Kind::Bundle => {}
            Kind::Oneshot => {
                let arg = unit_arg(&p.id);
                write_file(&sdir.join("up"), &format!("{runner} up{args} {arg}\n"), false)?;
                write_file(&sdir.join("down"), &format!("{runner} down{args} {arg}\n"), false)?;
                write_file(&sdir.join("timeout-up"), &format!("{}\n", p.timings.up_ms), false)?;
                write_file(&sdir.join("timeout-down"), &format!("{}\n", p.timings.down_ms), false)?;
            }
            Kind::Longrun => {
                for (name, text, exec) in longrun_files(cfg, &p.id, &p.timings) {
                    write_file(&sdir.join(name), &text, exec)?;
                }
                if let Some(stop) = &p.stop_atom {
                    write_stop_atom(cfg, plan, p, &dir.join(stop))?;
                }
                if let (Some(log), Some(log_dir)) = (&p.log_atom, &cfg.log_dir) {
                    write_file(&sdir.join("producer-for"), &format!("{log}\n"), false)?;
                    let ldir = dir.join(log);
                    fs::create_dir_all(&ldir)?;
                    write_file(&ldir.join("type"), "longrun\n", false)?;
                    write_file(&ldir.join("consumer-for"), &format!("{}\n", p.id), false)?;
                    write_file(&ldir.join("notification-fd"), "3\n", false)?;
                    if plan.boot.contains(log) {
                        fs::write(ldir.join("flag-recommended"), "")?;
                    }
                    let logdir = util::sh_quote(&log_dir.join(&p.id).display().to_string());
                    write_file(
                        &ldir.join("run"),
                        &format!(
                            "#!/bin/sh\nmkdir -p {logdir} || exit 1\nexec s6-log -d3 n10 s1000000 T {logdir}\n"
                        ),
                        true,
                    )?;
                }
            }
        }
    }

    // Booted when /system-update exists (offline updates, dnf system-upgrade).
    let b = dir.join("uinitconverter-system-update");
    fs::create_dir_all(b.join("contents.d"))?;
    write_file(&b.join("type"), "bundle\n", false)?;
    // logind too: update tools may ask it to reboot when they are done.
    for u in ["system-update.target", "systemd-logind.service"] {
        if let Some(p) = plan.units.get(u) {
            fs::write(b.join("contents.d").join(u), "")?;
            if let Some(stop) = &p.stop_atom {
                fs::write(b.join("contents.d").join(stop), "")?;
            }
        }
    }

    // Index read by the daemon and the runner.
    let mut idx = String::new();
    for p in plan.units.values() {
        let _ = writeln!(
            idx,
            "unit\t{}\t{}\t{}",
            p.id,
            p.kind.as_str(),
            p.log_atom.as_deref().unwrap_or("-")
        );
    }
    for (a, id) in &plan.aliases {
        let _ = writeln!(idx, "alias\t{a}\t{id}");
    }
    for (t, u) in &plan.triggers {
        let _ = writeln!(idx, "trigger\t{t}\t{u}");
    }
    if let Some(def) = &plan.default_target {
        let _ = writeln!(idx, "default\t{def}");
    }
    fs::write(dir.join("..").join("index"), idx)?;
    fs::write(dir.join("..").join("report.txt"), report(plan))?;
    Ok(())
}

/// The `.stop` oneshot of a longrun (see index::stop_atom). Nothing depends
/// on it: s6-rc-update restarts a service that gains a dependency on a new
/// service, and an upgrade would have restarted every dependent of such a
/// longrun (gdm, with the session). So when s6-rc stops the longrun with
/// units that depend on it, its ExecStop= runs beside their stops rather
/// than after them; the longrun itself still goes down last.
fn write_stop_atom(cfg: &Config, plan: &Plan, p: &Planned, sdir: &Path) -> std::io::Result<()> {
    let runner = cfg.runner.display().to_string();
    let args = cfg.runner_args();
    fs::create_dir_all(sdir.join("dependencies.d"))?;
    write_file(&sdir.join("type"), "oneshot\n", false)?;
    fs::write(sdir.join("dependencies.d").join(&p.id), "")?;
    // Empty: nothing to do when it comes up.
    write_file(&sdir.join("up"), "", false)?;
    write_file(&sdir.join("down"), &format!("{runner} stop-atom{args} {}\n", unit_arg(&p.id)), false)?;
    // The runner ends ExecStop= at TimeoutStopSec= and always succeeds; a
    // failed down would keep s6-rc from stopping the service.
    let down = if p.timings.down_ms == 0 { 0 } else { p.timings.down_ms.saturating_add(10_000).min(u32::MAX as u64) };
    write_file(&sdir.join("timeout-down"), &format!("{down}\n"), false)?;
    if plan.boot.contains(&p.id) {
        fs::write(sdir.join("flag-recommended"), "")?;
    }
    Ok(())
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

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    } else {
        Err(format!(
            "{:?} failed: {}{}",
            cmd,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ))
    }
}

pub struct GenResult {
    pub db_dir: PathBuf,
    pub plan: Plan,
    pub updated_live: bool,
    /// What the repository commands changed in the set on their own
    /// (s6-rc-set-fix moving services, essentials following their flag).
    pub set_changes: Vec<String>,
}

/// Replace the symlink `link` with one to `target`, atomically.
fn swap_symlink(target: &Path, link: &Path) -> Result<(), String> {
    let name = link.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = link.with_file_name(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, link).map_err(|e| format!("{}: {e}", link.display()))
}

/// The stores the repository links, in s6-rc's order (a later store's
/// service overrides an earlier one's): the generated one, then the
/// hand-written ones that exist.
pub fn stores(cfg: &Config) -> Vec<PathBuf> {
    let mut v = vec![cfg.store_dir()];
    v.extend(cfg.extra_source.iter().filter(|d| d.is_dir()).cloned());
    v
}

/// The stores an existing repository links (REPO/stores/NNNN), or None
/// when there is no repository.
fn repo_stores(cfg: &Config) -> Option<Vec<PathBuf>> {
    let dir = cfg.repo_dir.join("stores");
    let mut names: Vec<String> = fs::read_dir(&dir).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Some(names.iter().filter_map(|n| fs::read_link(dir.join(n)).ok()).collect())
}

/// Create the repository, change its stores, or sync it with them.
fn repo_sync(cfg: &Config) -> Result<(), String> {
    let want = stores(cfg);
    // s6-rc-set-commit makes this bundle itself and refuses a store that
    // defines one (uinitconverter shipped /etc/uinitconverter/s6/default before).
    if let Some(d) = want.iter().map(|s| s.join(&cfg.default_bundle)).find(|d| d.exists()) {
        return Err(format!(
            "{}: s6-rc-set-commit makes the {} bundle from the set; remove this one",
            d.display(),
            cfg.default_bundle
        ));
    }
    let repo = cfg.repo_dir.as_os_str();
    match repo_stores(cfg) {
        None => {
            if let Some(parent) = cfg.repo_dir.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            run(Command::new("s6-rc-repo-init").arg("-r").arg(repo).args(&want))?;
        }
        Some(have) if have != want => {
            run(Command::new("s6-rc-repo-init").arg("-r").arg(repo).arg("-U").args(&want))?;
        }
        Some(_) => {
            run(Command::new("s6-rc-repo-sync").arg("-r").arg(repo))?;
        }
    }
    if !cfg.repo_dir.join("sources").join(&cfg.set_name).is_dir() {
        run(Command::new("s6-rc-set-new").arg("-r").arg(repo).arg(&cfg.set_name))?;
    }
    Ok(())
}


/// The early services are `always`: those with flag-essential, and the
/// members of SYSINIT_BUNDLE (uinitconverter init adds or removes syslog,
/// journal-relay and resolv there, so they carry no flag of their own).
/// A repository sync reads flags only for services it hasn't seen before,
/// so this keeps the set in step when they change; a service that is no
/// longer either goes back to `active`.
fn sync_essentials(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let sources = cfg.repo_dir.join("sources");
    let atomics = sources.join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let members: BTreeSet<String> = fs::read_dir(sources.join(".bundles").join(&cfg.sysinit_bundle).join("contents.d"))
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let mut up = Vec::new();
    let mut down = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let flagged = atomics.join(&name).join("flag-essential").exists() || members.contains(&name);
        match (flagged, install::set_rx(cfg, &name)) {
            (true, Some(rx)) if rx != "always" => up.push(name),
            (false, Some("always")) => down.push(name),
            _ => {}
        }
    }
    for (rx, names) in [("always", up), ("active", down)] {
        if names.is_empty() {
            continue;
        }
        run(Command::new("s6-rc-set-change")
            .arg("-r")
            .arg(&cfg.repo_dir)
            .arg("-e")
            .arg(&cfg.set_name)
            .arg(rx)
            .args(&names))?;
        changes.push(format!("{} -> {rx} (early boot service)", names.join(" ")));
    }
    Ok(())
}

/// A logger is `active` with its service (both were in the boot closure),
/// but `s6 set disable -p X` moves only X and what depends on it: the
/// logger is X's dependency, so it stays and boots with nothing to log.
/// Move a last consumer that is `active` while none of its producers is
/// active or always to `usable`. Starting the service still starts it (a
/// dependency), and set-fix -u makes it active again with the service.
fn sync_loggers(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let atomics = cfg.repo_dir.join("sources").join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let mut idle = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = atomics.join(&name);
        if name.starts_with('.') || dir.join("producer-for").exists() {
            continue;
        }
        let Ok(producers) = fs::read_to_string(dir.join("consumer-for")) else { continue };
        if install::set_rx(cfg, &name) == Some("active")
            && !producers
                .lines()
                .filter(|l| !l.is_empty())
                .any(|p| matches!(install::set_rx(cfg, p), Some("active" | "always")))
        {
            idle.push(name);
        }
    }
    if idle.is_empty() {
        return Ok(());
    }
    idle.sort();
    run(Command::new("s6-rc-set-change")
        .arg("-r")
        .arg(&cfg.repo_dir)
        .arg(&cfg.set_name)
        .arg("usable")
        .args(&idle))?;
    changes.push(format!("{} -> usable (logger of a service that is not active)", idle.join(" ")));
    Ok(())
}

/// A longrun's `.stop` oneshot (see write_stop_atom) takes its service's
/// prescription (active for always). `s6 set disable X` without -p leaves
/// it active, and set-fix -u would then enable X again, as the oneshot
/// depends on it; `s6 set enable X` leaves it usable, out of the default
/// bundle, so `s6 live reset` would take it down alone and run X's
/// ExecStop= while X stays up.
fn sync_stop_atoms(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let atomics = cfg.repo_dir.join("sources").join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let mut moves: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix(".stop") else { continue };
        if stop_atom(base) != name || !atomics.join(base).exists() {
            continue;
        }
        let want = match install::set_rx(cfg, base) {
            Some("always" | "active") => "active",
            Some(rx) => rx,
            None => continue,
        };
        if install::set_rx(cfg, &name) != Some(want) {
            moves.entry(want).or_default().push(name);
        }
    }
    for (rx, mut names) in moves {
        names.sort();
        run(Command::new("s6-rc-set-change")
            .arg("-r")
            .arg(&cfg.repo_dir)
            .arg(&cfg.set_name)
            .arg(rx)
            .args(&names))?;
        changes.push(format!("{} -> {rx} (with its service)", names.join(" ")));
    }
    Ok(())
}

/// Commit the set and install it: as the boot database, and (with a live
/// state) as the live one. s6-rc-set-fix -u first enables what active
/// services depend on: a new package's service may need one the set has
/// as usable, and the commit refuses an inconsistent set.
fn commit_and_install(cfg: &Config, update_live: bool, changes: &mut Vec<String>) -> Result<bool, String> {
    let repo = cfg.repo_dir.as_os_str();
    let out = run(Command::new("s6-rc-set-fix").arg("-r").arg(repo).arg("-v2").arg("-u").arg(&cfg.set_name))?;
    changes.extend(
        out.lines()
            .filter_map(|l| l.split_once("moving service ").map(|(_, r)| r.to_string())),
    );
    // Again for the services set-fix just enabled.
    sync_stop_atoms(cfg, changes)?;
    // --no-snapshot for both: s6-rc 0.8.0.2's s6rc_repo_setuptodate()
    // terminates the snapshot's path one byte too late (an uninitialized
    // byte is left in it), so with snapshots set-install randomly finds a
    // just-committed set "not up-to-date".
    run(Command::new("s6-rc-set-commit")
        .arg("-r")
        .arg(repo)
        .arg("--no-snapshot")
        .arg("-f")
        .arg("-D")
        .arg(&cfg.default_bundle)
        .arg(&cfg.set_name))?;

    let live = update_live && fs::symlink_metadata(cfg.live_dir.join("compiled")).is_ok();
    let install = |extra: &[&str]| -> Result<String, String> {
        run(Command::new("s6-rc-set-install")
            .arg("-r")
            .arg(repo)
            .arg("-c")
            .arg(cfg.boot_db())
            .arg("-l")
            .arg(&cfg.live_dir)
            // -K: the replaced database stays, as compiled.previous.
            .arg("-K")
            .arg("--no-snapshot")
            .args(extra)
            .arg(&cfg.set_name))
    };
    if !live {
        return install(&["--no-update"]).map(|_| false);
    }
    // s6-rc-update can fail after its switch ("unable to manage new service
    // directories": relink_supervised) or race s6-svscan's rescan; repair
    // or rescan, and retry.
    // Report the first error: later attempts may only fail because of
    // what the first one left behind.
    //
    // When s6-rc-update fails, set-install has copied the database beside
    // the boot link but not switched the link. With the copy there, running
    // set-install again says "already installed" and exits 0 without
    // updating anything. So: if the live state already runs on the copy
    // (s6-rc-update failed after its switch, starting the new
    // supervisors), run s6-rc-update on it again and switch the boot link
    // here; otherwise remove the copy and run set-install again.
    let copy = fs::read_link(cfg.repo_dir.join("compiled").join(&cfg.set_name))
        .ok()
        .zip(cfg.boot_db().parent().map(Path::to_path_buf))
        .map(|(name, dir)| (name.clone(), dir.join(name)));
    let mut attempt = 0;
    let mut first: Option<String> = None;
    let mut res = install(&[]).map(|_| ());
    loop {
        let e = match res {
            Ok(()) => return Ok(true),
            Err(e) => e,
        };
        // A failed rollback leaves a running service's directory in the
        // new live directory: every retry fails on the rename.
        if attempt >= 3 || e.contains("unable to rollback") {
            return Err(match first {
                Some(f) if f != e => format!("{}\n(last attempt: {})", f.trim(), e.trim()),
                _ => e,
            });
        }
        attempt += 1;
        eprintln!("uinitconverter: installing the set failed, retrying: {}", e.trim());
        let linking = e.contains("manage new service directories");
        first.get_or_insert(e);
        if linking {
            // No rescan here: it would kill the supervisors whose links the
            // failed update removed (see relink_supervised).
            relink_supervised(&cfg.live_dir);
        } else if let Ok(scan) = fs::canonicalize(cfg.live_dir.join("scandir")) {
            let _ = Command::new("s6-svscanctl").arg("-an").arg(scan).status();
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
        let live_db = fs::canonicalize(cfg.live_dir.join("compiled")).ok();
        res = match &copy {
            Some((name, path)) if live_db.is_some() && live_db == fs::canonicalize(path).ok() => {
                run(Command::new("s6-rc-update").arg("-e").arg("-l").arg(&cfg.live_dir).arg(path))
                    .and_then(|_| swap_symlink(name, &cfg.boot_db()))
            }
            Some((_, path)) => {
                if fs::canonicalize(cfg.boot_db()).ok() != fs::canonicalize(path).ok() {
                    let _ = fs::remove_dir_all(path);
                }
                install(&[]).map(|_| ())
            }
            None => install(&[]).map(|_| ()),
        };
    }
}

/// Put back the scan directory links of live service directories that
/// still have a supervisor. "unable to manage new service directories"
/// comes after s6-rc-update's switch to the new live directory: its
/// s6-ftrigrd died (s6 2.15.1.1 crashes when a subscription follows one
/// that already fired), and s6_supervise_link_names() then unlinks every
/// name it had linked so far, those of running services too. The next scan
/// would kill their supervisors, and the services stay down while s6-rc
/// has them up. Directories with no supervisor stay unlinked: the retry's
/// s6-rc-update links them and refuses a link that exists.
fn relink_supervised(live: &Path) {
    let prefix = fs::read_to_string(live.join("prefix")).unwrap_or_default();
    let Ok(dir) = fs::read_dir(live.join("servicedirs")) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let link = live.join("scandir").join(format!("{prefix}{name}"));
        if fs::symlink_metadata(&link).is_ok()
            || !uinitconverter_base::util::lock_held(&e.path().join("supervise/lock"))
        {
            continue;
        }
        // The target s6-rc gives it: LIVE/servicedirs/NAME as -l names LIVE.
        match std::os::unix::fs::symlink(live.join("servicedirs").join(&name), &link) {
            Ok(()) => eprintln!("uinitconverter: relinked {} (its supervisor still runs)", link.display()),
            Err(err) => eprintln!("uinitconverter: relinking {}: {err}", link.display()),
        }
    }
}

/// Plan and write the generated store, sync the repository with it, then
/// commit the set and install it (as the live database too when
/// `update_live` and a live s6-rc state exists).
pub fn regenerate(cfg: &Config, update_live: bool) -> Result<GenResult, String> {
    let plan = plan(cfg);
    let stamp = format!("{}-{}", util::now_realtime_usec(), std::process::id());
    let db_root = cfg.state_dir.join("db");
    let db_dir = db_root.join(&stamp);
    let source = db_dir.join("source");
    write_source(cfg, &plan, &source).map_err(|e| format!("{}: {e}", source.display()))?;
    let source_abs = fs::canonicalize(&source).map_err(|e| e.to_string())?;

    // The repository links the store by STORE_DIR, a symlink to the newest
    // source. If anything below fails, it goes back to the old one.
    let store = cfg.store_dir();
    let old_store = fs::read_link(&store).ok();
    let old_boot = fs::canonicalize(cfg.boot_db()).ok();
    swap_symlink(&source_abs, &store)?;
    let mut set_changes = Vec::new();
    let res = repo_sync(cfg)
        .and_then(|_| sync_essentials(cfg, &mut set_changes))
        .and_then(|_| sync_loggers(cfg, &mut set_changes))
        .and_then(|_| sync_stop_atoms(cfg, &mut set_changes))
        .and_then(|_| commit_and_install(cfg, update_live, &mut set_changes));
    let updated_live = match res {
        Ok(r) => r,
        Err(e) => {
            match &old_store {
                Some(old) => {
                    let _ = swap_symlink(old, &store);
                    let _ = repo_sync(cfg);
                }
                None => {
                    let _ = fs::remove_file(&store);
                }
            }
            let _ = fs::remove_dir_all(&db_dir);
            return Err(e);
        }
    };

    // The database installed before is the one to roll back to
    // (uinitconverter.db=previous on the kernel command line); the one before
    // that goes. After a rollback boot the live database is the previous
    // one: it stays "previous" rather than the database the user fled from.
    let previous = cfg.state_dir.join("compiled.previous");
    let old_previous = fs::canonicalize(&previous).ok();
    let booted_previous = old_previous.is_some()
        && fs::canonicalize(cfg.live_dir.join("compiled")).ok() == old_previous;
    let state_abs = fs::canonicalize(&cfg.state_dir).unwrap_or_else(|_| cfg.state_dir.clone());
    let replaced = old_boot.filter(|o| fs::canonicalize(cfg.boot_db()).ok().as_ref() != Some(o));
    match replaced {
        Some(old) if booted_previous => {
            if old.starts_with(&state_abs) && Some(&old) != old_previous.as_ref() {
                let _ = fs::remove_dir_all(old);
            }
        }
        Some(old) => {
            swap_symlink(&old, &previous)?;
            if let Some(p) = old_previous.filter(|p| *p != old && p.starts_with(&state_abs)) {
                let _ = fs::remove_dir_all(p);
            }
        }
        None => {}
    }

    // "current" (index and report for the daemon) and "previous" (the
    // index of the database to roll back to).
    if let Ok(cur) = fs::read_link(cfg.state_dir.join("current")) {
        if !booted_previous {
            swap_symlink(&cur, &cfg.state_dir.join("previous"))?;
        }
    }
    swap_symlink(&PathBuf::from("db").join(&stamp), &cfg.state_dir.join("current"))?;

    // Garbage-collect generated sources that are neither current nor
    // previous, keeping any that still hold a database in use (databases
    // compiled before the repository existed lived in db/<stamp>/compiled).
    let keep: Vec<PathBuf> = ["current", "previous", "compiled", "compiled.previous"]
        .iter()
        .filter_map(|l| fs::canonicalize(cfg.state_dir.join(l)).ok())
        .chain(fs::canonicalize(cfg.live_dir.join("compiled")).ok())
        .collect();
    if let Ok(rd) = fs::read_dir(&db_root) {
        for e in rd.flatten() {
            let p = fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
            if !keep.iter().any(|k| k.starts_with(&p)) {
                let _ = fs::remove_dir_all(&p);
            }
        }
    }
    // ... and the copies s6-rc-set-install made (STATE_DIR/.SET:...) that
    // are no longer the boot, previous or live database.
    let copy_prefix = format!(".{}:", cfg.set_name);
    if let Ok(rd) = fs::read_dir(&state_abs) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&copy_prefix) && !keep.contains(&e.path()) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }

    Ok(GenResult { db_dir, plan, updated_live, set_changes })
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

    /// After s6-rc-update's failed linking, only directories that still
    /// have a supervisor get their link back.
    #[test]
    fn relink_supervised_only_locked() {
        use std::io::{BufRead, Write};
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-relink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let live = tmp.join("live");
        for n in ["run.service", "new.service", "kept.service"] {
            fs::create_dir_all(live.join("servicedirs").join(n).join("supervise")).unwrap();
            fs::write(live.join("servicedirs").join(n).join("supervise/lock"), "").unwrap();
        }
        fs::create_dir_all(live.join("scandir")).unwrap();
        fs::write(live.join("prefix"), "").unwrap();
        std::os::unix::fs::symlink("elsewhere", live.join("scandir/kept.service")).unwrap();
        // A supervisor's fcntl lock, held by another process.
        let mut child = Command::new("python3")
            .args(["-c", "import fcntl,sys; f=open(sys.argv[1],'w'); fcntl.lockf(f, fcntl.LOCK_EX); print('locked', flush=True); sys.stdin.read()"])
            .arg(live.join("servicedirs/run.service/supervise/lock"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "locked");

        relink_supervised(&live);
        assert_eq!(
            fs::read_link(live.join("scandir/run.service")).unwrap(),
            live.join("servicedirs/run.service")
        );
        assert!(fs::symlink_metadata(live.join("scandir/new.service")).is_err());
        assert_eq!(fs::read_link(live.join("scandir/kept.service")).unwrap(), Path::new("elsewhere"));
        let _ = child.stdin.take().unwrap().write_all(b"");
        let _ = child.wait();
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
