//! `uinitconverter init`: set up an installed system to boot with s6-linux-init +
//! s6-rc + uinitconverter instead of systemd as PID 1.
//!
//! The packages (dnf, e.g. from a GitHub release) bring the programs and the files in
//! /usr; this writes the machine's own configuration: /etc/uinitconverter, the
//! s6-linux-init tree, the s6-rc database, SELinux labels and a GRUB entry
//! "Fedora (s6 + uinitconverter)". The systemd boot entry keeps working, and is
//! still the default unless --default-s6 is given. Nothing is installed:
//! a missing program is reported with the dnf command that brings it.

use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{exit, Command, Stdio};

use uinitconverter_base::config::{Config, DEFAULT_LOG_DIR, SYSLOGS};
use uinitconverter_base::tools;

mod boot;
mod checks;
mod options;
mod report;
mod s6;

use boot::*;
use checks::*;
use options::*;
use report::*;
use s6::S6LI_BASE;

const TITLE: &str = "Fedora (s6 + uinitconverter)";
const LIBEXEC: &str = "/usr/libexec/uinitconverter";
const SHARE: &str = "/usr/share/uinitconverter";
const BOOT_OPTIONS: &str = "/etc/uinitconverter/boot-options";
const BOOT_DEFAULT_FILE: &str = "/etc/uinitconverter/boot-default";
const BOOT_ONLY_S6_FILE: &str = "/etc/uinitconverter/boot-only-s6";
/// The kernel-install plugin, from the uinitconverter package.
const KI_PLUGIN: &str = "/usr/lib/kernel/install.d/90-uinitconverter.install";
/// The copy the old setup-fedora.sh put in /etc, which would hide the
/// packaged one (kernel-install prefers /etc).
const OLD_KI_PLUGIN: &str = "/etc/kernel/install.d/90-uinitconverter.install";
const BLS_DIR: &str = "/boot/loader/entries";
const CORE_PACKAGES: &str =
    "uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd";
/// Where the packages Fedora does not have come from.
const RELEASES: &str = "https://github.com/shandyCho/useless-init-converter/releases";
/// The packages init may ask for that Fedora does not have: built with
/// dist/rpm/build-skarnet-rpms.sh, or in a release.
const SKARNET_PACKAGES: [&str; 2] = ["s6-linux-init", "mdevd"];
const RELEASE_PACKAGES: [&str; 1] = ["uinitconverter-journal-relay"];

const USAGE: &str = "\
usage: uinitconverter init [OPTIONS]
       uinitconverter init --uninstall [--yes] [--dry-run]

Set up this Fedora system to boot with s6-linux-init + s6-rc + uinitconverter.
Install the packages first: the skarnet ones built with
dist/rpm/build-skarnet-rpms.sh, then the RPMs of a release from
https://github.com/shandyCho/useless-init-converter/releases.
The systemd boot entry keeps working: init adds a GRUB entry
\"Fedora (s6 + uinitconverter)\". It shows its plan and asks before changing
anything.

  --dry-run             print every command, change nothing (no root needed)
  -y, --yes             do not ask
  --default-s6          make the s6 entry the default, also for new kernels
                        (the systemd entry stays in the menu)
  --default-systemd     make the systemd entry the default again
  --journal=yes|no      no: no systemd-journald, a syslog daemon + s6-log
                        directories instead (journalctl and the log lines in
                        systemctl status won't work; needs
                        uinitconverter-journal-relay)
  --syslog=rsyslog|syslog-ng
                        the syslog daemon with --journal=no (default rsyslog)
  --resolved=yes|no     no: no systemd-resolved on the s6 boot, NetworkManager
                        handles DNS itself (the systemd boot keeps resolved)
  --bus=daemon|broker   the s6 boot's system bus: dbus-daemon, or dbus-broker
                        as on the systemd boot (falls back to dbus-daemon if it
                        can't start; uinitconverter.bus=daemon on the kernel
                        command line overrides it for one boot)
  --selinux=enforcing|permissive
                        SELinux mode of the s6 entry (first run: permissive,
                        until you have checked for denials)
  --boot-only-s6=yes|no yes: the s6 entry boots uinitconverter's own initramfs, with
                        s6-svscan as its PID 1 and mdevd loading the drivers,
                        so no systemd runs at all (needs mdevd; a LUKS root is
                        unlocked by a typed passphrase, no TPM2/FIDO2)
  --grub=yes|no         no: do not add or update the boot entry
  --uninstall           remove the boot entry and what init wrote (not the
                        packages, /etc/uinitconverter or the logs)

An option left out keeps what the last run set (or the default above).
";

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn say(msg: &str) {
    println!("\x1b[1m==> {msg}\x1b[0m");
}

fn warn(msg: &str) {
    eprintln!("\x1b[33mwarning:\x1b[0m {msg}");
}

fn die(msg: &str) -> ! {
    eprintln!("\x1b[31merror:\x1b[0m {msg}");
    exit(1);
}

/// A program's stdout, or None if it can't run or fails.
fn output(argv: &[&str]) -> Option<String> {
    let out = Command::new(tools::path(argv[0]))
        .args(&argv[1..])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether a program exits 0 (output discarded).
fn succeeds(argv: &[&str]) -> bool {
    Command::new(tools::path(argv[0]))
        .args(&argv[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn in_path(prog: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/sbin:/usr/bin".into())
        .split(':')
        .chain(["/usr/sbin", "/usr/bin"])
        .any(|d| Path::new(d).join(prog).is_file())
}

fn rpm_installed(pkg: &str) -> bool {
    succeeds(&["rpm", "-q", pkg])
}

/// The entries of a directory whose names pass `keep`, sorted.
fn list_dir(dir: &str, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| keep(n))
        .map(|n| format!("{dir}/{n}"))
        .collect();
    v.sort();
    v
}

struct Ctx {
    /// uinitconverter.conf as it was before this run: its directories.
    cfg: Config,
    dry: bool,
    yes: bool,
    /// Where overwritten files go.
    backup: String,
}

impl Ctx {
    /// Run a command, or print it with --dry-run. A failure ends init.
    fn run(&self, argv: &[&str]) {
        if !self.try_run(argv) {
            die(&format!("failed: {}", argv.join(" ")));
        }
    }

    /// Run a command, or print it with --dry-run; whether it succeeded.
    fn try_run(&self, argv: &[&str]) -> bool {
        if self.dry {
            println!("  + {}", argv.join(" "));
            return true;
        }
        match Command::new(tools::path(argv[0])).args(&argv[1..]).status() {
            Ok(s) => s.success(),
            Err(e) => {
                warn(&format!("{}: {e}", argv[0]));
                false
            }
        }
    }

    /// Write a file (backing up an existing one), or show it with --dry-run.
    fn put(&self, dest: &str, content: &str, mode: u32) {
        if self.dry {
            println!("  + write {dest} (mode {mode:04o})");
            return;
        }
        self.backup(dest);
        if let Some(dir) = Path::new(dest).parent() {
            let _ = fs::create_dir_all(dir);
        }
        let r = fs::write(dest, content).and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dest, fs::Permissions::from_mode(mode))
        });
        if let Err(e) = r {
            die(&format!("{dest}: {e}"));
        }
    }

    /// Copy a file or tree to the backup directory before it changes.
    fn backup(&self, path: &str) {
        if !Path::new(path).exists() {
            return;
        }
        if self.dry {
            println!("  + backup {path}");
            return;
        }
        let dest = format!("{}{path}", self.backup);
        if let Some(dir) = Path::new(&dest).parent() {
            let _ = fs::create_dir_all(dir);
        }
        if !succeeds(&["cp", "-a", path, &dest]) {
            die(&format!("could not back up {path} to {dest}"));
        }
    }

    fn confirm(&self, question: &str) {
        if self.yes || self.dry {
            return;
        }
        print!("{question} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut ans = String::new();
        let _ = std::io::stdin().lock().read_line(&mut ans);
        if !ans.trim_start().starts_with(['y', 'Y']) {
            println!("aborted");
            exit(1);
        }
    }
}

// ---------------------------------------------------------------- checks

// ---------------------------------------------------------------- steps

fn configure_uinitconverter(ctx: &Ctx, s: &Settings) {
    say("Configuring /etc/uinitconverter");
    let conf_path = ctx.cfg.config_path.display().to_string();
    let conf_path = conf_path.as_str();
    match read(conf_path) {
        Some(mut text) => {
            let mut changed = false;
            for (key, val) in [("JOURNAL", s.journal), ("RESOLVED", s.resolved)] {
                if let Some(t) = set_conf_key(&text, key, val) {
                    println!("  {key}={} in {conf_path}", if val { "yes" } else { "no" });
                    text = t;
                    changed = true;
                }
            }
            if let Some(t) = set_conf_bus(&text, s.broker) {
                println!(
                    "  BUS={} in {conf_path}",
                    if s.broker { "broker" } else { "daemon" }
                );
                text = t;
                changed = true;
            }
            if let Some(t) = set_conf_choice(&text, "SYSLOG", "syslog", &s.syslog, SYSLOGS[0]) {
                println!("  SYSLOG={} in {conf_path}", s.syslog);
                text = t;
                changed = true;
            }
            if changed {
                ctx.put(conf_path, &text, 0o644);
            }
        }
        None if ctx.dry => warn(&format!("{conf_path} is missing")),
        None => die(&format!(
            "{conf_path} is missing: sudo dnf reinstall uinitconverter"
        )),
    }
    if !s.journal {
        let name = if s.syslog == "syslog-ng" {
            "syslog-ng.conf"
        } else {
            "rsyslog.conf"
        };
        let conf = read(&format!("{SHARE}/{name}")).unwrap_or_default();
        ctx.put(&format!("/etc/uinitconverter/{name}"), &conf, 0o644);
    }
    s6::configure(ctx, s);
    // The syslog daemons' state (rsyslog's work directory, syslog-ng's
    // persist file and control socket), labelled for them by the policy.
    let syslog_state = if s.syslog == "syslog-ng" {
        "/var/lib/syslog-ng"
    } else {
        "/var/lib/rsyslog"
    };
    let (state, logs) = (ctx.cfg.state_dir.display().to_string(), log_dir(&ctx.cfg));
    ctx.run(&["mkdir", "-p", &state, syslog_state]);
    // Every service's output is in there, with s6-log's mode 644: only for
    // root and wheel, like the journal.
    ctx.run(&["install", "-d", "-m", "0750", "-g", "wheel", &logs]);
}

/// LOG_DIR, also when JOURNAL=yes leaves it unset (a later --journal=no needs it).
fn log_dir(cfg: &Config) -> String {
    cfg.log_dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| DEFAULT_LOG_DIR.into())
}

fn unit_installed(cfg: &Config, unit: &str) -> bool {
    cfg.unit_path.iter().any(|d| d.join(unit).exists())
}

fn enable_units(ctx: &Ctx) {
    say("Enabling units (these are also normal on the systemd boot)");
    for u in ["chronyd.service", "NetworkManager.service"] {
        if unit_installed(&ctx.cfg, u) {
            ctx.run(&["systemctl", "--no-reload", "enable", u]);
        } else {
            warn(&format!(
                "{u} is not installed; the s6 boot runs without it"
            ));
        }
    }
    if rpm_installed("gdm") {
        ctx.run(&["systemctl", "--no-reload", "enable", "gdm.service"]);
        ctx.run(&[
            "systemctl",
            "--no-reload",
            "set-default",
            "graphical.target",
        ]);
    }
}

fn build_database(ctx: &Ctx) {
    say("Translating units into an s6-rc database");
    let gen = format!("{LIBEXEC}/uinitconverter-gen");
    ctx.run(&[&gen, "reload", "--no-update"]);
    if ctx.dry {
        return;
    }
    let report = ctx
        .cfg
        .state_dir
        .join("current/report.txt")
        .display()
        .to_string();
    println!("  report: {report}");
    let text = read(&report).unwrap_or_default();
    for l in text.lines().filter(|l| {
        ["translated:", "skipped:", "cycles broken:"]
            .iter()
            .any(|k| l.starts_with(k))
    }) {
        println!("  {l}");
    }
    let n = text.lines().filter(|l| l.contains("warning:")).count();
    println!("  {n} warnings (mostly sandboxing directives that are not enforced)");
}

fn setup_selinux(ctx: &Ctx) {
    say("SELinux labels");
    // s6-linux-init's stage 1 must run as init_t, like systemd.
    let init = format!("{S6LI_BASE}/bin/init");
    let labelled = output(&["semanage", "fcontext", "-l"])
        .is_some_and(|t| t.lines().any(|l| l.starts_with(&format!("{init} "))));
    if !labelled {
        ctx.run(&["semanage", "fcontext", "-a", "-t", "init_exec_t", &init]);
    }
    // ... and the whole s6 tree must stay there, so that services enter their
    // own domains as under systemd (see the policy module). The package's
    // %post loaded it; loading it again is cheap and covers a relabel.
    ctx.run(&[
        "semodule",
        "-i",
        &format!("{SHARE}/selinux/uinitconverter.cil"),
    ]);
    let mut paths: Vec<String> = [
        S6LI_BASE,
        LIBEXEC,
        "/usr/sbin/init-selinux",
        "/etc/uinitconverter",
        SHARE,
        "/usr/share/dbus-1/system.d/uinitconverter.conf",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([ctx.cfg.state_dir.display().to_string(), log_dir(&ctx.cfg)])
    .collect();
    paths.extend(s6::skarnet_files());
    let mut argv = vec!["restorecon", "-R"];
    argv.extend(paths.iter().map(String::as_str));
    ctx.run(&argv);
    s6::relabel_runtime(ctx);
}

// ---------------------------------------------------------------- texts

// ---------------------------------------------------------------- uninstall

fn uninstall(ctx: &Ctx, args: &[String]) -> ! {
    say("Removing the s6 boot entry and what uinitconverter init wrote");
    ctx.confirm(&format!(
        "Remove the '{TITLE}' boot entry, {S6LI_BASE} and the boot settings?"
    ));
    systemd_default(ctx);
    remove_grub_entry(ctx);
    for f in [
        OLD_KI_PLUGIN,
        BOOT_OPTIONS,
        BOOT_DEFAULT_FILE,
        BOOT_ONLY_S6_FILE,
    ] {
        if exists(f) {
            ctx.run(&["rm", "-f", f]);
        }
    }
    for img in list_dir("/boot", |n| {
        n.starts_with("initramfs-") && n.ends_with("-s6.img")
    }) {
        ctx.run(&["rm", "-f", &img]);
    }
    s6::uninstall(ctx);
    if ctx.dry {
        print!("{}", dry_run_end(args));
        exit(0);
    }
    println!("  Kept: /etc/uinitconverter (your configuration), /var/lib/uinitconverter, /var/log/uinitconverter,");
    println!("  and enabled units (all harmless for the systemd boot).");
    println!("  To remove the packages too: sudo dnf remove {CORE_PACKAGES}");
    println!("  (and the skarnet and standalone packages, if nothing else needs them).");
    exit(0);
}

// ---------------------------------------------------------------- main

pub fn main(args: &[String]) -> ! {
    // Its directories and TOOLS=; the defaults when there is no config yet.
    let cfg = Config::load(None).unwrap_or_else(|e| {
        warn(&e);
        Config::defaults()
    });
    let opts = match parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            print!("{USAGE}");
            exit(0);
        }
        Err(e) => {
            eprintln!("uinitconverter init: {e}");
            exit(2);
        }
    };
    let stamp = output(&["date", "+%Y%m%d-%H%M%S"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "now".into());
    let ctx = Ctx {
        dry: opts.dry,
        yes: opts.yes,
        backup: cfg
            .state_dir
            .join("setup-backup")
            .join(&stamp)
            .display()
            .to_string(),
        cfg,
    };
    check_system(&ctx);
    if opts.uninstall {
        uninstall(&ctx, args);
    }
    let s = resolve(&opts, &ctx.cfg);
    print!("{}", plan(&s, &ctx.backup));
    println!();
    check_programs(&ctx, &s);
    check_luks(&ctx);
    ctx.confirm("Continue?");
    check_versions(&ctx);
    configure_uinitconverter(&ctx, &s);
    enable_units(&ctx);
    build_database(&ctx);
    s6::setup_s6_linux_init(&ctx);
    // After the labels: the new daemon starts under the new policy, and
    // s6-svc -wR needs the event directories relabelled.
    setup_selinux(&ctx);
    s6::restart_daemon(&ctx);
    add_grub_entry(&ctx, &s);
    if ctx.dry {
        print!("{}", dry_run_end(args));
    } else {
        print!("{}", next_steps(&s));
    }
    exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dry_run_end_drops_the_flag() {
        assert!(dry_run_end(&args(&["--dry-run"])).contains("  sudo uinitconverter init\n"));
        assert!(
            dry_run_end(&args(&["--journal=no", "--dry-run", "--uninstall"]))
                .contains("  sudo uinitconverter init --journal=no --uninstall\n")
        );
    }

    #[test]
    fn options() {
        let o = parse_args(&args(&[
            "--default-s6",
            "--journal=no",
            "--selinux=enforcing",
            "-y",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(o.default_s6, Some(true));
        assert_eq!(o.journal, Some(false));
        assert_eq!(o.resolved, None);
        assert_eq!(o.enforcing, Some(true));
        assert!(o.yes && o.grub && !o.dry);
        let o = parse_args(&args(&[
            "--grub=no",
            "--boot-only-s6=yes",
            "--resolved=yes",
        ]))
        .unwrap()
        .unwrap();
        assert!(!o.grub);
        assert_eq!((o.boot_only_s6, o.resolved), (Some(true), Some(true)));
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert_eq!(
            parse_args(&args(&["--bus=broker"]))
                .unwrap()
                .unwrap()
                .broker,
            Some(true)
        );
        assert_eq!(
            parse_args(&args(&["--bus=daemon"]))
                .unwrap()
                .unwrap()
                .broker,
            Some(false)
        );
        assert_eq!(
            parse_args(&args(&["--syslog=syslog-ng"]))
                .unwrap()
                .unwrap()
                .syslog
                .as_deref(),
            Some("syslog-ng")
        );
        for bad in [
            "--journal",
            "--journal=off",
            "--selinux=disabled",
            "--no-journal",
            "--rpms=/x",
            "--yes=no",
            "--bus",
            "--bus=yes",
            "--syslog",
            "--syslog=metalog",
        ] {
            assert!(parse_args(&args(&[bad])).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn conf_keys() {
        let base = "# comment\nDEBUG=no\n";
        assert!(conf_bool(base, "JOURNAL"));
        assert!(!conf_bool("JOURNAL=yes\n  JOURNAL= Off\n", "JOURNAL"));
        assert!(conf_bool("JOURNALX=no\n", "JOURNAL"));
        assert_eq!(set_conf_key(base, "JOURNAL", true), None);
        let off = set_conf_key(base, "JOURNAL", false).unwrap();
        assert_eq!(
            off,
            "# comment\nDEBUG=no\n\n# set by uinitconverter init --journal=no\nJOURNAL=no\n"
        );
        assert_eq!(set_conf_key(&off, "JOURNAL", false), None);
        assert_eq!(set_conf_key(&off, "JOURNAL", true).unwrap(), base);
        // BUS=: dbus-daemon unless the last setting says broker; BUS_ADDRESS is another key.
        assert!(!conf_broker("BUS_ADDRESS=broker\n") && conf_broker("BUS=daemon\nBUS= Broker\n"));
        assert_eq!(set_conf_bus(base, false), None);
        let broker = set_conf_bus(base, true).unwrap();
        assert_eq!(
            broker,
            "# comment\nDEBUG=no\n\n# set by uinitconverter init --bus=broker\nBUS=broker\n"
        );
        assert_eq!(set_conf_bus(&broker, true), None);
        assert_eq!(set_conf_bus(&broker, false).unwrap(), base);
        assert_eq!(
            set_conf_bus("BUS=broker\nBUS_ADDRESS=x\n", false).unwrap(),
            "BUS_ADDRESS=x\n"
        );
        // SYSLOG=: the same, with the value in the marker; quotes are ignored.
        let ng = set_conf_choice(base, "SYSLOG", "syslog", "syslog-ng", "rsyslog").unwrap();
        assert_eq!(ng, "# comment\nDEBUG=no\n\n# set by uinitconverter init --syslog=syslog-ng\nSYSLOG=syslog-ng\n");
        assert_eq!(conf_choice(&ng, "SYSLOG").as_deref(), Some("syslog-ng"));
        assert_eq!(
            conf_choice("SYSLOG=\"Syslog-NG\"\n", "SYSLOG").as_deref(),
            Some("syslog-ng")
        );
        assert_eq!(
            set_conf_choice(&ng, "SYSLOG", "syslog", "syslog-ng", "rsyslog"),
            None
        );
        assert_eq!(
            set_conf_choice(&ng, "SYSLOG", "syslog", "rsyslog", "rsyslog").unwrap(),
            base
        );
        assert_eq!(
            set_conf_choice(base, "SYSLOG", "syslog", "rsyslog", "rsyslog"),
            None
        );
        // The old script's marker goes too.
        let old = "A=1\n\n# set by setup-fedora.sh --no-resolved\nRESOLVED=no\n";
        assert_eq!(set_conf_key(old, "RESOLVED", true).unwrap(), "A=1\n");
    }

    #[test]
    fn boot_settings() {
        assert_eq!(
            boot_args(true),
            "init=/usr/sbin/init-selinux plymouth.enable=0 rd.plymouth=0"
        );
        assert!(boot_args(false).ends_with(" enforcing=0"));
        assert_eq!(words("# s6: comment\ns6\n"), ["s6"]);
        assert!(words("# x\ninit=/a enforcing=0 # y\n").contains(&"enforcing=0".to_string()));
    }

    #[test]
    fn luks() {
        let tab = "# c\nluks-a UUID=a none\nluks-b UUID=b none\nhome /dev/sdb1 none\n";
        let late = |cmdline| -> Vec<String> {
            luks_volumes(tab, cmdline)
                .into_iter()
                .filter(|(_, early)| !early)
                .map(|(n, _)| n)
                .collect()
        };
        assert_eq!(
            late("root=x rd.luks.uuid=luks-a quiet\n"),
            ["luks-b", "home"]
        );
        assert_eq!(
            late("rd.luks.uuid=a rd.luks.name=b=x rd.luks.uuid=/dev/sdb1"),
            Vec::<String>::new()
        );
        assert_eq!(luks_volumes(tab, "").len(), 3);
    }
}
