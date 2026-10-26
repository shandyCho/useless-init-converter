//! sdcompatd: serves org.freedesktop.systemd1 on top of s6-rc, so systemctl,
//! libsystemd and logind keep working; also a minimal client for it.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use sdcompat_base::pathspec as pathunit;
pub(crate) use sdcompat_base::timerspec as timer;
pub(crate) use sdcompat_base::{config, envfile, fdstore, loader, mountunit, state, unit, util};
pub(crate) use sdcompat_exec::{cgroup, conditions, execcmd, socket};
pub(crate) use sdcompat_gen as gen;
pub(crate) use sdcompat_install as install;

mod daemon;

use config::Config;

const USAGE: &str = "\
usage: sdcompatd [--config PATH] [COMMAND] [ARGS]
       sdcompatd --version

  daemon [--system | --session | --address ADDR] [--ready-fd N]
                serve org.freedesktop.systemd1 (default: system bus and
                PRIVATE_SOCKET); with --ready-fd, write a newline to fd N
                when ready. This is also what sdcompatd does with no command.
  start|stop|restart UNIT...
                ask the running daemon (private socket or bus)
  stop-instances
                at shutdown (the shutting-down marker exists): ask the
                daemon to stop the on-demand instances (user@UID, ...) now,
                before s6-rc stops the services they are ordered after
  log UNIT [-f] show (or follow) a service's log: journalctl -u, or its
                s6-log directory with JOURNAL=no
";

fn die(msg: &str) -> ! {
    eprintln!("sdcompatd: {msg}");
    exit(1)
}

fn serve(cfg: Config, rest: &[String]) -> i32 {
    let mut bus = daemon::Bus::System;
    let mut ready_fd = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--system" => bus = daemon::Bus::System,
            "--session" => bus = daemon::Bus::Session,
            "--address" => bus = daemon::Bus::Address(it.next().cloned().unwrap_or_else(|| die("--address needs a value"))),
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

fn client(cfg: &Config, cmd: &str, units: &[String]) -> i32 {
    let method = match cmd {
        "start" => "StartUnit",
        "stop" => "StopUnit",
        _ => "RestartUnit",
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let conn = sdcompat_client::connect(cfg).await.unwrap_or_else(|e| die(&e));
        let mut code = 0;
        for u in units {
            if let Err(e) = sdcompat_client::manager_call(&conn, method, &(u.as_str(), "replace")).await {
                eprintln!("sdcompatd: {u}: {e}");
                code = 1;
            }
        }
        code
    })
}

/// rc.shutdown: stop the on-demand instances while the s6-rc services they
/// are ordered after still run. Waits for them (no D-Bus call timeout).
fn stop_instances(cfg: &Config) -> i32 {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let conn = sdcompat_client::connect(cfg).await.unwrap_or_else(|e| die(&e));
        let r = conn
            .call_method(
                Some("org.freedesktop.systemd1"),
                "/org/freedesktop/systemd1",
                Some(sdcompat_client::SDCOMPAT_IFACE),
                "StopInstances",
                &(),
            )
            .await;
        match r {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("sdcompatd: stop-instances: {e}");
                1
            }
        }
    })
}

fn log(cfg: &Config, rest: &[String]) -> ! {
    let follow = rest.iter().any(|a| a == "-f" || a == "--follow");
    let Some(unit) = rest.iter().find(|a| !a.starts_with('-')) else { die("usage: sdcompatd log UNIT [-f]") };
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

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // --config PATH before or after the command (generated scripts and the
    // sdcompat front end put it after), up to a "--".
    let mut config_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        if args[i] == "--config" {
            args.remove(i);
            if i >= args.len() {
                die("--config needs a path");
            }
            config_path = Some(PathBuf::from(args.remove(i)));
        } else {
            i += 1;
        }
    }
    let cmd = args.first().cloned().unwrap_or_else(|| "daemon".into());
    if matches!(cmd.as_str(), "help" | "--help" | "-h") || args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        exit(0);
    }
    if sdcompat_version::wanted(&cmd) {
        sdcompat_version::print("sdcompatd");
        exit(0);
    }
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let code = match cmd.as_str() {
        "daemon" => serve(cfg, rest),
        o if o.starts_with("--") => serve(cfg, &args),
        "start" | "stop" | "restart" => client(&cfg, &cmd, rest),
        "stop-instances" => stop_instances(&cfg),
        "log" => log(&cfg, rest),
        other => die(&format!("unknown command {other} (see sdcompatd help)")),
    };
    exit(code);
}
