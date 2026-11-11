//! The files of an s6 service directory that run a unit: what gen writes
//! into the s6-rc source and what the daemon writes for an on-demand
//! instance are the same files, so they are made here. The scripts are
//! execline (rule 1), one command line each into the runner or the timers.

use std::path::Path;

use crate::config::{runner_cmd, timers_cmd, Config};
use crate::index::unit_arg;
use crate::loader::Unit;
use crate::{tools, util};

/// What s6 needs of a unit's timeouts (milliseconds, 0 = none) and stop
/// signal; gen's database and the daemon's dynamic instances both use it.
#[derive(Debug, Clone, Default)]
pub struct Timings {
    /// s6-rc's timeout-up: only a backstop, the runner enforces
    /// TimeoutStartSec= itself (soft failure). 32-bit for s6-rc-compile.
    pub up_ms: u64,
    pub down_ms: u64,
    pub finish_ms: u64,
    pub down_signal: Option<String>,
    /// s6-supervise's max-death-tally, when StartLimitBurst= is beyond the
    /// 100 deaths it records by default (the runner counts them, see
    /// supervisor::recent_deaths).
    pub max_death_tally: Option<u32>,
}

impl Timings {
    pub fn of(unit: &Unit) -> Timings {
        let start = unit.start_timeout_usec();
        let restart_sec = unit
            .get("Service", "RestartSec")
            .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
            .unwrap_or(100_000);
        let down_signal = unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .filter(|&s| s != libc::SIGTERM)
            // s6-supervise reads down-signal with skalibs' sig0_scan(), which
            // takes a number too (SIGRTMIN+n has no name there).
            .map(|s| match util::signal_name(s) {
                n if n.parse::<i32>().is_ok() => n,
                n => format!("SIG{n}"),
            });
        // Type=notify: none. EXTEND_TIMEOUT_USEC= can push the start past
        // any fixed backstop (s6-rc would then bring the service down), and
        // the notify helper enforces the timeout, extended or not, and
        // releases s6-rc either way.
        let notify = matches!(
            unit.get("Service", "Type").as_deref(),
            Some("notify" | "notify-reload")
        );
        Timings {
            up_ms: if start == u64::MAX || notify {
                0
            } else {
                (ms(start) + 15_000).min(u32::MAX as u64)
            },
            down_ms: ms(unit.stop_timeout_usec()).min(u32::MAX as u64),
            finish_ms: ms(restart_sec).saturating_add(5000),
            down_signal,
            max_death_tally: unit
                .get("Unit", "StartLimitBurst")
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|&b| b >= 100)
                .map(|b| b.saturating_add(1).min(4096)),
        }
    }
}

fn ms(usec: u64) -> u64 {
    if usec == u64::MAX {
        0
    } else {
        usec / 1000
    }
}

/// A logger's run script: s6-log into LOG_DIR/ID (`notify`: readiness on fd
/// 3, s6-rc's loggers).
pub fn logger_run(log_dir: &Path, id: &str, notify: bool) -> String {
    let logdir = util::el_quote(&log_dir.join(id).display().to_string());
    let s6log = util::el_quote(&tools::path("s6-log"));
    let d3 = if notify { " -d3" } else { "" };
    format!(
        "{}if {{ mkdir -p {logdir} }}\n{s6log}{d3} n10 s1000000 T {logdir}\n",
        util::EXECLINE
    )
}

/// A script that passes its arguments on to `words`: s6 runs `finish` with
/// the exit code, the signal, the service name and the process group. In
/// execline, unless a word holds a `$` (execline would substitute it):
/// then, and only then, a shell script.
pub fn passing_args(words: &[&str]) -> String {
    if words.iter().all(|w| util::el_plain(w)) {
        let w: Vec<String> = words.iter().map(|w| util::el_quote(w)).collect();
        format!("{}{} $@\n", util::EXECLINE_ARGS, w.join(" "))
    } else {
        let w: Vec<String> = words.iter().map(|w| util::sh_quote(w)).collect();
        format!("#!/bin/sh\nexec {} \"$@\"\n", w.join(" "))
    }
}

/// `words` as one execline command line (an s6-rc oneshot's `up`/`down`).
pub fn command_line(words: &[&str]) -> String {
    let w: Vec<String> = words.iter().map(|w| util::el_quote(w)).collect();
    format!("{}\n", w.join(" "))
}

/// A script that runs `words`.
pub fn script(words: &[&str]) -> String {
    format!("{}{}", util::EXECLINE, command_line(words))
}

/// The words of a tool's command line for a unit: the tool, its command,
/// `--config PATH` if needed, the command's options, then the unit
/// (index::unit_arg's two words).
pub fn tool_words(cfg: &Config, tool: &Path, cmd: &str, opts: &[&str], id: &str) -> Vec<String> {
    let mut w = vec![tool.display().to_string(), cmd.to_string()];
    if let Some(path) = cfg.runner_args().strip_prefix(" --config ") {
        w.push("--config".into());
        w.push(path.into());
    }
    w.extend(opts.iter().map(|o| o.to_string()));
    w.extend(unit_arg(id).split(' ').map(String::from));
    w
}

pub fn refs(w: &[String]) -> Vec<&str> {
    w.iter().map(String::as_str).collect()
}

/// The files of a longrun's directory that s6-rc's source format and a
/// plain s6 service directory share: (name, contents, executable).
/// `on_demand`: the runner binds the sockets and waits for activity before
/// the first start (see Planned::on_demand).
pub fn longrun_files(
    cfg: &Config,
    id: &str,
    t: &Timings,
    on_demand: bool,
) -> Vec<(&'static str, String, bool)> {
    let words = |tool: &Path, cmd: &str| tool_words(cfg, tool, cmd, &[], id);
    if id.ends_with(".timer") || id.ends_with(".path") {
        let sub = if id.ends_with(".timer") {
            timers_cmd::TIMER_RUN
        } else {
            timers_cmd::PATH_RUN
        };
        return vec![
            ("run", script(&refs(&words(&cfg.timer_runner, sub))), true),
            ("notification-fd", "3\n".into(), false),
        ];
    }
    // Accept=yes: s6-ipcserverd on the bound socket, one connection
    // instance per client. SIGTERM stops accepting; running connections
    // go on, as systemd's instances are units of their own.
    if id.ends_with(".socket") {
        return vec![
            (
                "run",
                script(&refs(&words(&cfg.runner, runner_cmd::ACCEPT))),
                true,
            ),
            (
                "finish",
                passing_args(&refs(&words(&cfg.runner, runner_cmd::ACCEPT_FINISH))),
                true,
            ),
            ("notification-fd", "3\n".into(), false),
        ];
    }
    let lazy: &[&str] = if on_demand { &["--on-demand"] } else { &[] };
    let run_words = tool_words(cfg, &cfg.runner, runner_cmd::RUN, lazy, id);
    let mut out = vec![
        ("run", script(&refs(&run_words)), true),
        (
            "finish",
            passing_args(&refs(&words(&cfg.runner, runner_cmd::FINISH))),
            true,
        ),
        ("notification-fd", "3\n".into(), false),
        ("timeout-up", format!("{}\n", t.up_ms), false),
        ("timeout-down", format!("{}\n", t.down_ms), false),
        ("timeout-kill", format!("{}\n", t.down_ms), false),
        ("timeout-finish", format!("{}\n", t.finish_ms), false),
    ];
    if let Some(n) = t.max_death_tally {
        out.push(("max-death-tally", format!("{n}\n"), false));
    }
    if let Some(sig) = &t.down_signal {
        out.push(("down-signal", format!("{sig}\n"), false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated scripts are execline; only a `$` in a word of a script
    /// that passes its arguments on makes it a shell script.
    #[test]
    fn generated_scripts_are_execline() {
        assert_eq!(
            script(&["/usr/libexec/x/runner", "run", "--", "a.service"]),
            "#!/usr/bin/execlineb -P\n/usr/libexec/x/runner run -- a.service\n"
        );
        assert_eq!(
            command_line(&["/opt/my tools/runner", "up", "--config", "/etc/a\"b"]),
            "\"/opt/my tools/runner\" up --config \"/etc/a\\\"b\"\n"
        );
        assert_eq!(
            passing_args(&["/r", "finish", "--", "a.service"]),
            "#!/usr/bin/execlineb -S0\n/r finish -- a.service $@\n"
        );
        assert_eq!(
            passing_args(&["/opt/$x/r", "finish"]),
            "#!/bin/sh\nexec '/opt/$x/r' finish \"$@\"\n"
        );
    }
}
