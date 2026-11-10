//! s6-linux-init's stage 2 and shutdown scripts, and init-selinux, in Rust
//! (rule 6). The skel's rc.init, rc.shutdown and rc.shutdown.final are one
//! line of execline each, `initconvert rc init|shutdown|shutdown-final`;
//! /usr/sbin/init-selinux is a link to initconvert (argv0 dispatch).

use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{exit, Command};
use std::time::{Duration, Instant};

use crate::early::{cmdline, output, quiet, run, user_sessions};

const STATE: &str = "/var/lib/uinitconverter";
const LOG_DIR: &str = "/var/log/uinitconverter";
const SHUTDOWN_MARK: &str = "uinitconverter: shutdown starts";

fn kmsg(msg: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = f.write_all(msg.as_bytes());
    }
}

fn console(msg: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().write(true).open("/dev/console") {
        let _ = writeln!(f, "{msg}");
    }
}

fn exec(prog: &str, args: &[String]) -> ! {
    let e = Command::new(prog).args(args).exec();
    eprintln!("initconvert rc: can't run {prog}: {e}");
    exit(111)
}

/// Run PROG with a time limit, as `timeout -s KILL SECS`: its exit code,
/// 137 when it was killed at the limit, None when it couldn't start.
fn run_limited(secs: u64, prog: &str, args: &[&str]) -> Option<i32> {
    let mut c = Command::new(prog).args(args).spawn().ok()?;
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(Some(s)) = c.try_wait() {
            return Some(s.code().unwrap_or(128 + 9));
        }
        if Instant::now() >= end {
            let _ = c.kill();
            let _ = c.wait();
            return Some(137);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The database to boot: the boot link, or with uinitconverter.db=previous
/// the one installed before (if it exists).
fn boot_db(cmdline: &str) -> String {
    let previous = format!("{STATE}/compiled.previous");
    if cmdline
        .split_whitespace()
        .any(|w| w == "uinitconverter.db=previous")
        && Path::new(&previous).exists()
    {
        return previous;
    }
    format!("{STATE}/compiled")
}

/// Stage 2 (rc.init RUNLEVEL): the database to boot, s6-rc-init, then the
/// runlevel script. Kernel command line option: uinitconverter.db=previous
/// boots the database installed before the last one. Nothing is
/// regenerated here; unit files changed while the system was down (e.g.
/// from a systemd boot) are picked up by the next daemon-reload.
fn init(args: &[String]) -> ! {
    let mut rl = args.first().cloned().unwrap_or_else(|| "default".into());
    // The initramfs mounts / read-only; the database lives in /var/lib.
    quiet("mount", &["-o", "remount,rw", "/"]);
    // /var (or /var/lib) can be a filesystem of its own, such as Fedora
    // Cloud's btrfs "var" subvolume, which the initramfs leaves alone: mount
    // it from fstab before s6-rc-init reads the database. mount-fs's
    // `mount -a` skips it then.
    for m in ["/var", "/var/lib", STATE] {
        if quiet("findmnt", &["--fstab", "-n", m])
            && !quiet("mountpoint", &["-q", m])
            && run("mount", &[m]) != Some(0)
        {
            eprintln!("rc.init: mount {m} failed");
        }
    }
    // The database itself, not the link: a reload that installs a database
    // for the next boot only (it would restart an essential service)
    // switches the link, and the live state must keep the database it runs.
    let link = boot_db(&cmdline());
    let db = fs::canonicalize(&link)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(link);
    match run(
        "s6-rc-init",
        &["-c", &db, "-l", "/run/s6-rc", "/run/service"],
    ) {
        Some(0) => {}
        r => exit(r.unwrap_or(111)),
    }
    // Offline updates (dnf system-upgrade / dnf offline): like systemd's
    // system-update generator, boot into system-update.target instead.
    if fs::symlink_metadata("/system-update").is_ok()
        || fs::symlink_metadata("/etc/system-update").is_ok_and(|m| m.file_type().is_symlink())
    {
        eprintln!("rc.init: /system-update exists, booting the offline update");
        rl = "system-update".into();
    }
    exec("/etc/s6-linux-init/current/scripts/runlevel", &[rl])
}

/// Shutdown (rc.shutdown): every s6-rc service down, dependencies respected.
/// The marker stops initconvertd from starting units beside this change
/// (D-Bus activations during shutdown). The kernel log line marks where
/// this shutdown starts for shutdown-final.
/// s6-linux-init-shutdownd waits for this without a time limit, so a stop
/// that never finishes would keep the machine up for good. After 3 minutes
/// s6-rc gives up (exit 2) and shutdownd kills what is left, as systemd
/// ends a shutdown its unit stop timeouts couldn't. s6-rc starts its -t
/// clock only once it has the lock, and -b waits for the lock with no
/// limit: a change that never ends (the boot's, or a start initconvertd
/// began before the marker, of a unit with TimeoutStartSec=infinity) would
/// hold it for good. So the whole call is bounded (137: killed).
/// First initconvertd stops the on-demand instances (user@UID, ...), which
/// s6-rc doesn't know: they are ordered after s6-rc services such as logind
/// (stopped after it, each user@'s PAM close waited 45 s). Bounded too;
/// what it leaves, initconvertd stops again when s6-rc takes it down.
fn shutdown() -> ! {
    let _ = fs::create_dir_all("/run/uinitconverter");
    let _ = fs::write("/run/uinitconverter/shutting-down", "");
    kmsg(&format!("{SHUTDOWN_MARK}\n"));
    // No new unprivileged logins from here (systemd-user-sessions' stop).
    user_sessions(true);
    match run_limited(
        120,
        "/usr/libexec/uinitconverter/initconvertd",
        &["stop-instances"],
    ) {
        Some(0) => {}
        Some(137) => kmsg("uinitconverter: on-demand instances still not down after 120 s\n"),
        _ => kmsg("uinitconverter: initconvertd stop-instances failed\n"),
    }
    let rc = run_limited(180, "s6-rc", &["-v2", "-t", "180000", "-bDa", "change"]).unwrap_or(111);
    match rc {
        0 => {}
        2 | 137 => kmsg("uinitconverter: services still not down after 180 s, killing the rest\n"),
        r => kmsg(&format!("uinitconverter: s6-rc change exited {r}\n")),
    }
    exit(rc)
}

/// The kernel log from this shutdown's marker on; all of it without one.
fn from_marker(dmesg: &str) -> &str {
    dmesg
        .find(SHUTDOWN_MARK)
        .map(|i| &dmesg[dmesg[..i].rfind('\n').map(|n| n + 1).unwrap_or(0)..])
        .unwrap_or(dmesg)
}

/// After all processes are killed, right before the machine halts
/// (rc.shutdown.final): keep what this shutdown printed. s6's catch-all log
/// (copied to the console) is on tmpfs, and SELinux denials go only to the
/// kernel log once auditd is down. Root-only: they hold every service's
/// stderr. All archives, not only the newest: the kill also hits the
/// catch-all s6-log, and the restarted one archives the interrupted
/// "current" as @*.u and starts an empty one. The archives sort by time
/// (TAI64N names).
fn shutdown_final() -> ! {
    // SAFETY: umask takes a mode and can't fail.
    unsafe { libc::umask(0o077) };
    // s6-linux-init-umountall ran just before. A separate /var (or
    // /var/log) is unmounted by now: mount it again from fstab for the
    // two files.
    let mut mounted = None;
    for mp in ["/var/log", "/var"] {
        if quiet("findmnt", &["--fstab", "-n", mp]) && !quiet("mountpoint", &["-q", mp]) {
            if quiet("mount", &[mp]) {
                mounted = Some(mp);
            }
            break;
        }
    }
    // It can't unmount the root filesystem, so it remounted it read-only,
    // which works unless a process still has a file open for writing. Make
    // the filesystem writable for these two files and read-only again
    // afterwards. With more than 99 mount points it gives up at once ("too
    // many mount points") and leaves everything mounted read-write: finish
    // its job below.
    let writable = |p: &str| {
        let c = std::ffi::CString::new(p).unwrap();
        // SAFETY: a valid C string.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    };
    let umountall_failed = writable("/");
    let dir = Path::new(LOG_DIR);
    let mut remounted = None;
    if dir.is_dir() && !writable(LOG_DIR) {
        let mp = output("findmnt", &["-no", "TARGET", "-T", LOG_DIR])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/".into());
        if quiet("mount", &["-o", "remount,rw", &mp]) {
            remounted = Some(mp);
        }
    }
    if dir.is_dir() && !writable(LOG_DIR) {
        console(&format!(
            "uinitconverter: {LOG_DIR} is read-only, the shutdown logs are not kept"
        ));
    } else if dir.is_dir() {
        let mut names: Vec<_> = fs::read_dir("/run/uncaught-logs")
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with('@'))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names.push("current".into());
        let mut text = Vec::new();
        for n in names {
            if let Ok(b) = fs::read(Path::new("/run/uncaught-logs").join(n)) {
                text.extend(b);
            }
        }
        let _ = fs::write(dir.join("last-shutdown-console.log"), text);
        let dmesg = output("dmesg", &[]).unwrap_or_default();
        let _ = fs::write(dir.join("last-shutdown-kernel.log"), from_marker(&dmesg));
        // SAFETY: sync takes no arguments.
        unsafe { libc::sync() };
    }
    if let Some(mp) = remounted {
        quiet("mount", &["-o", "remount,ro", &mp]);
    }
    if let Some(mp) = mounted {
        quiet("umount", &[mp]);
    }
    if umountall_failed {
        // What s6-linux-init-umountall does: unmount everything, the busy
        // ones (the root) read-only. umount -a leaves proc, sysfs and devpts
        // alone.
        console("uinitconverter: filesystems still mounted read-write, unmounting them");
        // SAFETY: as above.
        unsafe { libc::sync() };
        quiet("umount", &["-a", "-r"]);
        quiet("mount", &["-o", "remount,ro", "/"]);
    }
    exit(0)
}

fn raise(res: libc::__rlimit_resource_t, soft: Option<u64>, hard: Option<u64>) -> bool {
    let mut l = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: a valid rlimit struct for get and set.
    unsafe {
        if libc::getrlimit(res, &mut l) != 0 {
            return false;
        }
        if let Some(h) = hard {
            l.rlim_max = h;
        }
        if let Some(s) = soft {
            l.rlim_cur = s;
        }
        libc::setrlimit(res, &l) == 0
    }
}

/// PID 1 with init=/usr/sbin/init-selinux: load the SELinux policy (systemd
/// normally does this as PID 1), then exec s6-linux-init so it starts in
/// init_t (its init is labelled init_exec_t).
pub fn init_selinux(args: &[String]) -> ! {
    if !quiet("mountpoint", &["-q", "/proc"]) {
        quiet("mount", &["-t", "proc", "proc", "/proc"]);
    }
    if !quiet("mountpoint", &["-q", "/sys"]) {
        quiet("mount", &["-t", "sysfs", "sysfs", "/sys"]);
    }
    // The kernel starts PID 1 at 1024:4096 and every service inherits it.
    // systemd gives services a hard limit of 524288 (soft stays 1024), and
    // programs like cups raise their own soft limit up to it. Not worth
    // failing a boot over.
    if !raise(libc::RLIMIT_NOFILE, None, Some(524288)) {
        eprintln!("init-selinux: can't raise the hard RLIMIT_NOFILE");
    }
    // The kernel starts PID 1 with no core dumps (soft 0) and every service
    // inherits it; systemd sets unlimited, and then systemd-coredump does
    // the limiting. Survives the exec into init_t only through the policy
    // module's rlimitinh rule.
    if !raise(libc::RLIMIT_CORE, Some(libc::RLIM_INFINITY), None) {
        eprintln!("init-selinux: can't raise the soft RLIMIT_CORE");
    }
    if run("/usr/sbin/load_policy", &["-i"]) != Some(0) {
        eprintln!("init-selinux: policy load failed");
        let enforcing = fs::read_to_string("/etc/selinux/config")
            .unwrap_or_default()
            .lines()
            .any(|l| l.starts_with("SELINUX=enforcing"));
        if enforcing {
            exec("/bin/sh", &[]);
        }
    }
    exec("/etc/s6-linux-init/current/bin/init", args)
}

const USAGE: &str = "usage: initconvert rc init RUNLEVEL | shutdown | shutdown-final";

pub fn main(args: &[String]) -> ! {
    match args.first().map(String::as_str).unwrap_or("") {
        "init" => init(&args[1..]),
        "shutdown" => shutdown(),
        "shutdown-final" => shutdown_final(),
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
    fn marker_cut() {
        let d = "[1] boot\n[9] uinitconverter: shutdown starts\n[10] x\n";
        assert_eq!(
            from_marker(d),
            "[9] uinitconverter: shutdown starts\n[10] x\n"
        );
        assert_eq!(from_marker("[1] a\n"), "[1] a\n");
    }

    #[test]
    fn previous_needs_the_link() {
        // No such link on the build machine: the boot link stays.
        assert_eq!(
            boot_db("ro uinitconverter.db=previous"),
            if Path::new(&format!("{STATE}/compiled.previous")).exists() {
                format!("{STATE}/compiled.previous")
            } else {
                format!("{STATE}/compiled")
            }
        );
        assert_eq!(boot_db("ro quiet"), format!("{STATE}/compiled"));
    }

    #[test]
    fn limited_run() {
        assert_eq!(run_limited(5, "true", &[]), Some(0));
        assert_eq!(run_limited(1, "sleep", &["5"]), Some(137));
        assert_eq!(run_limited(5, "/nonexistent", &[]), None);
    }
}
