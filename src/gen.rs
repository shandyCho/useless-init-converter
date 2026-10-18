//! Translate systemd units into an s6-rc source tree, compile it and switch
//! the live database to it.
//!
//! Mapping:
//!   .service (long-running)  -> longrun  (run/finish call `sdcompat run|finish`)
//!   .service Type=oneshot    -> oneshot  (up/down call `sdcompat up|down`)
//!   .target                  -> bundle   (contents = Wants/Requires + .wants/.requires dirs)
//!   enabled .socket          -> its service is pulled in instead (sockets are bound by the runner)
//!   dependencies             -> Requires/Requisite/BindsTo, plus Wants that are also ordered After

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::loader::{self, LoadState, Loader, Unit};
use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Longrun,
    Oneshot,
    Bundle,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Longrun => "longrun",
            Kind::Oneshot => "oneshot",
            Kind::Bundle => "bundle",
        }
    }
    fn parse(s: &str) -> Option<Kind> {
        match s {
            "longrun" => Some(Kind::Longrun),
            "oneshot" => Some(Kind::Oneshot),
            "bundle" => Some(Kind::Bundle),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Planned {
    pub id: String,
    pub kind: Kind,
    /// s6-rc dependencies (atoms) or contents (bundles).
    pub deps: BTreeSet<String>,
    /// Name of the s6-log consumer, when logging is enabled for this unit.
    pub log_atom: Option<String>,
    pub warnings: Vec<String>,
    pub timeout_up_ms: u64,
    pub timeout_down_ms: u64,
    pub timeout_finish_ms: u64,
    pub down_signal: Option<String>,
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
}

const UNIT_KEYS: &[&str] = &[
    "Description", "Documentation", "Requires", "Requisite", "Wants", "BindsTo", "PartOf",
    "Upholds", "Before", "After", "DefaultDependencies", "RefuseManualStart",
    "RefuseManualStop", "AllowIsolate", "IgnoreOnIsolate", "StopWhenUnneeded",
    "StartLimitIntervalSec", "StartLimitBurst", "SourcePath", "Conflicts",
];

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
    "RestartMode", "LimitCPU", "LimitFSIZE", "LimitDATA", "LimitSTACK", "LimitCORE", "LimitRSS",
    "LimitNOFILE", "LimitAS", "LimitNPROC", "LimitMEMLOCK", "LimitLOCKS", "LimitSIGPENDING",
    "LimitMSGQUEUE", "LimitNICE", "LimitRTPRIO", "LimitRTTIME", "SendSIGHUP", "TimeoutAbortSec",
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
            (ty == "service" || ty == "target") && !n.contains("@.")
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
        if plan.extra_names.contains(&unit.id) {
            plan.skipped.insert(name, "provided by extra s6-rc source".into());
            continue;
        }

        match unit.utype() {
            "service" | "target" => {}
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
        if unit.utype() == "service" && unit.get_bool("Unit", "DefaultDependencies", true) {
            requires.push("sysinit.target".into());
            after.push("basic.target".into());
        }
        // Type=dbus implies Requires=/After=dbus.socket.
        if unit.utype() == "service" && (unit.get("Service", "Type").as_deref() == Some("dbus")
            || (unit.get("Service", "Type").is_none() && unit.get("Service", "BusName").is_some()))
        {
            requires.push("dbus.socket".into());
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
        deps.remove(id);

        let kind = if is_target {
            if *id == "sysinit.target" && plan.extra_names.contains(&cfg.sysinit_bundle) {
                deps.insert(cfg.sysinit_bundle.clone());
            }
            Kind::Bundle
        } else if longrun_type(unit) {
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
            for sec in unit.file.sections() {
                if !["Unit", "Service", "Install"].contains(&sec) && !sec.starts_with("X-") {
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

        let timeout_start = unit
            .get("Service", "TimeoutStartSec")
            .or_else(|| unit.get("Service", "TimeoutSec"))
            .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(90_000_000);
        let timeout_stop = unit
            .get("Service", "TimeoutStopSec")
            .or_else(|| unit.get("Service", "TimeoutSec"))
            .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(90_000_000);
        let restart_sec = unit
            .get("Service", "RestartSec")
            .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(100_000);
        let down_signal = unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .filter(|&s| s != libc::SIGTERM)
            .map(|s| format!("SIG{}", util::signal_name(s)))
            .filter(|s| !s[3..].chars().all(|c| c.is_ascii_digit()));

        let log_atom = (kind == Kind::Longrun && cfg.log_dir.is_some() && !uses_tty_or_file(unit))
            .then(|| format!("{id}.log"));

        plan.units.insert(
            id.clone(),
            Planned {
                id: id.clone(),
                kind,
                deps,
                log_atom,
                warnings,
                timeout_up_ms: ms(timeout_start),
                timeout_down_ms: ms(timeout_stop),
                timeout_finish_ms: ms(restart_sec).saturating_add(5000),
                down_signal,
            },
        );
    }

    break_cycles(&mut plan);
    plan
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

/// Argument form of a unit name that is safe for both sh and execline.
fn unit_arg(name: &str) -> String {
    if name.bytes().all(|b| b.is_ascii_alphanumeric() || b"@:._-".contains(&b)) {
        format!("-- {name}")
    } else {
        let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
        format!("--unit-hex {hex}")
    }
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
                write_file(&sdir.join("timeout-up"), &format!("{}\n", p.timeout_up_ms), false)?;
                write_file(&sdir.join("timeout-down"), &format!("{}\n", p.timeout_down_ms), false)?;
            }
            Kind::Longrun => {
                let arg = unit_arg(&p.id);
                write_file(
                    &sdir.join("run"),
                    &format!("#!/bin/sh\nexec {runner} run{args} {arg}\n"),
                    true,
                )?;
                write_file(
                    &sdir.join("finish"),
                    &format!("#!/bin/sh\nexec {runner} finish{args} {arg} \"$@\"\n"),
                    true,
                )?;
                write_file(&sdir.join("notification-fd"), "3\n", false)?;
                write_file(&sdir.join("timeout-up"), &format!("{}\n", p.timeout_up_ms), false)?;
                write_file(&sdir.join("timeout-down"), &format!("{}\n", p.timeout_down_ms), false)?;
                write_file(&sdir.join("timeout-kill"), &format!("{}\n", p.timeout_down_ms), false)?;
                write_file(
                    &sdir.join("timeout-finish"),
                    &format!("{}\n", p.timeout_finish_ms),
                    false,
                )?;
                if let Some(sig) = &p.down_signal {
                    write_file(&sdir.join("down-signal"), &format!("{sig}\n"), false)?;
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

/// The index written next to a generated source tree.
#[derive(Debug, Default, Clone)]
pub struct Index {
    /// unit id -> (kind, log atom)
    pub units: BTreeMap<String, (Kind, Option<String>)>,
    pub aliases: BTreeMap<String, String>,
    pub default_target: Option<String>,
}

impl Index {
    pub fn load(cfg: &Config) -> Index {
        let mut idx = Index::default();
        let Ok(text) = fs::read_to_string(cfg.state_dir.join("current").join("index")) else {
            return idx;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["unit", id, kind, log] => {
                    if let Some(k) = Kind::parse(kind) {
                        let log = (*log != "-").then(|| log.to_string());
                        idx.units.insert(id.to_string(), (k, log));
                    }
                }
                ["alias", a, id] => {
                    idx.aliases.insert(a.to_string(), id.to_string());
                }
                ["default", d] => idx.default_target = Some(d.to_string()),
                _ => {}
            }
        }
        idx
    }

    pub fn resolve<'a>(&'a self, name: &'a str) -> &'a str {
        self.aliases.get(name).map(String::as_str).unwrap_or(name)
    }
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
    let mut updated_live = false;
    if update_live && fs::symlink_metadata(&live_compiled).is_ok() {
        let abs = fs::canonicalize(&compiled).map_err(|e| e.to_string())?;
        // s6-rc-update can race s6-svscan's rescan of the service directories
        // ("unable to manage new service directories"); rescan and retry.
        let mut attempt = 0;
        loop {
            match run(Command::new("s6-rc-update").arg("-l").arg(&cfg.live_dir).arg(&abs)) {
                Ok(()) => break,
                Err(e) if attempt < 3 => {
                    attempt += 1;
                    eprintln!("sdcompat: s6-rc-update failed, retrying: {}", e.trim());
                    if let Ok(scan) = fs::canonicalize(cfg.live_dir.join("scandir")) {
                        let _ = Command::new("s6-svscanctl").arg("-an").arg(scan).status();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
                }
                Err(e) => return Err(e),
            }
        }
        updated_live = true;
    }

    // Point "current" and "compiled" (used by s6-rc-init at boot) at the new db.
    for (link, target) in [("current", PathBuf::from("db").join(&stamp)), ("compiled", PathBuf::from("db").join(&stamp).join("compiled"))] {
        let tmp = cfg.state_dir.join(format!(".{link}.tmp"));
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&target, &tmp).map_err(|e| e.to_string())?;
        fs::rename(&tmp, cfg.state_dir.join(link)).map_err(|e| e.to_string())?;
    }

    // Garbage-collect databases that are neither current nor live.
    let live_target = fs::canonicalize(&live_compiled).ok();
    if let Ok(rd) = fs::read_dir(&db_root) {
        for e in rd.flatten() {
            let p = e.path();
            if p == db_dir {
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
