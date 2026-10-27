//! What the runner asks of its supervisor, s6-supervise: whether the service
//! is still wanted up, and the supervised process of a service. (Readiness
//! goes the other way, on the notification-fd pipe: Runner::signal_ready.)

use crate::config::Config;
use crate::tools;

/// s6-svstat on the current directory (finish runs in the service dir):
/// false once the service was told to stay down (s6-svc -d).
pub fn wanted_up() -> bool {
    std::process::Command::new(tools::path("s6-svstat"))
        .args(["-o", "wantedup", "."])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() != "false")
        .unwrap_or(true)
}

/// Whether service ID is up (None: down or unknown) and, if so, the PID
/// s6-supervise runs for it (None when it gives none).
pub fn up_pid(cfg: &Config, id: &str) -> Option<Option<u32>> {
    let out = std::process::Command::new(tools::path("s6-svstat"))
        .args(["-o", "up,pid"])
        .arg(cfg.live_dir.join("scandir").join(id))
        .output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    match text.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["true", pid] => Some(pid.parse::<u32>().ok().filter(|&p| p > 0)),
        _ => None,
    }
}
