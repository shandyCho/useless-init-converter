//! `sdcompat COMMAND`: the one command users and older scripts know. Each
//! command belongs to one of the sdcompat tools; this execs that tool with
//! the same arguments. Generated databases from before the split call
//! `/usr/libexec/sdcompat/sdcompat run|finish|...`, which still works.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command};

const USAGE: &str = "\
usage: sdcompat [--config PATH] COMMAND [ARGS]

Each command is handled by one sdcompat tool (in /usr/libexec/sdcompat):

  sdcompat-gen        reload, plan, enable, disable, mask, unmask, preset, is-enabled
  sdcompatd           daemon, start, stop, restart, log
  sdcompat-runner     run, finish, up, down, stop-exec, reload-exec
  sdcompat-timers     timer-run, path-run
  sdcompat-fdstore    fdstore           (standalone, optional)
  sdcompat-journal-relay  journal-relay (standalone, optional)
  sdcompat-notify     notify            (standalone, optional)

Run `sdcompat COMMAND --help` or see the tool's own usage for details.
";

/// The tool for a command, and whether it takes the command name as its
/// first argument (the standalone tools take only the options).
fn tool_for(cmd: &str) -> Option<(&'static str, bool)> {
    Some(match cmd {
        "reload" | "plan" | "enable" | "disable" | "mask" | "unmask" | "preset" | "is-enabled" => ("sdcompat-gen", true),
        "daemon" | "start" | "stop" | "restart" | "log" => ("sdcompatd", true),
        "run" | "finish" | "up" | "down" | "stop-exec" | "reload-exec" => ("sdcompat-runner", true),
        "timer-run" | "path-run" => ("sdcompat-timers", true),
        "fdstore" => ("sdcompat-fdstore", false),
        "journal-relay" => ("sdcompat-journal-relay", false),
        "notify" => ("sdcompat-notify", false),
        _ => return None,
    })
}

/// The directory of the tools: next to this program (so a build tree works
/// as well as /usr/libexec/sdcompat), else /usr/libexec/sdcompat.
fn tool_path(name: &str) -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let p = dir.join(name);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("/usr/libexec/sdcompat").join(name)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // --config PATH may come before the command; keep it for the tool.
    let mut i = 0;
    while args.get(i).map(|a| a == "--config").unwrap_or(false) {
        i += 2;
    }
    let Some(cmd) = args.get(i) else {
        eprint!("{USAGE}");
        exit(1);
    };
    if matches!(cmd.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        exit(0);
    }
    let Some((tool, with_cmd)) = tool_for(cmd) else {
        eprintln!("sdcompat: unknown command {cmd} (see sdcompat help)");
        exit(1);
    };
    let path = tool_path(tool);
    let mut c = Command::new(&path);
    c.arg0(tool);
    if with_cmd {
        c.args(&args);
    } else {
        // Standalone tools read no sdcompat.conf: drop --config.
        c.args(&args[i + 1..]);
    }
    let err = c.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        eprintln!("sdcompat: {cmd}: {} is not installed", path.display());
    } else {
        eprintln!("sdcompat: {}: {err}", path.display());
    }
    exit(127);
}
