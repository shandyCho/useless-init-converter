//! initconvertd: serves org.freedesktop.systemd1 on top of s6-rc, so systemctl,
//! libsystemd and logind keep working.

use std::path::PathBuf;
use std::process::exit;

pub(crate) use uinitconverter_base::acceptspec;
pub(crate) use uinitconverter_base::pathspec as pathunit;
pub(crate) use uinitconverter_base::timerspec as timer;
pub(crate) use uinitconverter_base::{
    config, envfile, fdstore, loader, mountunit, snapshot, state, tools, unit, util,
};
pub(crate) use uinitconverter_exec::{cgroup, conditions, execcmd, socket};
pub(crate) use uinitconverter_gen as gen;
pub(crate) use uinitconverter_install as install;

mod daemon;

use config::Config;

const USAGE: &str = "\
usage: initconvertd [--config PATH] [daemon] [OPTIONS]
       initconvertd --version

  daemon [--system | --session | --address ADDR] [--ready-fd N]
                serve org.freedesktop.systemd1 (default: system bus and
                PRIVATE_SOCKET); with --ready-fd, write a newline to fd N
                when ready. This is also what initconvertd does with no command.
";

fn die(msg: &str) -> ! {
    eprintln!("initconvertd: {msg}");
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
            "--address" => {
                bus = daemon::Bus::Address(
                    it.next()
                        .cloned()
                        .unwrap_or_else(|| die("--address needs a value")),
                )
            }
            "--ready-fd" => {
                ready_fd = Some(
                    it.next()
                        .and_then(|f| f.parse().ok())
                        .unwrap_or_else(|| die("--ready-fd needs a number")),
                )
            }
            o => die(&format!("unknown daemon option {o}")),
        }
    }
    util::tee_stderr_to_syslog(c"initconvertd");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| die(&e.to_string()));
    match rt.block_on(daemon::run(cfg, bus, ready_fd)) {
        Ok(()) => 0,
        Err(e) => die(&e),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // --config PATH before or after the command (generated scripts and the
    // uinitconverter front end put it after), up to a "--".
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
    if matches!(cmd.as_str(), "help" | "--help" | "-h")
        || args.iter().any(|a| a == "--help" || a == "-h")
    {
        print!("{USAGE}");
        exit(0);
    }
    if uinitconverter_version::wanted(&cmd) {
        uinitconverter_version::print("initconvertd");
        exit(0);
    }
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let code = match cmd.as_str() {
        "daemon" => serve(cfg, rest),
        o if o.starts_with("--") => serve(cfg, &args),
        other => die(&format!("unknown command {other} (see initconvertd help)")),
    };
    exit(code);
}
