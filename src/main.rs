use std::path::PathBuf;
use std::process::exit;

use sdcompat::config::Config;
use sdcompat::{daemon, fdstore, gen, install, runner, util};

const USAGE: &str = "\
usage: sdcompat [--config PATH] COMMAND [ARGS]

Database:
  reload [--no-update]     translate units, compile the s6-rc database and
                           switch the live state to it (s6-rc-update)
  plan                     print the translation report without writing anything

Daemon:
  daemon [--system | --session | --address ADDR] [--ready-fd N]
                           serve org.freedesktop.systemd1 (default: system bus);
                           with --ready-fd, write a newline to fd N when ready
  fdstore [--ready-fd N]   hold FDSTORE= fds and listening sockets, so they
                           survive restarts of the daemon too

Offline unit file management:
  enable|disable|mask|unmask|preset UNIT...
  is-enabled UNIT

Tools:
  notify [--ready] [--status=TEXT] [VAR=VALUE...]   systemd-notify replacement
  log UNIT [-f]            show (or follow) a service's s6-log output

Internal (used by generated s6-rc scripts and the daemon):
  run|finish|up|down|stop-exec|reload-exec|timer-run|path-run [--mainpid N] (-- UNIT | --unit-hex HEX) [ARGS]
";

fn die(msg: &str) -> ! {
    eprintln!("sdcompat: {msg}");
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

fn print_changes(changes: &[install::Change]) {
    for c in changes {
        if c.kind == "symlink" {
            println!("Created symlink {} → {}.", c.path, c.source);
        } else {
            println!("Removed \"{}\".", c.path);
        }
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
    // Internal subcommands also accept --config after the command name.
    if args.len() > 2 && args[1] == "--config" {
        config_path = Some(PathBuf::from(args.remove(2)));
        args.remove(1);
    }
    let Some(cmd) = args.first().cloned() else {
        eprint!("{USAGE}");
        exit(1);
    };
    let rest = &args[1..];
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));

    let code = match cmd.as_str() {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            0
        }
        "plan" => {
            print!("{}", gen::report(&gen::plan(&cfg)));
            0
        }
        "reload" => {
            let update = !rest.iter().any(|a| a == "--no-update");
            match gen::regenerate(&cfg, update) {
                Ok(r) => {
                    let first = gen::report(&r.plan);
                    println!("{}", first.lines().next().unwrap_or(""));
                    println!(
                        "database: {}{}",
                        r.db_dir.display(),
                        if r.updated_live { " (live state updated)" } else { "" }
                    );
                    println!("report:   {}", r.db_dir.join("report.txt").display());
                    0
                }
                Err(e) => die(&e),
            }
        }
        "daemon" => {
            let mut bus = daemon::Bus::System;
            let mut ready_fd = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--system" => bus = daemon::Bus::System,
                    "--session" => bus = daemon::Bus::Session,
                    "--address" => {
                        bus = daemon::Bus::Address(it.next().cloned().unwrap_or_else(|| die("--address needs a value")))
                    }
                    "--ready-fd" => {
                        ready_fd = Some(it.next().and_then(|f| f.parse().ok()).unwrap_or_else(|| die("--ready-fd needs a number")))
                    }
                    o => die(&format!("unknown daemon option {o}")),
                }
            }
            util::tee_stderr_to_syslog(c"sdcompatd");
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| die(&e.to_string()));
            match rt.block_on(daemon::run(cfg, bus, ready_fd)) {
                Ok(()) => 0,
                Err(e) => die(&e),
            }
        }
        "fdstore" => {
            let ready_fd = match rest {
                [] => None,
                [o, n] if o == "--ready-fd" => Some(n.parse().unwrap_or_else(|_| die("--ready-fd needs a number"))),
                _ => die("usage: sdcompat fdstore [--ready-fd N]"),
            };
            fdstore::serve_forever(cfg, ready_fd)
        }
        "notify" => runner::cmd_notify(rest),
        "start" | "stop" | "restart" => {
            let method = match cmd.as_str() {
                "start" => "StartUnit",
                "stop" => "StopUnit",
                _ => "RestartUnit",
            };
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let conn = sdcompat::client::connect(&cfg).await.unwrap_or_else(|e| die(&e));
                let mut code = 0;
                for u in rest {
                    if let Err(e) = sdcompat::client::manager_call(&conn, method, &(u.as_str(), "replace")).await {
                        eprintln!("sdcompat: {u}: {e}");
                        code = 1;
                    }
                }
                code
            })
        }
        "log" => {
            let follow = rest.iter().any(|a| a == "-f" || a == "--follow");
            let Some(unit) = rest.iter().find(|a| !a.starts_with('-')) else { die("usage: sdcompat log UNIT [-f]") };
            if cfg.journal {
                let mut j = std::process::Command::new("journalctl");
                j.args(["-u", unit, "--no-pager"]);
                j.arg(if follow { "-f" } else { "-n200" });
                let err = std::os::unix::process::CommandExt::exec(&mut j);
                die(&format!("journalctl: {err}"))
            }
            let Some(dir) = &cfg.log_dir else { die("LOG_DIR is not configured") };
            let file = dir.join(unit).join("current");
            let mut tail = std::process::Command::new("tail");
            tail.arg(if follow { "-F" } else { "-n" });
            if !follow {
                tail.arg("200");
            }
            let err = std::os::unix::process::CommandExt::exec(tail.arg(&file));
            die(&format!("tail {}: {err}", file.display()))
        }
        "enable" | "disable" | "mask" | "unmask" | "preset" => {
            let names: Vec<String> = rest.iter().filter(|a| !a.starts_with("--")).cloned().collect();
            let runtime = rest.iter().any(|a| a == "--runtime");
            let force = rest.iter().any(|a| a == "--force");
            let res = match cmd.as_str() {
                "enable" => install::enable(&cfg, &names, runtime, force).map(|(_, c)| c),
                "disable" => install::disable(&cfg, &names, runtime),
                "mask" => install::mask(&cfg, &names, runtime, force),
                "unmask" => install::unmask(&cfg, &names, runtime),
                _ => install::preset(&cfg, &names, "full", runtime, force).map(|(_, c)| c),
            };
            match res {
                Ok(c) => {
                    print_changes(&c);
                    0
                }
                Err(e) => die(&e),
            }
        }
        "is-enabled" => {
            let mut code = 1;
            for n in rest {
                match install::unit_file_state(&cfg, n) {
                    Ok(s) => {
                        if s.starts_with("enabled") || s == "static" || s == "alias" {
                            code = 0;
                        }
                        println!("{s}");
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
            code
        }
        "timer-run" => {
            let a = parse_internal(rest);
            sdcompat::timer::cmd_timer_run(cfg, &a.unit)
        }
        "path-run" => {
            let a = parse_internal(rest);
            sdcompat::pathunit::cmd_path_run(cfg, &a.unit)
        }
        "run" | "finish" | "up" | "down" | "stop-exec" | "reload-exec" => {
            let a = parse_internal(rest);
            let r = runner::Runner::new(cfg, &a.unit).unwrap_or_else(|e| die(&e));
            match cmd.as_str() {
                "run" => r.cmd_run(),
                "finish" => {
                    let code = a.rest.first().and_then(|c| c.parse().ok()).unwrap_or(0);
                    let sig = a.rest.get(1).and_then(|c| c.parse().ok()).unwrap_or(0);
                    r.cmd_finish(code, sig)
                }
                "up" => r.cmd_up(),
                "down" => r.cmd_down(),
                "stop-exec" => r.cmd_stop_exec(a.main_pid),
                _ => r.cmd_reload(a.main_pid),
            }
        }
        other => die(&format!("unknown command {other} (see sdcompat help)")),
    };
    exit(code);
}
