//! What the hand-written s6 services in /etc/uinitconverter/s6 do before
//! their program runs (rule 6: their run scripts held this as shell; each
//! `run` is now `exec initconvert svc NAME`). Every path ends in an exec, so
//! s6-supervise keeps supervising the program itself; skarnet's tools stay
//! in the chain where they did (s6-ipcserver-socketbinder binds a socket,
//! s6-notifyoncheck reports udevd's readiness, s6-pause idles).

use std::ffi::CString;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{exit, Command};

use uinitconverter_base::config::Config;

const LIBEXEC: &str = "/usr/libexec/uinitconverter";
const CONF: &str = "/etc/uinitconverter/uinitconverter.conf";

fn say(name: &str, msg: &str) {
    eprintln!("{name}: {msg}");
}

/// exec PROG ARGS; only returns (exits 111) when it can't.
fn exec(prog: &str, args: &[&str]) -> ! {
    let e = Command::new(prog).args(args).exec();
    eprintln!("initconvert svc: can't run {prog}: {e}");
    exit(111)
}

fn runner_confine(unit: &str, cmd: &[&str]) -> ! {
    let mut a = vec!["confine", unit, "--"];
    a.extend_from_slice(cmd);
    exec(&format!("{LIBEXEC}/initconvert-runner"), &a)
}

fn has(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

fn restorecon(path: &str) {
    if has("restorecon") {
        let _ = Command::new("restorecon")
            .arg(path)
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// The last KEY= in uinitconverter.conf text: the leading word of letters
/// and dashes of its value, quotes dropped (as the run scripts' sed did).
fn conf_word(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(|v| {
            v.trim_start()
                .trim_start_matches('"')
                .chars()
                .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
                .collect::<String>()
        })
        .filter(|v| !v.is_empty())
}

fn cmdline_value(cmdline: &str, key: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .filter_map(|w| w.strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(str::to_string)
}

fn cmdline() -> String {
    fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

fn oom_score_adj(v: i32) {
    let _ = fs::write("/proc/self/oom_score_adj", v.to_string());
}

/// A try counter in RUN_DIR: how many starts in a row (within `window`
/// seconds of the last, if given) the service has had in this boot.
fn count_try(file: &str, window: Option<i64>) -> u32 {
    let _ = fs::create_dir_all("/run/uinitconverter");
    let up = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split('.').next().and_then(|n| n.parse::<i64>().ok()))
        .unwrap_or(0);
    let old = fs::read_to_string(file).unwrap_or_default();
    let mut w = old.split_whitespace();
    let mut n: u32 = w.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // Signed, as the shell's arithmetic was: a hand-made "N -100" counts.
    let last: i64 = w.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if window.is_some_and(|win| up - last > win) {
        n = 0;
    }
    n += 1;
    let _ = fs::write(
        file,
        match window {
            Some(_) => format!("{n} {up}\n"),
            None => format!("{n}\n"),
        },
    );
    n
}

/// The system bus. BUS= in uinitconverter.conf (initconvert init --bus=)
/// picks it, uinitconverter.bus=daemon|broker on the kernel command line
/// overrides that for one boot.
///  - daemon (default): the reference dbus-daemon; --systemd-activation makes
///    it ask uinitconverter to start services with SystemdService=.
///  - broker: dbus-broker, as on the systemd boot. dbus-broker-launch starts
///    services through StartUnit/StartTransientUnit on uinitconverter's
///    org.freedesktop.systemd1. If it can't run, this falls back to
///    dbus-daemon rather than boot without a bus.
fn dbus() -> ! {
    let conf = fs::read_to_string(CONF).unwrap_or_default();
    let bus = cmdline_value(&cmdline(), "uinitconverter.bus").or_else(|| conf_word(&conf, "BUS"));
    let _ = fs::create_dir_all("/run/dbus");
    let _ = fs::create_dir_all("/run/uinitconverter");
    // systemd creates it for dbus.socket with its file_contexts label
    // (system_dbusd_var_run_t); the socket inherits it, and confined services
    // may only connect to a socket with that label.
    restorecon("/run/dbus");
    if bus.as_deref() == Some("broker") {
        // A launcher that keeps exiting (s6 runs this again each time) gives
        // way to dbus-daemon after three tries in this boot.
        let n = count_try("/run/uinitconverter/dbus-broker-tries", None);
        let why = if !has("dbus-broker-launch") {
            Some("dbus-broker is not installed".to_string())
        } else if n > 3 {
            Some(format!(
                "dbus-broker-launch exited {} times in this boot",
                n - 1
            ))
        } else {
            // dbus-broker-launch exits at once without journald's socket
            // (journald, or journal-relay with JOURNAL=no).
            let s = Path::new("/run/systemd/journal/socket");
            let mut i = 0;
            while !s.exists() && i < 100 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                i += 1;
            }
            (!s.exists()).then(|| "no /run/systemd/journal/socket after 10 s".to_string())
        };
        match why {
            None => {
                // As dbus.socket + dbus-broker.service: the socket is bound
                // first (clients wait in its backlog, so the bus counts as
                // ready now) and passed as fd 3 with LISTEN_FDS; fd 3 was the
                // readiness pipe. OOMScoreAdjust= of dbus-broker.service;
                // confine applies the rest of it (cgroup, LimitNOFILE=,
                // ProtectSystem=, PrivateTmp=, PrivateDevices=). If that
                // fails, this exits and the tries above lead to dbus-daemon.
                let _ = fs::remove_file("/run/dbus/system_bus_socket");
                oom_score_adj(-900);
                runner_confine(
                    "dbus-broker.service",
                    &[
                        "s6-ipcserver-socketbinder",
                        "-a",
                        "0666",
                        "/run/dbus/system_bus_socket",
                        &format!("{LIBEXEC}/initconvert"),
                        "svc",
                        "listen-exec",
                        "--ready",
                        "--",
                        "dbus-broker-launch",
                        "--scope",
                        "system",
                        "--audit",
                    ],
                );
            }
            Some(why) => say(
                "dbus",
                &format!("dbus-daemon instead of dbus-broker: {why}"),
            ),
        }
    }
    exec(
        "dbus-daemon",
        &[
            "--system",
            "--nofork",
            "--nopidfile",
            "--systemd-activation",
            "--print-address=3",
        ],
    )
}

/// The syslog daemon that stands in for journald under s6 (JOURNAL=no).
/// SYSLOG= in uinitconverter.conf (initconvert init --syslog=) picks it.
/// Each uses its own configuration under /etc/uinitconverter, written by
/// initconvert init, so the systemd boot, where journald owns /dev/log, is
/// unaffected.
///  - rsyslog (default): rsyslogd binds /dev/log itself (imuxsock). It runs
///    with rsyslog.service's sandbox, umask and limits; if those can't be
///    applied it doesn't start, and s6 tries again.
///  - syslog-ng: with /run/systemd/system present (uinitconverter's
///    sd_booted() flag) syslog-ng refuses a plain /dev/log, so it is bound
///    here and passed as fd 3 with LISTEN_FDS, as systemd's syslog.socket
///    would (see /etc/uinitconverter/syslog-ng.conf).
fn syslog() -> ! {
    let conf = fs::read_to_string(CONF).unwrap_or_default();
    let syslog = conf_word(&conf, "SYSLOG");
    // syslog starts before tmpfiles-setup, and rsyslog.service's
    // ProtectHome= only covers a /run/user that already exists. Same mode as
    // tmpfiles' line; created from init_t it needs its label set.
    if !Path::new("/run/user").is_dir() {
        use std::os::unix::fs::DirBuilderExt;
        let _ = fs::DirBuilder::new().mode(0o755).create("/run/user");
        restorecon("/run/user");
    }
    let rsyslog = || -> ! {
        runner_confine(
            "rsyslog.service",
            &[
                "rsyslogd",
                "-n",
                "-iNONE",
                "-f",
                "/etc/uinitconverter/rsyslog.conf",
            ],
        )
    };
    match syslog.as_deref().unwrap_or("rsyslog") {
        "syslog-ng" => {
            let ng = ["-F", "-f", "/etc/uinitconverter/syslog-ng.conf"];
            if Path::new("/run/systemd/system").is_dir() {
                std::env::set_var("UINITCONVERTER_DEVLOG", "systemd-syslog()");
                let mut a = vec![
                    "-m",
                    "-b0",
                    "-a",
                    "0666",
                    "/dev/log",
                    "/usr/libexec/uinitconverter/initconvert",
                    "svc",
                    "listen-exec",
                    "--restorecon",
                    "/dev/log",
                    "--",
                    "syslog-ng",
                ];
                a.extend_from_slice(&ng);
                exec("s6-ipcserver-socketbinder", &a);
            }
            std::env::set_var("UINITCONVERTER_DEVLOG", "unix-dgram(\"/dev/log\")");
            exec("syslog-ng", &ng)
        }
        "rsyslog" => rsyslog(),
        other => {
            say(
                "syslog",
                &format!("SYSLOG={other} in uinitconverter.conf: expected rsyslog or syslog-ng; using rsyslog"),
            );
            rsyslog()
        }
    }
}

/// systemd-udevd runs fine without systemd as PID 1. s6-notifyoncheck execs
/// into it (so s6's stop signal reaches udevd itself) and signals readiness
/// once the control socket exists. It runs with systemd-udevd.service's
/// cgroup, sandbox and filters. The boot needs udevd, so three starts within
/// a minute of each other, or uinitconverter.udev=plain on the kernel command
/// line, run it without them. A later restart (a systemd update) starts
/// counting again.
fn udevd() -> ! {
    let mut plain = cmdline()
        .split_whitespace()
        .any(|w| w == "uinitconverter.udev=plain")
        .then(|| "uinitconverter.udev=plain".to_string());
    let n = count_try("/run/uinitconverter/udevd-tries", Some(60));
    if n > 3 {
        plain = Some(format!("udevd exited {} times in a row", n - 1));
    }
    // OOMScoreAdjust= of systemd-udevd.service; set here for both ways of starting.
    oom_score_adj(-1000);
    let cmd = [
        "s6-notifyoncheck",
        "-w",
        "50",
        "-n",
        "0",
        "-c",
        "test -S /run/udev/control",
        "/usr/lib/systemd/systemd-udevd",
    ];
    match plain {
        None => runner_confine("systemd-udevd.service", &cmd),
        Some(why) => {
            say(
                "udevd",
                &format!("without systemd-udevd.service's sandbox: {why}"),
            );
            exec(cmd[0], &cmd[1..])
        }
    }
}

/// Words of a shell value as `eval "set -- $VALUE"` gives them for what
/// /etc/default files hold: blanks separate, single and double quotes group
/// (dropped), a backslash escapes the next character outside single quotes.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars();
    let mut q: Option<char> = None;
    while let Some(c) = chars.next() {
        match (q, c) {
            (Some('\''), '\'') | (Some('"'), '"') => q = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    have = true;
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                q = Some(c);
                have = true;
            }
            (None, c) if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

/// earlyoom replaces systemd-oomd under s6 (same settings file as
/// earlyoom.service), with earlyoom.service's user, capabilities, sandbox,
/// Nice= and OOMScoreAdjust=. If they can't be applied it doesn't start, and
/// s6 tries again.
fn earlyoom() -> ! {
    let text = fs::read_to_string("/etc/default/earlyoom").unwrap_or_default();
    // EARLYOOM_ARGS is split with quote removal, as systemd does for
    // $EARLYOOM_ARGS: Fedora's default has quoted regexes with spaces
    // ('^(Web Content|Isolated Web Co)$').
    let raw = text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("EARLYOOM_ARGS="))
        .last()
        .unwrap_or("");
    // The assignment's own quotes, then the words inside it.
    let args = shell_words(&raw_inner(raw));
    let mut cmd = vec!["/usr/bin/earlyoom"];
    cmd.extend(args.iter().map(String::as_str));
    runner_confine("earlyoom.service", &cmd)
}

/// The text an assignment's value holds, its outer quotes removed but the
/// quotes inside kept (for `eval "set -- $VALUE"`): "a 'b c'" -> a 'b c'.
fn raw_inner(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Ready on fd 3, then idle: an optional standalone tool that isn't there.
fn ready_and_pause(name: &str, msg: &str, ready: bool) -> ! {
    say(name, msg);
    if ready {
        notify_ready();
    }
    exec("s6-pause", &[])
}

fn notify_ready() {
    // SAFETY: fd 3 is the notification pipe; a write of one byte.
    unsafe {
        libc::write(3, b"\n".as_ptr().cast(), 1);
    }
}

/// Without journald (JOURNAL=no): journald's client sockets in
/// /run/systemd/journal, forwarded to the syslog. dbus-broker-launch, the
/// session bus of systemd --user, refuses to start without them.
/// uinitconverter-journal-relay is a separate project, required by init for
/// JOURNAL=no; if it is missing anyway, ready and idle so the boot goes on.
fn journal_relay() -> ! {
    let bin = format!("{LIBEXEC}/initconvert-journal-relay");
    if !Path::new(&bin).exists() {
        ready_and_pause(
            "journal-relay",
            "initconvert-journal-relay is not installed: no journald sockets",
            true,
        );
    }
    exec(&bin, &["--ready-fd", "3"])
}

/// FDSTORE= fds and listening sockets held apart from initconvertd, so they
/// survive its restarts. uinitconverter-fdstore is a separate, optional
/// project: without it, ready and idle; initconvertd then holds the fds.
fn fdstore() -> ! {
    let bin = format!("{LIBEXEC}/initconvert-fdstore");
    if !Path::new(&bin).exists() {
        ready_and_pause(
            "uinitconverter-fdstore",
            "initconvert-fdstore is not installed; initconvertd holds the fds",
            true,
        );
    }
    // Where initconvertd and the runner look for it: RUN_DIR/fdstore.
    let run_dir = Config::load(None)
        .map(|c| c.run_dir)
        .unwrap_or_else(|_| "/run/uinitconverter".into());
    let sock = run_dir.join("fdstore");
    exec(
        &bin,
        &["--socket", &sock.to_string_lossy(), "--ready-fd", "3"],
    )
}

/// s6-rc-update (daemon-reload) and s6-svlink wait for a new supervisor's
/// start event; this unblocks them when it came before they listened (an s6
/// race: the wait has no end and holds s6-rc's lock).
/// initconvert-linkwatch is a separate, optional project: without it, idle.
fn linkwatch() -> ! {
    let bin = format!("{LIBEXEC}/initconvert-linkwatch");
    if !Path::new(&bin).exists() {
        ready_and_pause(
            "uinitconverter-linkwatch",
            "initconvert-linkwatch is not installed",
            false,
        );
    }
    exec(&bin, &[])
}

/// `svc listen-exec [--ready] [--restorecon PATH] -- PROG ARGS`, run by
/// s6-ipcserver-socketbinder with the bound socket on fd 0: report ready on
/// fd 3 (--ready), then hand the socket over as systemd's socket activation
/// does (fd 3, LISTEN_FDS=1, LISTEN_PID of this process, which PROG becomes)
/// with /dev/null as stdin.
fn listen_exec(args: &[String]) -> ! {
    let mut ready = false;
    let mut relabel = None;
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        match args[i].as_str() {
            "--ready" => ready = true,
            "--restorecon" => {
                i += 1;
                relabel = args.get(i).cloned();
            }
            a => {
                eprintln!("initconvert svc listen-exec: unknown option {a}");
                exit(100);
            }
        }
        i += 1;
    }
    let cmd = &args[(i + 1).min(args.len())..];
    let Some(prog) = cmd.first() else {
        eprintln!("usage: initconvert svc listen-exec [--ready] [--restorecon PATH] -- PROG ARGS");
        exit(100)
    };
    if let Some(p) = relabel {
        restorecon(&p);
    }
    if ready {
        notify_ready();
    }
    let null = CString::new("/dev/null").unwrap();
    // SAFETY: plain fd calls on this process's own descriptors.
    unsafe {
        if libc::dup2(0, 3) < 0 {
            eprintln!(
                "initconvert svc listen-exec: dup2: {}",
                std::io::Error::last_os_error()
            );
            exit(111);
        }
        let n = libc::open(null.as_ptr(), libc::O_RDONLY);
        if n >= 0 && n != 0 {
            libc::dup2(n, 0);
            libc::close(n);
        }
    }
    std::env::set_var("LISTEN_FDS", "1");
    std::env::set_var("LISTEN_PID", std::process::id().to_string());
    let rest: Vec<&str> = cmd[1..].iter().map(String::as_str).collect();
    exec(prog, &rest)
}

const USAGE: &str =
    "usage: initconvert svc dbus|syslog|udevd|earlyoom|journal-relay|fdstore|linkwatch|listen-exec";

pub fn main(args: &[String]) -> ! {
    match args.first().map(String::as_str).unwrap_or("") {
        "dbus" => dbus(),
        "syslog" => syslog(),
        "udevd" => udevd(),
        "earlyoom" => earlyoom(),
        "journal-relay" => journal_relay(),
        "fdstore" => fdstore(),
        "linkwatch" => linkwatch(),
        "listen-exec" => listen_exec(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            exit(100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_words() {
        let c = "BUS=daemon\n  BUS= broker # x\nBUS_ADDRESS=y\nSYSLOG=\"syslog-ng\"\n";
        assert_eq!(conf_word(c, "BUS").as_deref(), Some("broker"));
        assert_eq!(conf_word(c, "SYSLOG").as_deref(), Some("syslog-ng"));
        assert_eq!(conf_word(c, "JOURNAL"), None);
        assert_eq!(
            cmdline_value("ro uinitconverter.bus=broker quiet", "uinitconverter.bus").as_deref(),
            Some("broker")
        );
    }

    #[test]
    fn earlyoom_args_as_the_shell_splits_them() {
        // Fedora's /etc/default/earlyoom.
        let line = r#"EARLYOOM_ARGS="-r 0 -m 4 -M 409600 --prefer '^(Web Content|Isolated Web Co)$' --avoid '^(dnf|rpm-ostree)$'""#;
        let raw = line.strip_prefix("EARLYOOM_ARGS=").unwrap();
        assert_eq!(
            shell_words(&raw_inner(raw)),
            [
                "-r",
                "0",
                "-m",
                "4",
                "-M",
                "409600",
                "--prefer",
                "^(Web Content|Isolated Web Co)$",
                "--avoid",
                "^(dnf|rpm-ostree)$"
            ]
        );
        assert_eq!(shell_words(r#"a\ b "c\"d" ''"#), ["a b", "c\"d", ""]);
    }
}
