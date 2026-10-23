//! Translate systemd units into an s6-rc source tree, compile it and switch
//! the live database to it.
//!
//! Mapping:
//!   .service (long-running)  -> longrun  (run/finish call `sdcompat-runner run|finish`)
//!   .service Type=oneshot    -> oneshot  (up/down call `sdcompat-runner up|down`)
//!   .target                  -> bundle   (contents = Wants/Requires + .wants/.requires dirs)
//!   enabled .socket          -> its service is pulled in instead (sockets are bound by the runner)
//!   dependencies             -> Requires/Requisite/BindsTo, plus Wants that are also ordered After


use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sdcompat_base::config::Config;
pub use sdcompat_base::index::{unit_arg, Index, Kind};
use sdcompat_base::loader::{self, LoadState, Loader, Unit};
use sdcompat_base::util;

#[derive(Debug, Clone)]
pub struct Planned {
    pub id: String,
    pub kind: Kind,
    /// s6-rc dependencies (atoms) or contents (bundles).
    pub deps: BTreeSet<String>,
    /// Name of the s6-log consumer, when logging is enabled for this unit.
    pub log_atom: Option<String>,
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
            .and_then(|v| sdcompat_base::unit::parse_timespan_usec(&v, 1_000_000))
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
}

/// The s6 service (dist/s6) that serves the fd store.
const FDSTORE_SERVICE: &str = "sdcompat-fdstore";

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
    "IPAddressAllow", "IPAddressDeny",
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
                let t = sdcompat_base::pathspec::trigger_unit(&unit);
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
                let t = sdcompat_base::timerspec::trigger_unit(&unit);
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
        .map(|(id, u)| (id.clone(), sdcompat_base::mountunit::where_of(u)))
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
        // FDSTORE=1 goes to the sdcompat-fdstore service. journald starts
        // before it otherwise (DefaultDependencies=no): "fd store not reachable".
        let fdstore_max: usize =
            unit.get("Service", "FileDescriptorStoreMax").and_then(|v| v.parse().ok()).unwrap_or(0);
        if unit.utype() == "service" && fdstore_max > 0 && plan.extra_names.contains(FDSTORE_SERVICE) {
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
            for (raw, spec) in sdcompat_base::timerspec::calendars(unit) {
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

        plan.units.insert(
            id.clone(),
            Planned {
                id: id.clone(),
                kind,
                deps,
                log_atom,
                warnings,
                timings: Timings::of(unit),
            },
        );
    }

    break_cycles(&mut plan);
    let aliases = plan.aliases.clone();
    resolve_conflicts(&mut plan, &loaded, &aliases);
    plan
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
                if let (Some(log), Some(log_dir)) = (&p.log_atom, &cfg.log_dir) {
                    write_file(&sdir.join("producer-for"), &format!("{log}\n"), false)?;
                    let ldir = dir.join(log);
                    fs::create_dir_all(&ldir)?;
                    write_file(&ldir.join("type"), "longrun\n", false)?;
                    write_file(&ldir.join("consumer-for"), &format!("{}\n", p.id), false)?;
                    write_file(&ldir.join("notification-fd"), "3\n", false)?;
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

    // default.target gets a stable bundle name for the boot bundle to include.
    if let Some(def) = &plan.default_target {
        if plan.units.contains_key(def) {
            let b = dir.join(&cfg.default_bundle);
            fs::create_dir_all(b.join("contents.d"))?;
            write_file(&b.join("type"), "bundle\n", false)?;
            fs::write(b.join("contents.d").join(def), "")?;
        }
    }

    // Booted when /system-update exists (offline updates, dnf system-upgrade).
    let b = dir.join("sdcompat-system-update");
    fs::create_dir_all(b.join("contents.d"))?;
    write_file(&b.join("type"), "bundle\n", false)?;
    // logind too: update tools may ask it to reboot when they are done.
    for u in ["system-update.target", "systemd-logind.service"] {
        if plan.units.contains_key(u) {
            fs::write(b.join("contents.d").join(u), "")?;
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

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
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
}

/// Plan, write, compile, and (if a live s6-rc state exists) switch to the new database.
pub fn regenerate(cfg: &Config, update_live: bool) -> Result<GenResult, String> {
    let plan = plan(cfg);
    let stamp = format!("{}-{}", util::now_realtime_usec(), std::process::id());
    let db_root = cfg.state_dir.join("db");
    let db_dir = db_root.join(&stamp);
    let source = db_dir.join("source");
    write_source(cfg, &plan, &source).map_err(|e| format!("{}: {e}", source.display()))?;

    let compiled = db_dir.join("compiled");
    let mut cmd = Command::new("s6-rc-compile");
    cmd.arg(&compiled).arg(&source);
    for extra in &cfg.extra_source {
        if extra.is_dir() {
            cmd.arg(extra);
        }
    }
    run(&mut cmd)?;

    let live_compiled = cfg.live_dir.join("compiled");
    // The database running now. After a rollback boot (sdcompat.db=previous)
    // it is "previous", not "current", and it has to stay the one to roll
    // back to: taking "current" would make the database the user fled from
    // "previous" and delete the good one below.
    let db_root_abs = fs::canonicalize(&db_root).unwrap_or_else(|_| db_root.clone());
    let running = fs::canonicalize(&live_compiled)
        .ok()
        .and_then(|c| c.parent().map(Path::to_path_buf))
        .filter(|d| d.parent() == Some(db_root_abs.as_path()) && d != &db_dir)
        .and_then(|d| d.file_name().map(|n| PathBuf::from("db").join(n)));
    let mut updated_live = false;
    if update_live && fs::symlink_metadata(&live_compiled).is_ok() {
        let abs = fs::canonicalize(&compiled).map_err(|e| e.to_string())?;
        // s6-rc-update can race s6-svscan's rescan of the service directories
        // ("unable to manage new service directories"); rescan and retry.
        // Report the first error: later attempts may only fail because of
        // what the first one left behind.
        let mut attempt = 0;
        let mut first: Option<String> = None;
        loop {
            match run(Command::new("s6-rc-update").arg("-l").arg(&cfg.live_dir).arg(&abs)) {
                Ok(()) => break,
                // A failed rollback leaves a running service's directory in
                // the new live directory: every retry fails on the rename.
                Err(e) if attempt < 3 && !e.contains("unable to rollback") => {
                    attempt += 1;
                    eprintln!("sdcompat: s6-rc-update failed, retrying: {}", e.trim());
                    first.get_or_insert(e);
                    if let Ok(scan) = fs::canonicalize(cfg.live_dir.join("scandir")) {
                        let _ = Command::new("s6-svscanctl").arg("-an").arg(scan).status();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
                }
                Err(e) => {
                    let e = match first {
                        Some(f) if f != e => format!("{}\n(last attempt: {})", f.trim(), e.trim()),
                        _ => e,
                    };
                    return Err(e);
                }
            }
        }
        updated_live = true;
    }

    // Keep the database we are replacing as "previous" (rollback with
    // sdcompat.db=previous on the kernel command line).
    if let Some(prev) = running.or_else(|| fs::read_link(cfg.state_dir.join("current")).ok()) {
        for (link, target) in [("previous", prev.clone()), ("compiled.previous", prev.join("compiled"))] {
            let tmp = cfg.state_dir.join(format!(".{link}.tmp"));
            let _ = fs::remove_file(&tmp);
            if std::os::unix::fs::symlink(&target, &tmp).is_ok() {
                let _ = fs::rename(&tmp, cfg.state_dir.join(link));
            }
        }
    }
    // Point "current" and "compiled" (used by s6-rc-init at boot) at the new db.
    for (link, target) in [("current", PathBuf::from("db").join(&stamp)), ("compiled", PathBuf::from("db").join(&stamp).join("compiled"))] {
        let tmp = cfg.state_dir.join(format!(".{link}.tmp"));
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&target, &tmp).map_err(|e| e.to_string())?;
        fs::rename(&tmp, cfg.state_dir.join(link)).map_err(|e| e.to_string())?;
    }

    // Garbage-collect databases that are neither current, previous nor live.
    let live_target = fs::canonicalize(&live_compiled).ok();
    let previous = fs::canonicalize(cfg.state_dir.join("previous")).ok();
    if let Ok(rd) = fs::read_dir(&db_root) {
        for e in rd.flatten() {
            let p = e.path();
            if p == db_dir || fs::canonicalize(&p).ok() == previous {
                continue;
            }
            let pc = fs::canonicalize(p.join("compiled")).ok();
            if pc.is_some() && pc == live_target {
                continue;
            }
            let _ = fs::remove_dir_all(&p);
        }
    }

    Ok(GenResult { db_dir, plan, updated_live })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_name_orders_after_dbus() {
        let tmp = std::env::temp_dir().join(format!("sdcompat-gen-{}", std::process::id()));
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
}
