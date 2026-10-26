//! `sdcompat COMMAND`: the one command users and older scripts know. Each
//! command belongs to one of the sdcompat tools; this execs that tool with
//! the same arguments. Generated databases from before the split call
//! `/usr/libexec/sdcompat/sdcompat run|finish|...`, which still works.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command};

mod init;

const USAGE: &str = "\
usage: sdcompat [--config PATH] COMMAND [ARGS]
       sdcompat --version     this build and the installed tools' builds

  sdcompat init [OPTIONS]  set up booting with s6 (see sdcompat init --help)

Each other command is handled by one sdcompat tool (in /usr/libexec/sdcompat):

  sdcompat-gen        reload, plan, enable, disable, mask, unmask, preset, is-enabled
  sdcompatd           daemon, start, stop, restart, log
  sdcompat-runner     run, finish, up, down, stop-exec, reload-exec
  sdcompat-timers     timer-run, path-run
  sdcompat-fdstore    fdstore           (standalone, optional)
  sdcompat-journal-relay  journal-relay (standalone, optional)
  sdcompat-notify     notify            (standalone, optional)
  sdcompat-linkwatch  linkwatch         (standalone, optional)

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
        "linkwatch" => ("sdcompat-linkwatch", false),
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

/// The core tools, whose builds should match this one.
const CORE_TOOLS: [&str; 4] = ["sdcompat-gen", "sdcompat-runner", "sdcompat-timers", "sdcompatd"];
/// The standalone tools: separate projects with their own versions.
const STANDALONE: [&str; 4] = ["sdcompat-fdstore", "sdcompat-journal-relay", "sdcompat-linkwatch", "sdcompat-notify"];

/// The lines of `sdcompat --version`: this build, each core tool's (from
/// its own --version) and whether the standalone tools are installed; and
/// whether every core tool is installed and from this build.
fn version_report() -> (Vec<String>, bool) {
    let mut lines = vec![format!("sdcompat {}", sdcompat_version::VERSION)];
    let mut ok = true;
    for tool in CORE_TOOLS {
        let path = tool_path(tool);
        let out = Command::new(&path).arg("--version").output();
        let line = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Ok(_) => format!("{tool} (no --version: older than this build)"),
            Err(_) => format!("{tool} not installed ({})", path.display()),
        };
        let theirs = line.strip_prefix(tool).map(str::trim);
        if theirs == Some(sdcompat_version::VERSION) {
            lines.push(format!("  {line}"));
        } else {
            ok = false;
            if theirs.is_some_and(|v| !v.starts_with('(') && !v.starts_with("not ")) {
                lines.push(format!("  {line}  (differs from sdcompat)"));
            } else {
                lines.push(format!("  {line}"));
            }
        }
    }
    for tool in STANDALONE {
        let path = tool_path(tool);
        if path.is_file() {
            lines.push(format!("  {tool} installed (standalone)"));
        } else {
            lines.push(format!("  {tool} not installed (standalone, optional)"));
        }
    }
    (lines, ok)
}

/// `sdcompat --version`. Exits 1 when a core tool is missing or from
/// another build.
fn version() -> ! {
    let (lines, ok) = version_report();
    for l in lines {
        println!("{l}");
    }
    exit(if ok { 0 } else { 1 })
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
    if sdcompat_version::wanted(cmd) {
        version();
    }
    if cmd == "init" {
        init::main(&args[i + 1..]);
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
