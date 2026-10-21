//! sdcompat-timers: runs .timer and .path units, each as its own s6
//! longrun, and asks sdcompatd to start the unit they trigger.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use sdcompat_base::{config, loader, state, util};
pub(crate) use sdcompat_client as client;

mod pathunit;
mod timer;

use config::Config;

const USAGE: &str = "\
usage: sdcompat-timers [--config PATH] COMMAND [--config PATH] (-- UNIT | --unit-hex HEX)
       sdcompat-timers --version

  timer-run    compute the timer's next elapse, sleep, start its unit; repeat
  path-run     watch the .path unit's paths with inotify, start its unit
";

fn die(msg: &str) -> ! {
    eprintln!("sdcompat-timers: {msg}");
    exit(1)
}

/// The unit name: `-- NAME` or `--unit-hex HEX` (names unsafe for sh).
fn unit_arg(args: &[String]) -> String {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => return it.next().cloned().unwrap_or_else(|| die("missing unit name")),
            "--unit-hex" => {
                let hex = it.next().cloned().unwrap_or_default();
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .filter_map(|j| u8::from_str_radix(&hex[j * 2..j * 2 + 2], 16).ok())
                    .collect();
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            other if !other.starts_with('-') => return other.to_string(),
            _ => {}
        }
    }
    die("missing unit name")
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path: Option<PathBuf> = None;
    while args.first().map(|a| a == "--config").unwrap_or(false) {
        args.remove(0);
        if args.is_empty() {
            die("--config needs a path");
        }
        config_path = Some(PathBuf::from(args.remove(0)));
    }
    // Generated scripts put --config after the command name.
    if args.len() > 2 && args[1] == "--config" {
        config_path = Some(PathBuf::from(args.remove(2)));
        args.remove(1);
    }
    let Some(cmd) = args.first().cloned() else {
        eprint!("{USAGE}");
        exit(1);
    };
    if matches!(cmd.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        exit(0);
    }
    if sdcompat_version::wanted(&cmd) {
        sdcompat_version::print("sdcompat-timers");
        exit(0);
    }
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let unit = unit_arg(&args[1..]);
    let code = match cmd.as_str() {
        "timer-run" => timer::cmd_timer_run(cfg, &unit),
        "path-run" => pathunit::cmd_path_run(cfg, &unit),
        other => die(&format!("unknown command {other} (see sdcompat-timers help)")),
    };
    exit(code);
}
