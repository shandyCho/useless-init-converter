//! initconvert-devices: starts the units udev's devices want
//! (SYSTEMD_WANTS). It reads udev's database and udevd's events and asks
//! initconvertd to start each newly wanted unit, as initconvert-timers does
//! for timers; the daemon itself doesn't watch devices.

use std::io::Write;
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use uinitconverter_base::config::{Config, UINITCONVERTER_IFACE};
use uinitconverter_client as client;

mod devices;

use devices::Msg;

const USAGE: &str = "\
usage: initconvert-devices [--config PATH] [--ready-fd N]
       initconvert-devices --version

Starts the units that devices want (udev's SYSTEMD_WANTS): those of the
devices in udev's database (UDEV_DIR) at startup, then those of udevd's
events (UDEV_MONITOR=yes). The devices seen are kept in RUN_DIR/devices.

  --ready-fd N   write a newline to fd N once it listens (s6 readiness)
";

fn die(msg: &str) -> ! {
    eprintln!("initconvert-devices: {msg}");
    exit(1)
}

/// Ask initconvertd to start `unit` for `dev` (StartWanted: a start that
/// never reloads, whatever the unit). The daemon may not be up yet, or be
/// restarting: tried again for a while.
async fn start_wanted(cfg: Arc<Config>, unit: String, dev: String) {
    let mut last = String::new();
    for attempt in 0..30 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let conn = match client::connect(&cfg).await {
            Ok(c) => c,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let r = conn
            .call_method(
                Some("org.freedesktop.systemd1"),
                "/org/freedesktop/systemd1",
                Some(UINITCONVERTER_IFACE),
                "StartWanted",
                &(unit.as_str(), dev.as_str()),
            )
            .await;
        match r {
            // The daemon logs the job's result ("... wanted by device ...").
            Ok(_) => return,
            // The daemon's answer about the unit: final.
            Err(zbus::Error::MethodError(_, msg, _)) => {
                eprintln!(
                    "initconvert-devices: device {dev} wants {unit}: {}",
                    msg.unwrap_or_default()
                );
                return;
            }
            Err(e) => last = e.to_string(),
        }
    }
    eprintln!("initconvert-devices: device {dev} wants {unit}: initconvertd not reached: {last}");
}

fn main() {
    let mut config_path: Option<PathBuf> = None;
    let mut ready_fd: Option<i32> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                config_path = Some(PathBuf::from(
                    args.next().unwrap_or_else(|| die("--config needs a path")),
                ))
            }
            "--ready-fd" => {
                ready_fd = Some(
                    args.next()
                        .and_then(|f| f.parse().ok())
                        .unwrap_or_else(|| die("--ready-fd needs a number")),
                )
            }
            "help" | "--help" | "-h" => {
                print!("{USAGE}");
                exit(0)
            }
            v if uinitconverter_version::wanted(v) => {
                uinitconverter_version::print("initconvert-devices");
                exit(0)
            }
            o => die(&format!("unknown argument {o} (see --help)")),
        }
    }
    let cfg = Arc::new(Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e)));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| die(&e.to_string()));
    rt.block_on(run(cfg, ready_fd));
}

async fn run(cfg: Arc<Config>, ready_fd: Option<i32>) {
    let ready = || {
        if let Some(fd) = ready_fd {
            let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
            let _ = f.write_all(b"\n");
        }
    };
    // UDEV_DIR= (empty): device-wanted units are off. Stay up for s6.
    let Some(udev_dir) = cfg.udev_dir.clone() else {
        ready();
        return std::future::pending().await;
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // Listen before scanning, so nothing between the two is missed.
    if cfg.udev_monitor {
        match devices::open_monitor() {
            Ok(fd) => {
                let tx = tx.clone();
                std::thread::spawn(move || devices::monitor_loop(fd, tx));
            }
            Err(e) => eprintln!("initconvert-devices: cannot listen to udev events: {e}"),
        }
    }
    ready();
    let state = cfg.run_dir.join("devices");
    let mut plugged = devices::load_plugged(&state);
    let spawn = |wanted: Vec<(String, String)>| {
        for (dev, unit) in wanted {
            tokio::spawn(start_wanted(cfg.clone(), unit, dev));
        }
    };
    spawn(devices::sync(&mut plugged, devices::scan_db(&udev_dir)));
    devices::save_plugged(&state, &plugged);
    // `tx` is kept: without a monitor this waits for good (s6 keeps it up).
    while let Some(m) = rx.recv().await {
        match m {
            Msg::Event(e) => {
                let dev = e.id.clone();
                let units = devices::newly_wanted(&mut plugged, e);
                spawn(units.into_iter().map(|u| (dev.clone(), u)).collect());
            }
            Msg::Rescan => spawn(devices::sync(&mut plugged, devices::scan_db(&udev_dir))),
        }
        devices::save_plugged(&state, &plugged);
    }
}
