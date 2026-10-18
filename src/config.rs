//! Shared configuration, read from /etc/sdcompat/sdcompat.conf (KEY=VALUE).
//!
//! The generator embeds `--config PATH` into generated run scripts when a
//! non-default config is used, so the runner always sees the same settings.

use std::path::{Path, PathBuf};

use crate::envfile;

pub const DEFAULT_CONFIG: &str = "/etc/sdcompat/sdcompat.conf";

#[derive(Debug, Clone)]
pub struct Config {
    pub config_path: PathBuf,
    /// Unit search path, highest priority first. The first entry is where
    /// enable/disable/mask write their symlinks.
    pub unit_path: Vec<PathBuf>,
    /// Directory for runtime (`--runtime`) enablement symlinks.
    pub runtime_unit_dir: PathBuf,
    /// Preset directories, highest priority first.
    pub preset_path: Vec<PathBuf>,
    /// Hand-written s6-rc source directories compiled together with generated ones.
    pub extra_source: Vec<PathBuf>,
    /// Generated sources and compiled databases live here.
    pub state_dir: PathBuf,
    /// s6-rc live directory.
    pub live_dir: PathBuf,
    /// Per-unit runtime state (notify sockets, results, status text).
    pub run_dir: PathBuf,
    /// When set, every longrun gets an s6-log consumer writing to LOG_DIR/<unit>.
    pub log_dir: Option<PathBuf>,
    /// Unit name globs that are never translated.
    pub exclude: Vec<String>,
    /// Extra s6-rc service/bundle added to sysinit.target (udev, mounts, ...).
    pub sysinit_bundle: String,
    /// Bundle generated for default.target; include it in your boot bundle.
    pub default_bundle: String,
    /// Created once the daemon is ready, so sd_booted() returns true.
    pub booted_flag: Option<PathBuf>,
    /// Peer-to-peer socket used by systemctl when run as root.
    pub private_socket: Option<PathBuf>,
    /// Path of the sdcompat binary as seen from generated scripts.
    pub runner: PathBuf,
    /// Shutdown commands used for PowerOff/Reboot/Halt.
    pub poweroff_cmd: String,
    pub reboot_cmd: String,
    pub halt_cmd: String,
    /// Users (besides root) allowed to call mutating D-Bus methods.
    pub allow_uids: Vec<u32>,
}

impl Config {
    pub fn defaults() -> Config {
        let runner = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("/usr/libexec/sdcompat/sdcompat"));
        Config {
            config_path: PathBuf::from(DEFAULT_CONFIG),
            unit_path: [
                "/etc/systemd/system",
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
            extra_source: vec![PathBuf::from("/etc/sdcompat/s6")],
            state_dir: PathBuf::from("/var/lib/sdcompat"),
            live_dir: PathBuf::from("/run/s6-rc"),
            run_dir: PathBuf::from("/run/sdcompat"),
            log_dir: Some(PathBuf::from("/var/log/sdcompat")),
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
                "sdcompat*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            sysinit_bundle: "sdcompat-sysinit".into(),
            default_bundle: "sdcompat-default".into(),
            booted_flag: Some(PathBuf::from("/run/systemd/system")),
            private_socket: Some(PathBuf::from("/run/systemd/private")),
            runner,
            poweroff_cmd: "s6-linux-init-shutdown -p now".into(),
            reboot_cmd: "s6-linux-init-shutdown -r now".into(),
            halt_cmd: "s6-linux-init-shutdown -h now".into(),
            allow_uids: Vec::new(),
        }
    }

    /// Load defaults, then the config file (if present).
    pub fn load(path: Option<&Path>) -> Result<Config, String> {
        let mut cfg = Config::defaults();
        let explicit = path.is_some() || std::env::var_os("SDCOMPAT_CONFIG").is_some();
        let path = path
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("SDCOMPAT_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
        cfg.config_path = path.clone();

        let vars = match envfile::load(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => return Ok(cfg),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        for (k, v) in vars {
            let paths = |v: &str| -> Vec<PathBuf> {
                v.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
            };
            let opt = |v: &str| (!v.is_empty()).then(|| PathBuf::from(v));
            match k.as_str() {
                "UNIT_PATH" => cfg.unit_path = paths(&v),
                "RUNTIME_UNIT_DIR" => cfg.runtime_unit_dir = PathBuf::from(&v),
                "PRESET_PATH" => cfg.preset_path = paths(&v),
                "EXTRA_SOURCE" => cfg.extra_source = paths(&v),
                "STATE_DIR" => cfg.state_dir = PathBuf::from(&v),
                "LIVE_DIR" => cfg.live_dir = PathBuf::from(&v),
                "RUN_DIR" => cfg.run_dir = PathBuf::from(&v),
                "LOG_DIR" => cfg.log_dir = opt(&v),
                "EXCLUDE" => cfg.exclude = v.split_whitespace().map(String::from).collect(),
                "EXCLUDE_ADD" => cfg.exclude.extend(v.split_whitespace().map(String::from)),
                "SYSINIT_BUNDLE" => cfg.sysinit_bundle = v,
                "DEFAULT_BUNDLE" => cfg.default_bundle = v,
                "BOOTED_FLAG" => cfg.booted_flag = opt(&v),
                "PRIVATE_SOCKET" => cfg.private_socket = opt(&v),
                "RUNNER" => cfg.runner = PathBuf::from(&v),
                "POWEROFF_CMD" => cfg.poweroff_cmd = v,
                "REBOOT_CMD" => cfg.reboot_cmd = v,
                "HALT_CMD" => cfg.halt_cmd = v,
                "ALLOW_UIDS" => {
                    cfg.allow_uids = v.split_whitespace().filter_map(|s| s.parse().ok()).collect()
                }
                other => eprintln!("sdcompat: {}: unknown key {other}", path.display()),
            }
        }
        Ok(cfg)
    }

    /// Writable unit directory for enable/disable/mask.
    pub fn config_unit_dir(&self) -> &Path {
        &self.unit_path[0]
    }

    pub fn unit_run_dir(&self, unit: &str) -> PathBuf {
        self.run_dir.join("units").join(unit)
    }

    /// Arguments generated scripts pass to the runner so it loads this config.
    pub fn runner_args(&self) -> String {
        if self.config_path == Path::new(DEFAULT_CONFIG) {
            String::new()
        } else {
            format!(" --config {}", self.config_path.display())
        }
    }

    pub fn is_excluded(&self, unit: &str) -> bool {
        self.exclude.iter().any(|g| crate::util::fnmatch(g, unit))
    }
}
