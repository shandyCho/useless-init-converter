//! Shared configuration, read from /etc/uinitconverter/uinitconverter.conf (KEY=VALUE).
//!
//! The generator embeds `--config PATH` into generated run scripts when a
//! non-default config is used, so the runner always sees the same settings.

use std::path::{Path, PathBuf};

use crate::envfile;

pub const DEFAULT_CONFIG: &str = "/etc/uinitconverter/uinitconverter.conf";

/// LOG_DIR's default. JOURNAL=yes clears Config::log_dir, so a tool that
/// prepares the directory for a later JOURNAL=no falls back to this.
pub const DEFAULT_LOG_DIR: &str = "/var/log/uinitconverter";

/// Where an uinitconverter tool is: next to the running program when it is there
/// (a build tree, or /usr/libexec/uinitconverter itself), else /usr/libexec/uinitconverter.
pub fn tool_path(name: &str) -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let p = dir.join(name);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("/usr/libexec/uinitconverter").join(name)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub config_path: PathBuf,
    /// The init system the units are translated for. Only "s6" exists yet;
    /// the key is there so a configuration names its backend.
    pub init: String,
    /// Unit search path, highest priority first. The first entry is where
    /// enable/disable/mask write their symlinks.
    pub unit_path: Vec<PathBuf>,
    /// Directory for runtime (`--runtime`) enablement symlinks.
    pub runtime_unit_dir: PathBuf,
    /// Preset directories, highest priority first.
    pub preset_path: Vec<PathBuf>,
    /// Hand-written s6-rc service stores, used beside the generated one.
    pub extra_source: Vec<PathBuf>,
    /// Generated stores, the index and the boot databases live here.
    pub state_dir: PathBuf,
    /// The s6-rc repository (s6-rc-repo-init), shared with the `s6` command.
    pub repo_dir: PathBuf,
    /// The generated store as the repository sees it: a symlink to the
    /// newest generated source (default STATE_DIR/store).
    pub store_dir: Option<PathBuf>,
    /// The repository's service set that is committed and installed.
    pub set_name: String,
    /// s6-rc live directory.
    pub live_dir: PathBuf,
    /// Per-unit runtime state (notify sockets, results, status text).
    pub run_dir: PathBuf,
    /// When set, every longrun gets an s6-log consumer writing to LOG_DIR/<unit>.
    pub log_dir: Option<PathBuf>,
    /// Unit name globs that are never translated.
    pub exclude: Vec<String>,
    /// Globs that are translated even if they match EXCLUDE.
    pub include: Vec<String>,
    /// systemd unit name -> s6-rc service from EXTRA_SOURCE that replaces it
    /// as a dependency (e.g. dbus.service -> dbus).
    pub dep_map: Vec<(String, String)>,
    /// Extra s6-rc service/bundle added to sysinit.target (udev, mounts, ...).
    pub sysinit_bundle: String,
    /// The bundle s6-rc-set-commit makes of the set's active and always
    /// services: what boots (s6-linux-init's runlevel).
    pub default_bundle: String,
    /// s6 service running initconvertd; timers depend on it.
    pub daemon_service: String,
    /// Created once the daemon is ready, so sd_booted() returns true.
    pub booted_flag: Option<PathBuf>,
    /// Peer-to-peer socket used by systemctl when run as root.
    pub private_socket: Option<PathBuf>,
    /// D-Bus address clients (timers) use when the private socket is unavailable.
    pub bus_address: Option<String>,
    /// Path of the uinitconverter binary as seen from generated scripts.
    pub runner: PathBuf,
    /// initconvert-timers: what generated .timer and .path services run.
    pub timer_runner: PathBuf,
    /// Shutdown commands used for PowerOff/Reboot/Halt.
    pub poweroff_cmd: String,
    pub reboot_cmd: String,
    pub halt_cmd: String,
    /// Users (besides root) allowed to call mutating D-Bus methods.
    pub allow_uids: Vec<u32>,
    /// Run systemd-journald and send service output to it (journalctl works).
    /// When false, each service gets an s6-log directory under LOG_DIR.
    pub journal: bool,
    /// The syslog daemon of the s6 "syslog" service when JOURNAL=no
    /// (SYSLOG=, one of SYSLOGS). Read by that service's run script; here
    /// it is only checked.
    pub syslog: String,
    /// When false, systemd-resolved is not translated (RESOLVED=no): the
    /// s6 "resolv" oneshot makes NetworkManager handle DNS on that boot.
    pub resolved: bool,
    /// journald's stdout stream socket.
    pub journal_stdout: PathBuf,
    /// Where StartTransientUnit writes unit files (systemd-run).
    pub transient_dir: PathBuf,
    /// udev's runtime directory: units in SYSTEMD_WANTS of devices in its
    /// database are started (None: device wants are ignored).
    pub udev_dir: Option<PathBuf>,
    /// Also follow udevd's netlink events (devices added after startup).
    pub udev_monitor: bool,
}

/// The values INIT= accepts.
pub const INITS: &[&str] = &["s6"];

/// JOURNAL='s default: no systemd-journald; rsyslog (or syslog-ng) and
/// per-service s6-log directories (rule 2, work_history part 220).
pub const JOURNAL_DEFAULT: bool = false;
/// RESOLVED='s default: no systemd-resolved; NetworkManager handles DNS
/// (rule 2, work_history part 220).
pub const RESOLVED_DEFAULT: bool = false;

/// Units taken out of INCLUDE when RESOLVED=no.
const RESOLVED_UNITS: &[&str] = &[
    "systemd-resolved.service",
    "systemd-resolved-varlink.socket",
    "systemd-resolved-monitor.socket",
];

/// Units added to INCLUDE when JOURNAL=yes.
const JOURNAL_UNITS: &[&str] = &[
    "systemd-journald.service",
    "systemd-journald.socket",
    "systemd-journald-dev-log.socket",
    "systemd-journal-flush.service",
];

/// The values SYSLOG= accepts.
pub const SYSLOGS: &[&str] = &["rsyslog", "syslog-ng"];

/// Every key uinitconverter.conf may set: the contract between the parser
/// (Config::load), dist/uinitconverter.conf (documents them) and
/// `initconvert init` (writes some). The tests check each side against
/// these lists.
pub const KEYS: &[&str] = &[
    "INIT",
    "TOOLS",
    "UNIT_PATH",
    "RUNTIME_UNIT_DIR",
    "PRESET_PATH",
    "EXTRA_SOURCE",
    "STATE_DIR",
    "REPO_DIR",
    "STORE_DIR",
    "SET",
    "LIVE_DIR",
    "RUN_DIR",
    "LOG_DIR",
    "EXCLUDE",
    "USER_MANAGER",
    "USER_EXCLUDE",
    "USER_FALLBACK_ON",
    "EXCLUDE_ADD",
    "INCLUDE",
    "DEP_MAP",
    "DEP_MAP_ADD",
    "SYSINIT_BUNDLE",
    "DEFAULT_BUNDLE",
    "DAEMON_SERVICE",
    "BOOTED_FLAG",
    "PRIVATE_SOCKET",
    "BUS_ADDRESS",
    "RUNNER",
    "TIMER_RUNNER",
    "POWEROFF_CMD",
    "REBOOT_CMD",
    "HALT_CMD",
    "JOURNAL",
    "SYSLOG",
    "AUTO_MASK",
    "RESOLVED",
    "BUS",
    "TRANSIENT_DIR",
    "JOURNAL_STDOUT",
    "UDEV_DIR",
    "UDEV_MONITOR",
    "ALLOW_UIDS",
];
/// Removed keys still accepted (ignored), so an older file still loads.
/// USER_*: the s6 user manager, removed in work_history part 205
/// (`initconvert-user-setup on` wrote USER_MANAGER=s6).
pub const REMOVED_KEYS: &[&str] = &[
    "AUTO_MASK",
    "USER_MANAGER",
    "USER_EXCLUDE",
    "USER_FALLBACK_ON",
];

/// uinitconverter's own D-Bus methods (StopInstances, ...), on the manager's
/// object path (not systemd's interface).
pub const UINITCONVERTER_IFACE: &str = "org.uinitconverter.Manager";

/// initconvert-runner's commands: the contract between the runner (parses
/// them into its Cmd), initconvert-gen (writes them into the service
/// scripts), initconvertd (runs them) and dist/s6's run scripts (confine).
/// Write these constants, not literals; tests on each side check the rest.
pub mod runner_cmd {
    /// A longrun's `run` script.
    pub const RUN: &str = "run";
    /// A longrun's `finish` script.
    pub const FINISH: &str = "finish";
    /// A oneshot's (mount's, swap's) `up` script.
    pub const UP: &str = "up";
    /// A oneshot's `down` script.
    pub const DOWN: &str = "down";
    /// ExecStop= with $MAINPID, run by initconvertd.
    pub const STOP_EXEC: &str = "stop-exec";
    /// The `down` of UNIT.stop: ExecStop= when s6-rc stops a longrun.
    pub const STOP_ATOM: &str = "stop-atom";
    /// ExecReload= with $MAINPID, run by initconvertd.
    pub const RELOAD_EXEC: &str = "reload-exec";
    /// Run a command with a unit's confinement (dist/s6's run scripts).
    pub const CONFINE: &str = "confine";
    /// An Accept=yes socket's `run` script.
    pub const ACCEPT: &str = "accept";
    /// An Accept=yes socket's `finish` script.
    pub const ACCEPT_FINISH: &str = "accept-finish";
    /// One connection's instance, run by s6-ipcserverd or s6-tcpserverd.
    pub const CONNECTION: &str = "connection";
    pub const ALL: &[&str] = &[
        RUN,
        FINISH,
        UP,
        DOWN,
        STOP_EXEC,
        STOP_ATOM,
        RELOAD_EXEC,
        CONFINE,
        ACCEPT,
        ACCEPT_FINISH,
        CONNECTION,
    ];
}

/// initconvert-timers' commands, written by initconvert-gen into a .timer's
/// or .path unit's `run` script.
pub mod timers_cmd {
    pub const TIMER_RUN: &str = "timer-run";
    pub const PATH_RUN: &str = "path-run";
    pub const ALL: &[&str] = &[TIMER_RUN, PATH_RUN];
}

/// Units added to EXCLUDE and mapped to the s6 "syslog" service when
/// JOURNAL=no: the distribution's syslog daemons, whichever SYSLOG= picks.
const SYSLOG_UNITS: &[&str] = &["rsyslog.service", "syslog-ng.service", "syslog.service"];

impl Config {
    pub fn defaults() -> Config {
        let runner = tool_path("initconvert-runner");
        let timer_runner = tool_path("initconvert-timers");
        Config {
            config_path: PathBuf::from(DEFAULT_CONFIG),
            init: "s6".into(),
            unit_path: [
                "/etc/systemd/system",
                "/run/systemd/transient",
                "/run/systemd/system",
                "/usr/local/lib/systemd/system",
                "/usr/lib/systemd/system",
            ]
            .iter()
            .map(PathBuf::from)
            .collect(),
            runtime_unit_dir: PathBuf::from("/run/systemd/system"),
            preset_path: [
                "/etc/systemd/system-preset",
                "/run/systemd/system-preset",
                "/usr/local/lib/systemd/system-preset",
                "/usr/lib/systemd/system-preset",
            ]
            .iter()
            .map(PathBuf::from)
            .collect(),
            extra_source: vec![
                PathBuf::from("/etc/uinitconverter/s6"),
                PathBuf::from("/etc/s6/sources"),
            ],
            state_dir: PathBuf::from("/var/lib/uinitconverter"),
            repo_dir: PathBuf::from("/var/lib/s6-rc/repository"),
            store_dir: None,
            set_name: "current".into(),
            live_dir: PathBuf::from("/run/s6-rc"),
            run_dir: PathBuf::from("/run/uinitconverter"),
            log_dir: Some(PathBuf::from(DEFAULT_LOG_DIR)),
            exclude: [
                "systemd-*",
                "initrd-*",
                "emergency.service",
                "rescue.service",
                "dbus-broker.service",
                "dbus.service",
                "plymouth-*",
                "kmod-static-nodes.service",
                "ldconfig.service",
                "uinitconverter*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            // systemd's own services that run fine under uinitconverter. logind and
            // the user manager (user@.service) make GNOME and other desktops work.
            include: [
                "systemd-logind.service",
                "systemd-hostnamed.service",
                "systemd-localed.service",
                "systemd-timedated.service",
                "systemd-suspend.service",
                "systemd-hibernate.service",
                "systemd-hybrid-sleep.service",
                "systemd-suspend-then-hibernate.service",
                "systemd-logind-varlink.socket",
                "systemd-hostnamed.socket",
                // Fedora's /etc/resolv.conf points at resolved's 127.0.0.53.
                "systemd-resolved.service",
                "systemd-resolved-varlink.socket",
                "systemd-resolved-monitor.socket",
                // systemd-vconsole-setup, systemd-backlight@ and
                // systemd-user-sessions are replaced by the s6 oneshots
                // vconsole (kbd) and backlight and the tmpfiles step
                // (rule 2, work_history part 220).
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            dep_map: [
                ("dbus.service", "dbus"),
                ("dbus.socket", "dbus"),
                ("dbus-broker.service", "dbus"),
                ("messagebus.service", "dbus"),
                ("systemd-udevd.service", "udevd"),
                ("systemd-udev-trigger.service", "udev-trigger"),
                ("systemd-udev-settle.service", "udev-trigger"),
                ("systemd-journald.socket", "syslog"),
                ("syslog.socket", "syslog"),
                ("systemd-tmpfiles-setup.service", "tmpfiles-setup"),
                ("systemd-sysusers.service", "sysusers"),
                ("systemd-remount-fs.service", "mount-fs"),
                ("systemd-user-sessions.service", "tmpfiles-setup"),
                ("systemd-vconsole-setup.service", "vconsole"),
            ]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
            sysinit_bundle: "uinitconverter-sysinit".into(),
            default_bundle: "default".into(),
            daemon_service: "uinitconverterd".into(),
            booted_flag: Some(PathBuf::from("/run/systemd/system")),
            private_socket: Some(PathBuf::from("/run/systemd/private")),
            bus_address: None,
            runner,
            timer_runner,
            poweroff_cmd: "s6-linux-init-shutdown -p now".into(),
            reboot_cmd: "s6-linux-init-shutdown -r now".into(),
            halt_cmd: "s6-linux-init-shutdown -h now".into(),
            allow_uids: Vec::new(),
            journal: JOURNAL_DEFAULT,
            syslog: "rsyslog".into(),
            resolved: RESOLVED_DEFAULT,
            journal_stdout: PathBuf::from("/run/systemd/journal/stdout"),
            transient_dir: PathBuf::from("/run/systemd/transient"),
            udev_dir: Some(PathBuf::from("/run/udev")),
            udev_monitor: true,
        }
    }

    /// Load defaults, then the config file (if present).
    pub fn load(path: Option<&Path>) -> Result<Config, String> {
        let mut cfg = Config::defaults();
        let explicit = path.is_some() || std::env::var_os("UINITCONVERTER_CONFIG").is_some();
        let path = path
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("UINITCONVERTER_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
        cfg.config_path = path.clone();

        let vars = match envfile::load(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
                return Ok(cfg.finish())
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        for (k, v) in vars {
            let paths = |v: &str| -> Vec<PathBuf> {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            };
            let opt = |v: &str| (!v.is_empty()).then(|| PathBuf::from(v));
            match k.as_str() {
                "INIT" => cfg.init = v,
                "TOOLS" => {
                    crate::tools::configure(&v).map_err(|e| format!("{}: {e}", path.display()))?
                }
                "UNIT_PATH" => cfg.unit_path = paths(&v),
                "RUNTIME_UNIT_DIR" => cfg.runtime_unit_dir = PathBuf::from(&v),
                "PRESET_PATH" => cfg.preset_path = paths(&v),
                "EXTRA_SOURCE" => cfg.extra_source = paths(&v),
                "STATE_DIR" => cfg.state_dir = PathBuf::from(&v),
                "REPO_DIR" => cfg.repo_dir = PathBuf::from(&v),
                "STORE_DIR" => cfg.store_dir = opt(&v),
                "SET" => cfg.set_name = v,
                "LIVE_DIR" => cfg.live_dir = PathBuf::from(&v),
                "RUN_DIR" => cfg.run_dir = PathBuf::from(&v),
                "LOG_DIR" => cfg.log_dir = opt(&v),
                "EXCLUDE" => cfg.exclude = v.split_whitespace().map(String::from).collect(),
                // Removed with the s6 user manager: user@ runs systemd --user.
                "USER_MANAGER" | "USER_EXCLUDE" | "USER_FALLBACK_ON" => {}
                "EXCLUDE_ADD" => cfg.exclude.extend(v.split_whitespace().map(String::from)),
                "INCLUDE" => cfg.include = v.split_whitespace().map(String::from).collect(),
                "DEP_MAP" | "DEP_MAP_ADD" => {
                    if k == "DEP_MAP" {
                        cfg.dep_map.clear();
                    }
                    for pair in v.split_whitespace() {
                        if let Some((a, b)) = pair.split_once('=') {
                            cfg.dep_map.retain(|(x, _)| x != a);
                            cfg.dep_map.push((a.to_string(), b.to_string()));
                        }
                    }
                }
                "SYSINIT_BUNDLE" => cfg.sysinit_bundle = v,
                "DEFAULT_BUNDLE" => cfg.default_bundle = v,
                "DAEMON_SERVICE" => cfg.daemon_service = v,
                "BOOTED_FLAG" => cfg.booted_flag = opt(&v),
                "PRIVATE_SOCKET" => cfg.private_socket = opt(&v),
                "BUS_ADDRESS" => cfg.bus_address = (!v.is_empty()).then(|| v.clone()),
                "RUNNER" => cfg.runner = PathBuf::from(&v),
                "TIMER_RUNNER" => cfg.timer_runner = PathBuf::from(&v),
                "POWEROFF_CMD" => cfg.poweroff_cmd = v,
                "REBOOT_CMD" => cfg.reboot_cmd = v,
                "HALT_CMD" => cfg.halt_cmd = v,
                "JOURNAL" => cfg.journal = crate::unit::parse_bool(&v).unwrap_or(JOURNAL_DEFAULT),
                "SYSLOG" => cfg.syslog = v,
                // Removed (see automask.rs); a reload undoes what it masked.
                "AUTO_MASK" => {}
                "RESOLVED" => {
                    cfg.resolved = crate::unit::parse_bool(&v).unwrap_or(RESOLVED_DEFAULT)
                }
                // The system bus: read by the dbus s6 service's run script.
                "BUS" => {}
                "TRANSIENT_DIR" => cfg.transient_dir = PathBuf::from(&v),
                "JOURNAL_STDOUT" => cfg.journal_stdout = PathBuf::from(&v),
                "UDEV_DIR" => cfg.udev_dir = opt(&v),
                "UDEV_MONITOR" => cfg.udev_monitor = crate::unit::parse_bool(&v).unwrap_or(true),
                "ALLOW_UIDS" => {
                    cfg.allow_uids = v
                        .split_whitespace()
                        .filter_map(|s| s.parse().ok())
                        .collect()
                }
                other => {
                    eprintln!("uinitconverter: {}: unknown key {other}", path.display());
                    debug_assert!(!KEYS.contains(&other), "{other} is in KEYS but not parsed");
                }
            }
        }
        if !INITS.contains(&cfg.init.as_str()) {
            return Err(format!(
                "{}: INIT={}: supported: {}",
                path.display(),
                cfg.init,
                INITS.join(" ")
            ));
        }
        if !SYSLOGS.contains(&cfg.syslog.as_str()) {
            return Err(format!(
                "{}: SYSLOG={}: supported: {}",
                path.display(),
                cfg.syslog,
                SYSLOGS.join(" ")
            ));
        }
        Ok(cfg.finish())
    }

    fn finish(mut self) -> Config {
        if self.journal {
            for u in JOURNAL_UNITS {
                if !self.include.iter().any(|i| i == u) {
                    self.include.push(u.to_string());
                }
            }
            // journalctl replaces the per-service s6-log directories.
            self.log_dir = None;
            // The s6 "syslog" daemon has no config with journald; its
            // sockets belong to journald's own translated units.
            self.dep_map.retain(|(a, b)| {
                !(b == "syslog" && (a == "systemd-journald.socket" || a == "syslog.socket"))
            });
        } else {
            // The s6 "syslog" service is the syslog daemon (SYSLOG=); the
            // distribution's rsyslog.service or syslog-ng.service would start
            // a second one (reading a journal that does not exist) and write
            // the same files.
            // INCLUDEd (any name: syslog.service is their Alias=):
            // the owner runs the distribution's unit, so dependencies on it
            // go to it, not to the s6 service.
            let included = SYSLOG_UNITS
                .iter()
                .any(|u| self.include.iter().any(|g| crate::util::fnmatch(g, u)));
            for u in SYSLOG_UNITS.iter().filter(|_| !included) {
                if !self.exclude.iter().any(|e| e == u) {
                    self.exclude.push(u.to_string());
                }
                if self.mapped_dep(u).is_none() {
                    self.dep_map.push((u.to_string(), "syslog".to_string()));
                }
            }
        }
        if !self.resolved {
            self.include
                .retain(|u| !RESOLVED_UNITS.contains(&u.as_str()));
        }
        if !self.unit_path.contains(&self.transient_dir) {
            self.unit_path
                .insert(1.min(self.unit_path.len()), self.transient_dir.clone());
        }
        self
    }

    /// Writable unit directory for enable/disable/mask.
    pub fn config_unit_dir(&self) -> &Path {
        &self.unit_path[0]
    }

    pub fn store_dir(&self) -> PathBuf {
        self.store_dir
            .clone()
            .unwrap_or_else(|| self.state_dir.join("store"))
    }

    /// The boot database: a symlink s6-rc-set-install replaces, which
    /// rc.init gives to s6-rc-init (s6.conf's bootdb).
    pub fn boot_db(&self) -> PathBuf {
        self.state_dir.join("compiled")
    }

    /// RUN_DIR/units/<unit>, as state::for_unit names it: a name that is
    /// not a single path component is escaped, so "../x.scope" stays inside.
    pub fn unit_run_dir(&self, unit: &str) -> PathBuf {
        self.run_dir
            .join("units")
            .join(&*crate::util::path_component(unit))
    }

    /// Arguments generated scripts pass to the runner so it loads this config.
    /// initconvert-gen, which initconvertd runs for a reload: TOOLS='s
    /// `initconvert-gen=`, else beside the runner.
    pub fn gen_tool(&self) -> PathBuf {
        const NAME: &str = "initconvert-gen";
        let mapped = crate::tools::path(NAME);
        if mapped != NAME {
            return PathBuf::from(mapped);
        }
        self.runner
            .parent()
            .unwrap_or(Path::new("/usr/libexec/uinitconverter"))
            .join(NAME)
    }

    pub fn runner_args(&self) -> String {
        if self.config_path == Path::new(DEFAULT_CONFIG) {
            String::new()
        } else {
            format!(" --config {}", self.config_path.display())
        }
    }

    pub fn is_excluded(&self, unit: &str) -> bool {
        self.exclude.iter().any(|g| crate::util::fnmatch(g, unit))
            && !self.include.iter().any(|g| crate::util::fnmatch(g, unit))
    }

    pub fn mapped_dep(&self, unit: &str) -> Option<&str> {
        self.dep_map
            .iter()
            .find(|(a, _)| a == unit)
            .map(|(_, b)| b.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The s6 user manager's keys are gone but still load (ignored): an
    /// older file with USER_MANAGER=s6 must not stop the boot.
    #[test]
    fn removed_user_manager_keys_still_load() {
        let dir = std::env::temp_dir().join(format!("uinitconverter-um-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.conf");
        std::fs::write(
            &f,
            "USER_MANAGER=s6\nUSER_EXCLUDE=\"a.service\"\nUSER_FALLBACK_ON=x.target\n",
        )
        .unwrap();
        assert!(Config::load(Some(&f)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_and_tools_keys() {
        let _l = crate::tools::TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("uinitconverter-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.conf");
        std::fs::write(&f, "INIT=s6\nTOOLS=\"s6-svc=/x/s6-svc swapon=/y/swapon\"\n").unwrap();
        let cfg = Config::load(Some(&f)).unwrap();
        assert_eq!(cfg.init, "s6");
        assert_eq!(crate::tools::path("swapon"), "/y/swapon");
        crate::tools::configure("").unwrap();
        std::fs::write(&f, "INIT=dinit\n").unwrap();
        assert!(Config::load(Some(&f)).unwrap_err().contains("INIT=dinit"));
        std::fs::write(&f, "TOOLS=s6-svc\n").unwrap();
        assert!(Config::load(Some(&f)).is_err());
        std::fs::write(&f, "SYSLOG=syslog-ng\n").unwrap();
        assert_eq!(Config::load(Some(&f)).unwrap().syslog, "syslog-ng");
        std::fs::write(&f, "SYSLOG=metalog\n").unwrap();
        assert!(Config::load(Some(&f))
            .unwrap_err()
            .contains("SYSLOG=metalog"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_drops_syslog_map() {
        let mut cfg = Config::defaults();
        cfg.journal = true;
        let cfg = cfg.finish();
        assert_eq!(cfg.mapped_dep("systemd-journald.socket"), None);
        assert_eq!(cfg.mapped_dep("syslog.socket"), None);
        let mut cfg = Config::defaults();
        cfg.journal = false;
        let cfg = cfg.finish();
        assert_eq!(cfg.mapped_dep("systemd-journald.socket"), Some("syslog"));
        assert_eq!(cfg.mapped_dep("syslog.socket"), Some("syslog"));
    }

    #[test]
    fn journal_no_excludes_rsyslog() {
        let mut cfg = Config::defaults();
        cfg.journal = true;
        let cfg = cfg.finish();
        assert!(!cfg.is_excluded("rsyslog.service"));
        assert_eq!(cfg.mapped_dep("rsyslog.service"), None);
        let mut cfg = Config::defaults();
        cfg.journal = false;
        let cfg = cfg.finish();
        assert!(cfg.is_excluded("rsyslog.service"));
        assert_eq!(cfg.mapped_dep("rsyslog.service"), Some("syslog"));
        assert_eq!(cfg.mapped_dep("syslog.service"), Some("syslog"));
        assert!(cfg.is_excluded("syslog-ng.service"));
        assert_eq!(cfg.mapped_dep("syslog-ng.service"), Some("syslog"));
        let mut cfg = Config::defaults();
        cfg.journal = false;
        cfg.include.push("rsyslog.service".into());
        let cfg = cfg.finish();
        assert!(!cfg.is_excluded("rsyslog.service"));
        assert_eq!(cfg.mapped_dep("rsyslog.service"), None);
        assert_eq!(cfg.mapped_dep("syslog.service"), None);
    }

    #[test]
    fn resolved_no_excludes_its_units() {
        let mut cfg = Config::defaults();
        cfg.resolved = true;
        assert!(!cfg.finish().is_excluded("systemd-resolved.service"));
        // The defaults (rule 2, part 220): neither journald nor resolved.
        let d = Config::defaults();
        assert!(!d.journal && !d.resolved);
        let mut cfg = Config::defaults();
        cfg.resolved = false;
        let cfg = cfg.finish();
        for u in RESOLVED_UNITS {
            assert!(cfg.is_excluded(u), "{u}");
        }
        assert!(!cfg.is_excluded("systemd-logind.service"));
    }
}

#[cfg(test)]
mod contract_tests {
    use super::{runner_cmd, timers_cmd};
    use std::path::Path;

    /// gen writes the runner's commands into scripts and initconvertd runs
    /// them: with the constants, never a literal, or a renamed command
    /// would build and fail only at run time.
    #[test]
    fn runner_commands_are_written_as_constants() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut bad = Vec::new();
        let mut files = 0;
        let mut todo = vec![
            crates.join("uinitconverter-gen/src"),
            crates.join("uinitconverterd/src"),
        ];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                files += 1;
                for (n, line) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
                    let literal = runner_cmd::ALL.iter().chain(timers_cmd::ALL).any(|c| {
                        line.contains(&format!("{{runner}} {c}"))
                            || line.contains(&format!("{{timer_runner}} {c}"))
                            || line.contains(&format!("run_runner(\"{c}\""))
                            || line.contains(&format!("runner_cmd(\"{c}\""))
                    });
                    if literal {
                        bad.push(format!("{}:{}: {}", p.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(files > 10, "no sources under {}", crates.display());
        assert!(
            bad.is_empty(),
            "write config::runner_cmd / timers_cmd:\n{}",
            bad.join("\n")
        );
    }

    fn read(rel: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// KEYS is what the parser's match takes: no key parsed but unlisted,
    /// none listed but not parsed.
    #[test]
    fn keys_are_the_parsers() {
        let src = read("src/config.rs");
        let start = src.find("        for (k, v) in vars {").unwrap();
        let end = src[start..].find("                other => {").unwrap() + start;
        let mut parsed = std::collections::BTreeSet::new();
        for line in src[start..end].lines() {
            let t = line.trim_start();
            if !t.starts_with('"') || !t.contains("=>") {
                continue;
            }
            let arm = &t[..t.find("=>").unwrap()];
            for k in arm.split('|') {
                let k = k.trim().trim_matches('"');
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                    parsed.insert(k.to_string());
                }
            }
        }
        let listed: std::collections::BTreeSet<String> =
            super::KEYS.iter().map(|k| k.to_string()).collect();
        assert_eq!(parsed, listed);
    }

    /// dist/uinitconverter.conf documents every key a user may set, and
    /// names no key the parser doesn't know.
    #[test]
    fn dist_conf_documents_the_keys() {
        let conf = read("../../dist/uinitconverter.conf");
        for line in conf.lines() {
            let l = line.trim_start_matches('#');
            if let Some((k, _)) = l.split_once('=') {
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                    assert!(
                        super::KEYS.contains(&k),
                        "dist/uinitconverter.conf: {k} is not a key"
                    );
                }
            }
        }
        let words: std::collections::BTreeSet<&str> = conf
            .split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
            .collect();
        for k in super::KEYS {
            if super::REMOVED_KEYS.contains(k) {
                continue;
            }
            assert!(
                words.contains(k),
                "{k} is not documented in dist/uinitconverter.conf"
            );
        }
    }

    #[test]
    fn command_names_are_unique() {
        let mut all: Vec<&str> = runner_cmd::ALL
            .iter()
            .chain(timers_cmd::ALL)
            .copied()
            .collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
    }
}
