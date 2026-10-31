//! How a child process ended, and systemd's names and exit codes for it.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Status {
    Exited(i32),
    Killed(i32),
}

impl Status {
    pub(super) fn from_wait(s: i32) -> Status {
        if libc::WIFSIGNALED(s) {
            Status::Killed(libc::WTERMSIG(s))
        } else {
            Status::Exited(libc::WEXITSTATUS(s))
        }
    }
    pub(super) fn ok(&self) -> bool {
        *self == Status::Exited(0)
    }
}

pub(super) fn wait_pid(pid: i32) -> i32 {
    let mut status = 0;
    loop {
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return status;
        }
        if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return 0;
        }
    }
}

/// SuccessExitStatus= and friends: numbers, exit status names (systemd's
/// exit_status_from_string(): DATAERR, TEMPFAIL, ...) and signal names.
pub(super) fn status_in(list: &[String], status: Status) -> bool {
    list.iter().any(|w| match status {
        Status::Exited(c) => w.parse::<i32>().ok().or_else(|| exit_status_number(w)) == Some(c),
        Status::Killed(s) => {
            w.parse::<i32>().is_err()
                && exit_status_number(w).is_none()
                && util::signal_number(w) == Some(s)
        }
    })
}

pub(super) fn exit_status_number(name: &str) -> Option<i32> {
    const NAMES: &[(&str, i32)] = &[
        ("SUCCESS", 0),
        ("FAILURE", 1),
        ("INVALIDARGUMENT", 2),
        ("NOTIMPLEMENTED", 3),
        ("NOPERMISSION", 4),
        ("NOTINSTALLED", 5),
        ("NOTCONFIGURED", 6),
        ("NOTRUNNING", 7),
        ("USAGE", 64),
        ("DATAERR", 65),
        ("NOINPUT", 66),
        ("NOUSER", 67),
        ("NOHOST", 68),
        ("UNAVAILABLE", 69),
        ("SOFTWARE", 70),
        ("OSERR", 71),
        ("OSFILE", 72),
        ("CANTCREAT", 73),
        ("IOERR", 74),
        ("TEMPFAIL", 75),
        ("PROTOCOL", 76),
        ("NOPERM", 77),
        ("CONFIG", 78),
        ("CHDIR", 200),
        ("NICE", 201),
        ("FDS", 202),
        ("EXEC", 203),
        ("MEMORY", 204),
        ("LIMITS", 205),
        ("OOM_ADJUST", 206),
        ("SIGNAL_MASK", 207),
        ("STDIN", 208),
        ("STDOUT", 209),
        ("CHROOT", 210),
        ("IOPRIO", 211),
        ("TIMERSLACK", 212),
        ("SECUREBITS", 213),
        ("SETSCHEDULER", 214),
        ("CPUAFFINITY", 215),
        ("GROUP", 216),
        ("USER", 217),
        ("CAPABILITIES", 218),
        ("CGROUP", 219),
        ("SETSID", 220),
        ("CONFIRM", 221),
        ("STDERR", 222),
        ("PAM", 224),
        ("NETWORK", 225),
        ("NAMESPACE", 226),
        ("NO_NEW_PRIVILEGES", 227),
        ("SECCOMP", 228),
        ("SELINUX_CONTEXT", 229),
        ("PERSONALITY", 230),
        ("APPARMOR_PROFILE", 231),
        ("ADDRESS_FAMILIES", 232),
        ("RUNTIME_DIRECTORY", 233),
        ("CHOWN", 235),
        ("SMACK_PROCESS_LABEL", 236),
        ("KEYRING", 237),
        ("STATE_DIRECTORY", 238),
        ("CACHE_DIRECTORY", 239),
        ("LOGS_DIRECTORY", 240),
        ("CONFIGURATION_DIRECTORY", 241),
        ("NUMA_POLICY", 242),
        ("CREDENTIALS", 243),
        ("BPF", 245),
    ];
    NAMES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

pub(super) fn describe(st: Status) -> String {
    match st {
        Status::Exited(c) => format!("exit status {c}"),
        Status::Killed(s) => format!("killed by SIG{}", util::signal_name(s)),
    }
}

pub(super) fn result_for(st: Status) -> &'static str {
    match st {
        Status::Exited(_) => "exit-code",
        Status::Killed(_) => "signal",
    }
}

pub(super) fn exit_code_for(st: Status) -> i32 {
    match st {
        Status::Exited(c) => c.max(1),
        Status::Killed(s) => 128 + s,
    }
}
