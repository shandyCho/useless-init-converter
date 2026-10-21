//! sdcompat-runner: runs one systemd service under s6, called from the
//! scripts sdcompat-gen writes and by sdcompatd.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use sdcompat_base::{config, envfile, fdstore, index, loader, mountunit, state, unit, util};
pub(crate) use sdcompat_client as client;
pub(crate) use sdcompat_exec::{bpf, cgroup, conditions, execcmd, pam, sandbox, seccomp, selinux, socket};
pub(crate) use sdcompat_install as install;

mod runner;

use config::Config;

const USAGE: &str = "\
usage: sdcompat-runner [--config PATH] COMMAND [--config PATH] [--mainpid N] (-- UNIT | --unit-hex HEX) [ARGS]
       sdcompat-runner --version

  run          longrun `run` script; ends by exec()ing the main process
  finish CODE SIG
               longrun `finish` script; cleanup and Restart= policy
  up | down    oneshot scripts (also mount and swap units)
  stop-exec    ExecStop= with $MAINPID (called by sdcompatd)
  reload-exec  ExecReload= with $MAINPID (called by sdcompatd)
";

fn die(msg: &str) -> ! {
    eprintln!("sdcompat-runner: {msg}");
    exit(1)
}

struct Internal {
    unit: String,
    main_pid: Option<u32>,
    rest: Vec<String>,
}

fn parse_internal(args: &[String]) -> Internal {
    let mut unit = None;
    let mut main_pid = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mainpid" => {
                i += 1;
                main_pid = args.get(i).and_then(|p| p.parse().ok());
            }
            "--unit-hex" => {
                i += 1;
                let hex = args.get(i).cloned().unwrap_or_default();
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .filter_map(|j| u8::from_str_radix(&hex[j * 2..j * 2 + 2], 16).ok())
                    .collect();
                unit = Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            "--" => {
                i += 1;
                if unit.is_none() {
                    unit = args.get(i).cloned();
                    i += 1;
                }
                rest.extend(args[i.min(args.len())..].iter().cloned());
                break;
            }
            other if unit.is_none() && !other.starts_with('-') => unit = Some(other.to_string()),
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    Internal {
        unit: unit.unwrap_or_else(|| die("missing unit name")),
        main_pid,
        rest,
    }
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
        sdcompat_version::print("sdcompat-runner");
        exit(0);
    }
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let a = parse_internal(&args[1..]);
    let r = runner::Runner::new(cfg, &a.unit).unwrap_or_else(|e| die(&e));
    let code = match cmd.as_str() {
        "run" => r.cmd_run(),
        "finish" => {
            let code = a.rest.first().and_then(|c| c.parse().ok()).unwrap_or(0);
            let sig = a.rest.get(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            r.cmd_finish(code, sig)
        }
        "up" => r.cmd_up(),
        "down" => r.cmd_down(),
        "stop-exec" => r.cmd_stop_exec(a.main_pid),
        "reload-exec" => r.cmd_reload(a.main_pid),
        other => die(&format!("unknown command {other} (see sdcompat-runner help)")),
    };
    exit(code);
}
