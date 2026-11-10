//! `initconvert COMMAND`: the one command users and older scripts know. Each
//! command belongs to one of the uinitconverter tools; this execs that tool with
//! the same arguments. Generated databases from before the split call
//! `/usr/libexec/uinitconverter/uinitconverter run|finish|...`, which still works.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command};

mod early;
mod init;
mod rc;
mod svc;

const USAGE: &str = "\
usage: initconvert [--config PATH] COMMAND [ARGS]
       initconvert --version     this build and the installed tools' builds

  initconvert init [OPTIONS]  set up booting with s6 (see initconvert init --help)

Each other command is handled by one uinitconverter tool (in /usr/libexec/uinitconverter):

  initconvert-gen        reload, plan, enable, disable, mask, unmask, preset, is-enabled
  initconvertd           daemon, start, stop, restart, log
  initconvert-runner     run, finish, up, down, stop-exec, reload-exec
  initconvert-timers     timer-run, path-run
  initconvert-fdstore    fdstore           (standalone, optional)
  initconvert-journal-relay  journal-relay (standalone, optional)
  initconvert-notify     notify            (standalone, optional)
  initconvert-linkwatch  linkwatch         (standalone, optional)

Run `initconvert COMMAND --help` or see the tool's own usage for details.
";

/// The tool for a command, and whether it takes the command name as its
/// first argument (the standalone tools take only the options).
fn tool_for(cmd: &str) -> Option<(&'static str, bool)> {
    Some(match cmd {
        "reload" | "plan" | "enable" | "disable" | "mask" | "unmask" | "preset" | "is-enabled" => {
            ("initconvert-gen", true)
        }
        "daemon" | "start" | "stop" | "restart" | "log" => ("initconvertd", true),
        "run" | "finish" | "up" | "down" | "stop-exec" | "reload-exec" => {
            ("initconvert-runner", true)
        }
        "timer-run" | "path-run" => ("initconvert-timers", true),
        "fdstore" => ("initconvert-fdstore", false),
        "journal-relay" => ("initconvert-journal-relay", false),
        "notify" => ("initconvert-notify", false),
        "linkwatch" => ("initconvert-linkwatch", false),
        _ => return None,
    })
}

/// The directory of the tools: next to this program (so a build tree works
/// as well as /usr/libexec/uinitconverter), else /usr/libexec/uinitconverter.
fn tool_path(name: &str) -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        let p = dir.join(name);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("/usr/libexec/uinitconverter").join(name)
}

/// The core tools, whose builds should match this one.
const CORE_TOOLS: [&str; 4] = [
    "initconvert-gen",
    "initconvert-runner",
    "initconvert-timers",
    "initconvertd",
];
/// The standalone tools: separate projects with their own versions.
const STANDALONE: [&str; 4] = [
    "initconvert-fdstore",
    "initconvert-journal-relay",
    "initconvert-linkwatch",
    "initconvert-notify",
];

/// The lines of `initconvert --version`: this build, each core tool's (from
/// its own --version) and whether the standalone tools are installed; and
/// whether every core tool is installed and from this build.
fn version_report() -> (Vec<String>, bool) {
    let mut lines = vec![format!("initconvert {}", uinitconverter_version::VERSION)];
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
        if theirs == Some(uinitconverter_version::VERSION) {
            lines.push(format!("  {line}"));
        } else {
            ok = false;
            if theirs.is_some_and(|v| !v.starts_with('(') && !v.starts_with("not ")) {
                lines.push(format!("  {line}  (differs from initconvert)"));
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

/// `initconvert --version`. Exits 1 when a core tool is missing or from
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
    // The early-boot steps of the s6 services (`up`/`down` run
    // /usr/libexec/uinitconverter/early STEP, a link to this program).
    let argv0 = std::env::args().next().unwrap_or_default();
    if std::path::Path::new(&argv0)
        .file_name()
        .is_some_and(|n| n == "early")
    {
        early::main(&args);
    }
    if args.first().is_some_and(|a| a == "early") {
        early::main(&args[1..]);
    }
    // PID 1 with init=/usr/sbin/init-selinux (a link to this program).
    if std::path::Path::new(&argv0)
        .file_name()
        .is_some_and(|n| n == "init-selinux")
    {
        rc::init_selinux(&args);
    }
    // s6-linux-init's rc.init, rc.shutdown and rc.shutdown.final.
    if args.first().is_some_and(|a| a == "rc") {
        rc::main(&args[1..]);
    }
    // The hand-written s6 services' start (their run is `exec initconvert svc NAME`).
    if args.first().is_some_and(|a| a == "svc") {
        svc::main(&args[1..]);
    }
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
    if uinitconverter_version::wanted(cmd) {
        version();
    }
    if cmd == "init" {
        init::main(&args[i + 1..]);
    }
    let Some((tool, with_cmd)) = tool_for(cmd) else {
        eprintln!("initconvert: unknown command {cmd} (see initconvert help)");
        exit(1);
    };
    let path = tool_path(tool);
    let mut c = Command::new(&path);
    c.arg0(tool);
    if with_cmd {
        c.args(&args);
    } else {
        // Standalone tools read no uinitconverter.conf: drop --config.
        c.args(&args[i + 1..]);
    }
    let err = c.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        eprintln!("initconvert: {cmd}: {} is not installed", path.display());
    } else {
        eprintln!("initconvert: {}: {err}", path.display());
    }
    exit(127);
}
