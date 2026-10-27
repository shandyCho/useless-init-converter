//! uinitconverter-runner: runs one systemd service under s6, called from the
//! scripts uinitconverter-gen writes and by uinitconverterd.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use uinitconverter_base::{config, envfile, fdstore, index, loader, mountunit, state, unit, util};
pub(crate) use uinitconverter_client as client;
pub(crate) use uinitconverter_exec::{bpf, cgroup, conditions, execcmd, pam, sandbox, seccomp, selinux, socket};
pub(crate) use uinitconverter_install as install;

mod runner;

use config::Config;

const USAGE: &str = "\
usage: uinitconverter-runner [--config PATH] COMMAND [--config PATH] [--mainpid N] (-- UNIT | --unit-hex HEX) [ARGS]
       uinitconverter-runner --version

  run [--on-demand]
               longrun `run` script; ends by exec()ing the main process
               (--on-demand: first wait for activity on the unit's sockets)
  finish CODE SIG
               longrun `finish` script; cleanup and Restart= policy
  up | down    oneshot scripts (also mount and swap units)
  stop-exec    ExecStop= with $MAINPID (called by uinitconverterd)
  stop-atom    `down` of UNIT.stop: ExecStop= when s6-rc stops a longrun
  reload-exec  ExecReload= with $MAINPID (called by uinitconverterd)
";

fn die(msg: &str) -> ! {
    eprintln!("uinitconverter-runner: {msg}");
    exit(1)
}

struct Internal {
    unit: String,
    main_pid: Option<u32>,
    on_demand: bool,
    rest: Vec<String>,
}

fn parse_internal(args: &[String]) -> Internal {
    let mut unit = None;
    let mut main_pid = None;
    let mut on_demand = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mainpid" => {
                i += 1;
                main_pid = args.get(i).and_then(|p| p.parse().ok());
            }
            "--on-demand" => on_demand = true,
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
        on_demand,
        rest,
    }
}

/// The unit could not be set up (User= or Group= that does not exist when
/// starting, a missing EnvironmentFile=, the unit file gone). As for any other failed
/// start, s6-rc must still see it as done (every start is reported as
/// successful; the result goes to the state files), or the boot's change
/// waits for its timeout and skips the units after it: `run` records the
/// failure, reports ready and waits like a unit whose requirement failed;
/// `finish` says "don't restart" instead of failing every second; `up`
/// records it and succeeds.
fn setup_failed(cfg: &Config, cmd: &str, unit: &str, e: &str) -> i32 {
    use state::keys;
    eprintln!("uinitconverter-runner: {unit}: {e}");
    let st = state::for_unit(&cfg.run_dir, &loader::Loader::new(cfg).load(unit).id);
    match cmd {
        "run" | "up" => {
            st.stamp(keys::INACTIVE_EXIT);
            st.set(keys::RESULT, "resources");
            st.set(keys::START_FAILED, e);
            st.clear(keys::PARKED);
            st.stamp(keys::INACTIVE_ENTER);
            if cmd == "up" {
                return 0;
            }
            unsafe {
                if libc::fcntl(3, libc::F_GETFD) >= 0 {
                    libc::write(3, b"\n".as_ptr() as *const libc::c_void, 1);
                    libc::close(3);
                }
                loop {
                    libc::pause();
                }
            }
        }
        "finish" => 125,
        "down" | "stop-atom" => 0,
        _ => 1,
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
    if uinitconverter_version::wanted(&cmd) {
        uinitconverter_version::print("uinitconverter-runner");
        exit(0);
    }
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let a = parse_internal(&args[1..]);
    let mut r = match runner::Runner::new(cfg.clone(), &a.unit, matches!(cmd.as_str(), "run" | "up")) {
        Ok(r) => r,
        Err(e) => exit(setup_failed(&cfg, &cmd, &a.unit, &e)),
    };
    r.on_demand = a.on_demand;
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
        "stop-atom" => r.cmd_stop_atom(),
        "reload-exec" => r.cmd_reload(a.main_pid),
        other => die(&format!("unknown command {other} (see uinitconverter-runner help)")),
    };
    exit(code);
}
