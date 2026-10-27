//! The s6 part of `uinitconverter init`: s6-linux-init's tree, /etc/s6.conf
//! for the s6 command, the uinitconverter-sysinit members, the s6 runtime
//! labels, and restarting uinitconverterd on a running s6 boot.

use super::*;

pub(super) const S6LI_BASE: &str = "/etc/s6-linux-init/current";

pub(super) const S6_CONF: &str = "/etc/s6.conf";

/// In the s6.conf we write: one with it is ours to replace.
pub(super) const S6_CONF_MARK: &str = "written by uinitconverter's";

pub(super) const SYSINIT_CONTENTS: &str = "/etc/uinitconverter/s6/uinitconverter-sysinit/contents.d";

/// Take the s6 service NAME out of, or put it into, the uinitconverter-sysinit bundle.
pub(super) fn sysinit_member(ctx: &Ctx, name: &str, member: bool) {
    let f = format!("{SYSINIT_CONTENTS}/{name}");
    if !member && exists(&f) {
        ctx.run(&["rm", "-f", &f]);
    } else if member && !exists(&f) {
        ctx.run(&["touch", &f]);
    }
}

pub(super) fn setup_s6_linux_init(ctx: &Ctx) {
    say(&format!("Generating s6-linux-init in {S6LI_BASE}"));
    let tmp = if ctx.dry {
        "/tmp/uinitconverter-s6li".to_string()
    } else {
        output(&["mktemp", "-d"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| die("mktemp -d failed"))
    };
    let current = format!("{tmp}/current");
    let skel = format!("{SHARE}/s6-linux-init/skel");
    // -n: keep the initramfs's /run (remount it) as systemd does. Without
    // it s6-linux-init's umount fails (credential mounts under it) and a
    // second tmpfs hides the udev database, /run/log/journal, utab, utmp.
    #[rustfmt::skip]
    ctx.run(&[
        "s6-linux-init-maker", "-n", "-c", S6LI_BASE, "-f", &skel,
        "-p", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin",
        "-1", "-t", "2", "-D", "default", "-G", "/usr/sbin/agetty tty9 linux", &current,
    ]);
    if exists(S6LI_BASE) {
        let dest = format!("{}{S6LI_BASE}", ctx.backup);
        ctx.run(&["mkdir", "-p", &format!("{}/etc/s6-linux-init", ctx.backup)]);
        ctx.run(&["mv", S6LI_BASE, &dest]);
    }
    ctx.run(&["mkdir", "-p", "/etc/s6-linux-init"]);
    ctx.run(&["cp", "-a", &current, S6LI_BASE]);
    if !ctx.dry {
        let _ = fs::remove_dir_all(&tmp);
    }
    println!("  rescue: an early getty runs on tty9 (Ctrl+Alt+F9) even if stage 2 fails");
}

/// The installed programs that dist/selinux/uinitconverter.cil labels init_exec_t.
pub(super) fn skarnet_files() -> Vec<String> {
    let mut v = list_dir("/usr/bin", |n| n.starts_with("s6-") || n == "ucspilogd" || n == "execline");
    for d in ["/usr/libexec/execline", "/usr/libexec/s6", "/usr/libexec/s6-rc"] {
        if Path::new(d).is_dir() {
            v.push(d.to_string());
        }
    }
    v
}

pub(super) fn svok(ctx: &Ctx, dir: &str) -> bool {
    Path::new(dir).is_dir() && (ctx.dry || succeeds(&["s6-svok", dir]))
}

/// On the s6 boot, restart uinitconverterd so the installed build takes over
/// without a reboot. The daemon keeps dynamic units, the fd store and device
/// state across a restart. uinitconverter-fdstore and journal-relay are left alone:
/// restarting them would drop held fds and log sockets. uinitconverter-linkwatch
/// keeps no state: it is restarted too.
pub(super) fn restart_daemon(ctx: &Ctx) {
    let servicedir = |name: &str| ctx.cfg.live_dir.join("servicedirs").join(name).display().to_string();
    let lw = servicedir("uinitconverter-linkwatch");
    let lw = lw.as_str();
    if svok(ctx, lw) {
        ctx.run(&["s6-svc", "-r", lw]);
    }
    let svc = servicedir(&ctx.cfg.daemon_service);
    let svc = svc.as_str();
    if !svok(ctx, svc) {
        return;
    }
    say("Restarting uinitconverterd (running on the s6 boot)");
    if !ctx.try_run(&["s6-svc", "-wR", "-T", "30000", "-r", svc]) {
        warn(&format!("uinitconverterd was not ready within 30 s; check s6-svstat {svc}"));
    }
}

/// s6.conf for the s6 command, the admin's store, and which early services
/// are members of uinitconverter-sysinit for these settings.
pub(super) fn configure(ctx: &Ctx, s: &Settings) {
    // The s6 command: uinitconverter's repository stores and boot database.
    // A copy written by an earlier run is replaced; yours is kept.
    let ours = read(&format!("{SHARE}/s6.conf")).unwrap_or_default();
    match read(S6_CONF) {
        Some(cur) if !cur.contains(S6_CONF_MARK) => {
            println!("  keeping your {S6_CONF}; it needs these lines (see {SHARE}/s6.conf):");
            for l in ours.lines().filter(|l| ["repodir ", "bootdb ", "storelist "].iter().any(|k| l.starts_with(k))) {
                println!("    {l}");
            }
        }
        Some(cur) if cur == ours => {}
        _ => ctx.put(S6_CONF, &ours, 0o644),
    }
    // The admin's own s6-rc services: the last store, so they override.
    if let Some(own) = ctx.cfg.extra_source.last() {
        ctx.run(&["mkdir", "-p", &own.display().to_string()]);
    }
    // The boot bundle "default" is made by s6-rc-set-commit from the
    // repository set; s6-rc refuses a store that defines one.
    if exists("/etc/uinitconverter/s6/default") {
        println!("  removing /etc/uinitconverter/s6/default (the set's commit makes it now)");
        ctx.backup("/etc/uinitconverter/s6/default");
        ctx.run(&["rm", "-rf", "/etc/uinitconverter/s6/default"]);
    }
    // With journald, it owns /dev/log; the s6-only rsyslog is not started.
    // Without it, journal-relay serves journald's client sockets instead.
    sysinit_member(ctx, "syslog", !s.journal);
    sysinit_member(ctx, "journal-relay", !s.journal);
    // Without resolved, "resolv" hands DNS to NetworkManager at boot.
    sysinit_member(ctx, "resolv", !s.resolved);
}

/// The s6-rc repository and, on a running s6 boot, the runtime state:
/// labels uinitconverterd (init_t) and the services need.
pub(super) fn relabel_runtime(ctx: &Ctx) {
    // The s6-rc repository, which uinitconverterd (init_t) syncs and commits; on a
    // running s6 boot the runtime state too.
    let repo_parent = ctx.cfg.repo_dir.parent().unwrap_or(&ctx.cfg.repo_dir).display().to_string();
    for d in [ctx.cfg.run_dir.display().to_string(), repo_parent, "/run/uncaught-logs".into()] {
        if Path::new(&d).is_dir() {
            ctx.run(&["restorecon", "-R", &d]);
        }
    }
    // ... lets the services write their stderr to the catch-all logger's fifo.
    let fifo = "/run/service/s6-svscan-log/fifo";
    if exists(fifo) {
        ctx.run(&["restorecon", fifo]);
    }
    // ... lets s6-supervise notify waiters started from a shell (s6-svc -w).
    // (The s6-rc services' are in the live directory, relabelled below.)
    let events: Vec<String> = list_dir("/run/service", |_| true)
        .into_iter()
        .filter(|d| !Path::new(d).is_symlink())
        .map(|d| format!("{d}/event"))
        .filter(|e| Path::new(e).is_dir())
        .collect();
    if !events.is_empty() {
        let mut argv = vec!["restorecon"];
        argv.extend(events.iter().map(String::as_str));
        ctx.run(&argv);
    }
    // ... and lets s6-rc-update (daemon-reload) rename in the live directory.
    // LIVE_DIR is a symlink s6-rc-update points at LIVE_DIR:XXXX.
    let live_dir = &ctx.cfg.live_dir;
    let live_name = live_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let live_parent = live_dir.parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/".into());
    let mut live = list_dir(&live_parent, |n| n.starts_with(&format!("{live_name}:")));
    if live_dir.is_symlink() {
        live.push(live_dir.display().to_string());
    }
    if !live.is_empty() {
        let mut argv = vec!["restorecon", "-R"];
        argv.extend(live.iter().map(String::as_str));
        ctx.run(&argv);
    }
}

/// Remove the s6.conf init wrote and the s6-linux-init tree.
pub(super) fn uninstall(ctx: &Ctx) {
    if read(S6_CONF).is_some_and(|t| t.contains(S6_CONF_MARK)) {
        ctx.run(&["rm", "-f", S6_CONF]);
    }
    if exists(S6LI_BASE) {
        ctx.run(&["rm", "-rf", S6LI_BASE]);
    }
}
