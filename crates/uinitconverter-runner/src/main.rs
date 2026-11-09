//! initconvert-runner: runs one systemd service under s6, called from the
//! scripts initconvert-gen writes and by initconvertd.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use uinitconverter_base::{
    acceptspec, config, envfile, fdstore, index, loader, mountunit, snapshot, state, tools, unit,
    util,
};
pub(crate) use uinitconverter_client as client;
pub(crate) use uinitconverter_exec::{
    bpf, cgroup, conditions, execcmd, pam, sandbox, seccomp, selinux, socket,
};
pub(crate) use uinitconverter_install as install;

mod runner;
mod supervisor;

use config::Config;

const USAGE: &str = "\
usage: initconvert-runner [--config PATH] COMMAND [--config PATH] [--mainpid N] (-- UNIT | --unit-hex HEX) [ARGS]
       initconvert-runner --version

  run [--on-demand]
               longrun `run` script; ends by exec()ing the main process
               (--on-demand: first wait for activity on the unit's sockets)
  finish CODE SIG
               longrun `finish` script; cleanup and Restart= policy
  up | down    oneshot scripts (also mount and swap units)
  stop-exec    ExecStop= with $MAINPID (called by initconvertd)
  stop-atom    `down` of UNIT.stop: ExecStop= when s6-rc stops a longrun
  reload-exec  ExecReload= with $MAINPID (called by initconvertd)
  accept | accept-finish
               `run`/`finish` of an Accept=yes .socket: bind it, exec
               s6-ipcserverd, which runs `connection` for each client
  connection   one instance of the socket's template for the client on
               stdin/stdout (internal, run by s6-ipcserverd)
  confine UNIT -- COMMAND [ARGS]
               exec COMMAND with UNIT's cgroup, limits, sandbox and user (for
               what s6 starts itself, as the system bus; not stdio)
  notify-helper ...
               internal: the Type=notify helper `run` starts
";

fn die(msg: &str) -> ! {
    eprintln!("initconvert-runner: {msg}");
    exit(1)
}

/// The commands, from config::runner_cmd (the list gen, the daemon and
/// dist/s6 use): every match on Cmd is exhaustive, so a command added there
/// and not handled here fails to build (Cmd::ALL's test checks the list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Run,
    Finish,
    Up,
    Down,
    StopExec,
    StopAtom,
    ReloadExec,
    Confine,
    Accept,
    AcceptFinish,
    Connection,
}

impl Cmd {
    const ALL: [Cmd; 11] = [
        Cmd::Run,
        Cmd::Finish,
        Cmd::Up,
        Cmd::Down,
        Cmd::StopExec,
        Cmd::StopAtom,
        Cmd::ReloadExec,
        Cmd::Confine,
        Cmd::Accept,
        Cmd::AcceptFinish,
        Cmd::Connection,
    ];

    fn name(self) -> &'static str {
        use config::runner_cmd as c;
        match self {
            Cmd::Run => c::RUN,
            Cmd::Finish => c::FINISH,
            Cmd::Up => c::UP,
            Cmd::Down => c::DOWN,
            Cmd::StopExec => c::STOP_EXEC,
            Cmd::StopAtom => c::STOP_ATOM,
            Cmd::ReloadExec => c::RELOAD_EXEC,
            Cmd::Confine => c::CONFINE,
            Cmd::Accept => c::ACCEPT,
            Cmd::AcceptFinish => c::ACCEPT_FINISH,
            Cmd::Connection => c::CONNECTION,
        }
    }

    fn parse(s: &str) -> Option<Cmd> {
        Cmd::ALL.into_iter().find(|c| c.name() == s)
    }
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
fn setup_failed(cfg: &Config, cmd: Cmd, unit: &str, e: &str) -> i32 {
    use state::{keys, result};
    eprintln!("initconvert-runner: {unit}: {e}");
    let st = state::for_unit(&cfg.run_dir, &snapshot::Units::new(cfg).id(unit));
    match cmd {
        Cmd::Run | Cmd::Up => {
            st.stamp(keys::INACTIVE_EXIT);
            st.set(keys::RESULT, result::RESOURCES);
            st.set(keys::START_FAILED, e);
            st.clear(keys::PARKED);
            st.stamp(keys::INACTIVE_ENTER);
            if cmd == Cmd::Up {
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
        Cmd::Finish => 125,
        Cmd::Down | Cmd::StopAtom => 0,
        Cmd::StopExec
        | Cmd::ReloadExec
        | Cmd::Confine
        | Cmd::Accept
        | Cmd::AcceptFinish
        | Cmd::Connection => 1,
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
        uinitconverter_version::print("initconvert-runner");
        exit(0);
    }
    // Internal: `run` re-executes into it; it loads no config or unit.
    if cmd == "notify-helper" {
        exit(runner::notify_helper_main(&args[1..]));
    }
    let Some(cmd) = Cmd::parse(&cmd) else {
        die(&format!(
            "unknown command {cmd} (see initconvert-runner help)"
        ))
    };
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let a = parse_internal(&args[1..]);
    // Accept=yes sockets: UNIT is the .socket, not a service.
    match cmd {
        Cmd::Accept => exit(runner::accept_main(&cfg, &a.unit)),
        Cmd::AcceptFinish => exit(runner::accept_finish(&cfg, &a.unit)),
        Cmd::Connection => exit(runner::connection_main(&cfg, &a.unit)),
        _ => {}
    }
    let starting = matches!(cmd, Cmd::Run | Cmd::Up | Cmd::Confine);
    let made = if cmd == Cmd::Confine {
        runner::Runner::new_from_disk(cfg.clone(), &a.unit, starting)
    } else {
        runner::Runner::new(cfg.clone(), &a.unit, starting)
    };
    let mut r = match made {
        Ok(r) => r,
        Err(e) => exit(setup_failed(&cfg, cmd, &a.unit, &e)),
    };
    r.on_demand = a.on_demand;
    let code = match cmd {
        Cmd::Run => r.cmd_run(),
        Cmd::Finish => {
            let code = a.rest.first().and_then(|c| c.parse().ok()).unwrap_or(0);
            let sig = a.rest.get(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            r.cmd_finish(code, sig)
        }
        Cmd::Up => r.cmd_up(),
        Cmd::Down => r.cmd_down(),
        Cmd::StopExec => r.cmd_stop_exec(a.main_pid),
        Cmd::StopAtom => r.cmd_stop_atom(),
        Cmd::ReloadExec => r.cmd_reload(a.main_pid),
        Cmd::Confine => r.cmd_confine(&a.rest),
        Cmd::Accept | Cmd::AcceptFinish | Cmd::Connection => unreachable!("handled above"),
    };
    exit(code);
}

#[cfg(test)]
mod cmd_tests {
    use super::{config, Cmd};

    /// The enum and the shared list name the same commands, in the same order.
    #[test]
    fn every_listed_command_is_handled() {
        let names: Vec<&str> = Cmd::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names, config::runner_cmd::ALL);
        for n in config::runner_cmd::ALL {
            assert_eq!(Cmd::parse(n).map(Cmd::name), Some(*n));
        }
        assert_eq!(Cmd::parse("nope"), None);
    }

    /// dist/s6's hand-written run scripts call the runner too (confine).
    #[test]
    fn dist_scripts_use_listed_commands() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/s6");
        let mut seen = 0;
        let mut bad = Vec::new();
        for svc in std::fs::read_dir(&dist).unwrap().flatten() {
            for f in ["run", "finish", "up", "down"] {
                let Ok(text) = std::fs::read_to_string(svc.path().join(f)) else {
                    continue;
                };
                for (n, line) in text.lines().enumerate() {
                    let Some(rest) = line.split("initconvert-runner ").nth(1) else {
                        continue;
                    };
                    seen += 1;
                    let cmd = rest.split_whitespace().next().unwrap_or("");
                    if Cmd::parse(cmd).is_none() {
                        bad.push(format!("{}/{f}:{}: {cmd}", svc.path().display(), n + 1));
                    }
                }
            }
        }
        assert!(seen > 0, "no runner calls found under {}", dist.display());
        assert!(bad.is_empty(), "not runner commands:\n{}", bad.join("\n"));
    }
}
