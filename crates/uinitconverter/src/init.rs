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

mod s6;
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
const CORE_PACKAGES: &str = "uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd";
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

#[derive(Debug, PartialEq)]
struct Opts {
    dry: bool,
    yes: bool,
    uninstall: bool,
    default_s6: Option<bool>,
    journal: Option<bool>,
    resolved: Option<bool>,
    broker: Option<bool>,
    syslog: Option<String>,
    enforcing: Option<bool>,
    boot_only_s6: Option<bool>,
    grub: bool,
}

/// What this run sets up: the options, or what the last run set.
#[derive(Debug, PartialEq)]
struct Settings {
    journal: bool,
    resolved: bool,
    broker: bool,
    syslog: String,
    enforcing: bool,
    boot_only_s6: bool,
    default_s6: bool,
    grub: bool,
}

fn yes_no(opt: &str, v: &str) -> Result<bool, String> {
    match v {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(format!("{opt}: expected yes or no, got '{v}'")),
    }
}

/// The options, or None for --help.
fn parse_args(args: &[String]) -> Result<Option<Opts>, String> {
    let mut o = Opts {
        dry: false,
        yes: false,
        uninstall: false,
        default_s6: None,
        journal: None,
        resolved: None,
        broker: None,
        syslog: None,
        enforcing: None,
        boot_only_s6: None,
        grub: true,
    };
    for a in args {
        let (name, val) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a.as_str(), None),
        };
        match (name, val) {
            ("--dry-run", None) => o.dry = true,
            ("--yes" | "-y", None) => o.yes = true,
            ("--uninstall", None) => o.uninstall = true,
            ("--default-s6", None) => o.default_s6 = Some(true),
            ("--default-systemd", None) => o.default_s6 = Some(false),
            ("--journal", Some(v)) => o.journal = Some(yes_no(name, v)?),
            ("--resolved", Some(v)) => o.resolved = Some(yes_no(name, v)?),
            ("--bus", Some("broker")) => o.broker = Some(true),
            ("--bus", Some("daemon")) => o.broker = Some(false),
            ("--bus", Some(v)) => return Err(format!("--bus: expected daemon or broker, got '{v}'")),
            ("--syslog", Some(v)) if SYSLOGS.contains(&v) => o.syslog = Some(v.to_string()),
            ("--syslog", Some(v)) => return Err(format!("--syslog: expected {}, got '{v}'", SYSLOGS.join(" or "))),
            ("--boot-only-s6", Some(v)) => o.boot_only_s6 = Some(yes_no(name, v)?),
            ("--grub", Some(v)) => o.grub = yes_no(name, v)?,
            ("--selinux", Some("enforcing")) => o.enforcing = Some(true),
            ("--selinux", Some("permissive")) => o.enforcing = Some(false),
            ("--selinux", Some(v)) => return Err(format!("--selinux: expected enforcing or permissive, got '{v}'")),
            ("-h" | "--help", None) => return Ok(None),
            ("--journal" | "--resolved" | "--bus" | "--syslog" | "--boot-only-s6" | "--grub" | "--selinux", None) => {
                return Err(format!("{name} needs a value ({name}=...)"))
            }
            // The old setup-fedora.sh spellings.
            ("--no-journal" | "--no-resolved" | "--enforcing" | "--no-grub" | "--no-boot-only-s6", None) => {
                return Err(format!("unknown option {a}; see uinitconverter init --help (it is --NAME=yes|no now)"))
            }
            _ => return Err(format!("unknown option {a} (see uinitconverter init --help)")),
        }
    }
    Ok(Some(o))
}

/// Whether uinitconverter.conf text sets KEY to a true value (the last setting
/// counts; unset is true, as both keys default to yes).
fn conf_bool(text: &str, key: &str) -> bool {
    let v = text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(|v| v.trim().to_ascii_lowercase());
    !matches!(v.as_deref(), Some("no" | "false" | "0" | "off" | "n" | "f"))
}

/// The marker line above a KEY=no that init wrote.
fn conf_marker(key: &str) -> String {
    format!("# set by uinitconverter init --{}=no", key.to_ascii_lowercase())
}

/// uinitconverter.conf text with KEY set: false writes KEY=no, true removes the
/// setting (the default). Other lines are kept. None if nothing changes.
fn set_conf_key(text: &str, key: &str, val: bool) -> Option<String> {
    if conf_bool(text, key) == val {
        return None;
    }
    let marker = conf_marker(key);
    let old_marker = format!("# set by setup-fedora.sh --no-{}", key.to_ascii_lowercase());
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            let setting = l.trim_start().strip_prefix(key).is_some_and(|r| r.starts_with('='));
            !setting && *l != marker && *l != old_marker
        })
        .collect();
    let mut out = kept.join("\n").trim_end_matches('\n').to_string();
    if !val {
        out.push_str(&format!("\n\n{marker}\n{key}=no"));
    }
    out.push('\n');
    Some(out)
}

/// The value uinitconverter.conf text gives KEY (the last setting counts),
/// lowercased and without quotes; None when unset.
fn conf_choice(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(|v| v.trim().trim_matches('"').trim().to_ascii_lowercase())
}

/// Whether uinitconverter.conf text sets BUS=broker (unset is dbus-daemon).
fn conf_broker(text: &str) -> bool {
    conf_choice(text, "BUS").as_deref() == Some("broker")
}

/// KEY= for option OPT, as set_conf_key does it: the text with KEY=VALUE
/// under init's marker, or without any KEY= when VALUE is the default;
/// None if it already says so.
fn set_conf_choice(text: &str, key: &str, opt: &str, value: &str, default: &str) -> Option<String> {
    if conf_choice(text, key).as_deref().unwrap_or(default) == value {
        return None;
    }
    let marker = format!("# set by uinitconverter init --{opt}=");
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().strip_prefix(key).is_some_and(|r| r.starts_with('=')) && !l.starts_with(&marker))
        .collect();
    let mut out = kept.join("\n").trim_end_matches('\n').to_string();
    if value != default {
        out.push_str(&format!("\n\n{marker}{value}\n{key}={value}"));
    }
    out.push('\n');
    Some(out)
}

fn set_conf_bus(text: &str, broker: bool) -> Option<String> {
    set_conf_choice(text, "BUS", "bus", if broker { "broker" } else { "daemon" }, "daemon")
}

/// The words of a file without its # comments.
fn words(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace().map(String::from))
        .collect()
}

/// The s6 entry's kernel options.
fn boot_args(enforcing: bool) -> String {
    // plymouth's splash would keep the display (plymouth-quit.service is a
    // systemd-only unit), so the s6 entry boots without it and shows messages.
    let mut args = "init=/usr/sbin/init-selinux plymouth.enable=0 rd.plymouth=0".to_string();
    if !enforcing {
        args.push_str(" enforcing=0");
    }
    args
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn resolve(o: &Opts, cfg: &Config) -> Settings {
    let conf = fs::read_to_string(&cfg.config_path).unwrap_or_default();
    Settings {
        journal: o.journal.unwrap_or_else(|| conf_bool(&conf, "JOURNAL")),
        resolved: o.resolved.unwrap_or_else(|| conf_bool(&conf, "RESOLVED")),
        broker: o.broker.unwrap_or_else(|| conf_broker(&conf)),
        syslog: o
            .syslog
            .clone()
            .or_else(|| conf_choice(&conf, "SYSLOG").filter(|v| SYSLOGS.contains(&v.as_str())))
            .unwrap_or_else(|| SYSLOGS[0].to_string()),
        // The last run's options file; a first run boots permissive.
        enforcing: o
            .enforcing
            .unwrap_or_else(|| read(BOOT_OPTIONS).is_some_and(|t| !words(&t).iter().any(|w| w == "enforcing=0"))),
        boot_only_s6: o.boot_only_s6.unwrap_or_else(|| exists(BOOT_ONLY_S6_FILE)),
        default_s6: o
            .default_s6
            .unwrap_or_else(|| read(BOOT_DEFAULT_FILE).is_some_and(|t| words(&t) == ["s6"])),
        grub: o.grub,
    }
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
    let out = Command::new(tools::path(argv[0])).args(&argv[1..]).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
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

fn check_system(ctx: &Ctx) {
    if !ctx.dry && unsafe { libc::geteuid() } != 0 {
        die("run as root (or use --dry-run)");
    }
    let id = read("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("ID=").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| "unknown".into());
    if id != "fedora" {
        if ctx.dry && std::env::var("UINITCONVERTER_INIT_ANY_OS").as_deref() == Ok("1") {
            warn(&format!("not Fedora ({id}); dry run only"));
        } else {
            die(&format!("uinitconverter init is for Fedora (found ID={id})"));
        }
    }
    // What the steps copy or run; the configure step reads the two
    // configuration files, so a real run needs them.
    let packaged = [
        format!("{SHARE}/s6-linux-init/skel"),
        format!("{SHARE}/selinux/uinitconverter.cil"),
        format!("{SHARE}/s6.conf"),
        format!("{SHARE}/rsyslog.conf"),
        format!("{SHARE}/syslog-ng.conf"),
        format!("{LIBEXEC}/uinitconverter-gen"),
    ];
    if let Some(p) = packaged.iter().find(|p| !exists(p)) {
        let msg = format!("{p} is missing: install the packages first (sudo dnf install ./*.rpm with the RPMs from {RELEASES})");
        if ctx.dry {
            warn(&msg);
        } else {
            die(&msg);
        }
    }
}

/// The programs this run needs, by package. Missing ones end init (a dry
/// run only warns).
fn check_programs(ctx: &Ctx, s: &Settings) {
    let mut need: Vec<(bool, &str)> = vec![
        (in_path("s6-linux-init-maker"), "s6-linux-init"),
        (in_path("semanage"), "policycoreutils-python-utils"),
    ];
    if s.grub {
        need.push((in_path("grubby"), "grubby"));
    }
    if s.grub && s.default_s6 {
        need.push((in_path("grub2-editenv"), "grub2-tools"));
    }
    if !s.journal {
        need.push((exists(&format!("{LIBEXEC}/uinitconverter-journal-relay")), "uinitconverter-journal-relay"));
        if s.syslog == "syslog-ng" {
            need.push((in_path("syslog-ng"), "syslog-ng"));
        } else {
            need.push((in_path("rsyslogd"), "rsyslog"));
        }
    }
    if s.broker {
        need.push((in_path("dbus-broker-launch"), "dbus-broker"));
    }
    if s.grub && s.boot_only_s6 {
        need.push((in_path("mdevd"), "mdevd"));
    }
    let missing: Vec<&str> = need.iter().filter(|(ok, _)| !ok).map(|(_, p)| *p).collect();
    if !missing.is_empty() {
        let mut msg = format!("missing packages for these settings: sudo dnf install {}", missing.join(" "));
        if missing.iter().any(|p| SKARNET_PACKAGES.contains(p)) {
            msg += "; build s6-linux-init and mdevd with dist/rpm/build-skarnet-rpms.sh";
        }
        if missing.iter().any(|p| RELEASE_PACKAGES.contains(p)) {
            msg += &format!("; uinitconverter-journal-relay is an RPM in {RELEASES}");
        }
        if ctx.dry {
            warn(&msg);
        } else {
            die(&msg);
        }
    }
    if !rpm_installed("gdm") || !rpm_installed("gnome-shell") {
        warn("gdm/gnome-shell are not installed. For a full desktop run:");
        warn("  dnf group install workstation-product-environment");
    }
}

/// Show which builds are installed; the core tools must be from one build.
fn check_versions(ctx: &Ctx) {
    if ctx.dry {
        println!("  + uinitconverter --version");
        return;
    }
    let (lines, ok) = crate::version_report();
    for l in lines {
        println!("  {}", l.trim_start());
    }
    if !ok {
        warn(&format!(
            "the uinitconverter packages are not all from one build (see above): sudo dnf upgrade {CORE_PACKAGES}"
        ));
    }
}

/// The /etc/crypttab entries, each with whether the initramfs unlocks it.
fn luks_volumes(crypttab: &str, cmdline: &str) -> Vec<(String, bool)> {
    let cmdline = format!(" {} ", cmdline.trim());
    crypttab
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let name = f.next()?;
            if name.starts_with('#') {
                return None;
            }
            let dev = f.next().unwrap_or("");
            let uuid = dev.strip_prefix("UUID=").unwrap_or(dev);
            let early = cmdline.contains(&format!(" rd.luks.uuid={uuid} "))
                || cmdline.contains(&format!(" rd.luks.uuid=luks-{uuid} "))
                || cmdline.contains(&format!(" rd.luks.name={uuid}="));
            Some((name.to_string(), early))
        })
        .collect()
}

/// LUKS: volumes the initramfs unlocks (rd.luks.uuid=) are open before s6
/// starts. Anything else in /etc/crypttab is opened by systemd-cryptsetup@
/// units on a systemd boot, which do not exist under s6.
fn check_luks(ctx: &Ctx) {
    let Some(crypttab) = read("/etc/crypttab").filter(|t| !t.trim().is_empty()) else {
        return;
    };
    say("Checking encrypted volumes (/etc/crypttab)");
    let mut late = Vec::new();
    for (name, early) in luks_volumes(&crypttab, &read("/proc/cmdline").unwrap_or_default()) {
        if early {
            println!("  {name}: unlocked by the initramfs (fine)");
        } else {
            late.push(name);
        }
    }
    if !late.is_empty() {
        warn(&format!("not unlocked by the initramfs: {}", late.join(" ")));
        warn("  Under s6 nothing opens these (systemd-cryptsetup@ units are systemd-only),");
        warn("  so filesystems on them will not be mounted. Add them to the initramfs with");
        warn("  rd.luks.uuid= on the kernel command line, or leave them unused for this test.");
        ctx.confirm("Continue anyway?");
    }
    println!("  The LUKS passphrase prompt will be plain text (the s6 entry disables plymouth).");
}

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
                println!("  BUS={} in {conf_path}", if s.broker { "broker" } else { "daemon" });
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
        None => die(&format!("{conf_path} is missing: sudo dnf reinstall uinitconverter")),
    }
    if !s.journal {
        let name = if s.syslog == "syslog-ng" { "syslog-ng.conf" } else { "rsyslog.conf" };
        let conf = read(&format!("{SHARE}/{name}")).unwrap_or_default();
        ctx.put(&format!("/etc/uinitconverter/{name}"), &conf, 0o644);
    }
    s6::configure(ctx, s);
    // The syslog daemons' state (rsyslog's work directory, syslog-ng's
    // persist file and control socket), labelled for them by the policy.
    let syslog_state = if s.syslog == "syslog-ng" { "/var/lib/syslog-ng" } else { "/var/lib/rsyslog" };
    let (state, logs) = (ctx.cfg.state_dir.display().to_string(), log_dir(&ctx.cfg));
    ctx.run(&["mkdir", "-p", &state, syslog_state]);
    // Every service's output is in there, with s6-log's mode 644: only for
    // root and wheel, like the journal.
    ctx.run(&["install", "-d", "-m", "0750", "-g", "wheel", &logs]);
}

/// LOG_DIR, also when JOURNAL=yes leaves it unset (a later --journal=no needs it).
fn log_dir(cfg: &Config) -> String {
    cfg.log_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| DEFAULT_LOG_DIR.into())
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
            warn(&format!("{u} is not installed; the s6 boot runs without it"));
        }
    }
    if rpm_installed("gdm") {
        ctx.run(&["systemctl", "--no-reload", "enable", "gdm.service"]);
        ctx.run(&["systemctl", "--no-reload", "set-default", "graphical.target"]);
    }
}

fn build_database(ctx: &Ctx) {
    say("Translating units into an s6-rc database");
    let gen = format!("{LIBEXEC}/uinitconverter-gen");
    ctx.run(&[&gen, "reload", "--no-update"]);
    if ctx.dry {
        return;
    }
    let report = ctx.cfg.state_dir.join("current/report.txt").display().to_string();
    println!("  report: {report}");
    let text = read(&report).unwrap_or_default();
    for l in text.lines().filter(|l| ["translated:", "skipped:", "cycles broken:"].iter().any(|k| l.starts_with(k))) {
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
    ctx.run(&["semodule", "-i", &format!("{SHARE}/selinux/uinitconverter.cil")]);
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

/// The BLS entries init added (title "Fedora (s6 + uinitconverter)...").
fn s6_entries() -> Vec<String> {
    list_dir(BLS_DIR, |n| n.ends_with(".conf"))
        .into_iter()
        .filter(|f| read(f).is_some_and(|t| t.lines().any(|l| l.starts_with(&format!("title {TITLE}")))))
        .collect()
}

fn remove_grub_entry(ctx: &Ctx) {
    if Path::new(BLS_DIR).is_dir() {
        for f in s6_entries() {
            ctx.run(&["rm", "-f", &f]);
        }
    } else if in_path("grubby") {
        ctx.try_run(&["grubby", &format!("--remove-kernel=TITLE={TITLE}")]);
    }
}

/// The GRUB environment's saved_entry (GRUB_DEFAULT=saved), or nothing.
fn saved_entry() -> String {
    output(&["grub2-editenv", "-", "list"])
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("saved_entry=").map(String::from))
        .unwrap_or_default()
}

/// If the default is an s6 entry, make it the same kernel's systemd entry
/// (the s6 entry's BLS id is the systemd one's with "~uinitconverter" appended).
fn systemd_default(ctx: &Ctx) {
    let saved = saved_entry();
    if let Some(id) = saved.strip_suffix("~uinitconverter") {
        if exists(&format!("{BLS_DIR}/{id}.conf")) {
            ctx.run(&["grub2-editenv", "-", "set", &format!("saved_entry={id}")]);
        } else {
            ctx.run(&["grub2-editenv", "-", "unset", "saved_entry"]);
        }
    }
}

fn add_grub_entry(ctx: &Ctx, s: &Settings) {
    if !s.grub {
        return;
    }
    say(&format!("Adding boot entry: {TITLE}"));
    let args = boot_args(s.enforcing);
    ctx.put(BOOT_OPTIONS, &format!("# Extra kernel options for the \"{TITLE}\" boot entries.\n{args}\n"), 0o644);
    let kernel = if ctx.dry && !in_path("grubby") {
        "/boot/vmlinuz-KVER".to_string()
    } else {
        output(&["grubby", "--default-kernel"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| die("grubby --default-kernel failed"))
    };
    let kver = kernel.strip_prefix("/boot/vmlinuz-").unwrap_or(&kernel).to_string();
    // The plugin builds the s6 initramfs (--boot-only-s6=yes) while this file
    // exists, for the kernels below and for every later one.
    if s.boot_only_s6 {
        ctx.put(
            BOOT_ONLY_S6_FILE,
            "# The s6 entries boot with the s6 initramfs (uinitconverter init --boot-only-s6=yes).\n",
            0o644,
        );
    } else if exists(BOOT_ONLY_S6_FILE) {
        ctx.run(&["rm", "-f", BOOT_ONLY_S6_FILE]);
    }
    // An old setup-fedora.sh copy in /etc would hide the packaged plugin.
    if exists(OLD_KI_PLUGIN) {
        println!("  removing {OLD_KI_PLUGIN} (the package's {KI_PLUGIN} is used)");
        ctx.backup(OLD_KI_PLUGIN);
        ctx.run(&["rm", "-f", OLD_KI_PLUGIN]);
    }
    if Path::new(BLS_DIR).is_dir() || ctx.dry {
        // The kernel-install plugin copies the BLS entry for each kernel,
        // now and on every future kernel update.
        remove_grub_entry(ctx);
        // Every installed kernel, as the plugin keeps them: the removal
        // above took all their entries, not only the default kernel's.
        let kernels: Vec<String> = list_dir("/lib/modules", |_| true)
            .into_iter()
            .filter_map(|d| d.rsplit('/').next().map(String::from))
            .filter(|k| exists(&format!("/boot/vmlinuz-{k}")))
            .collect();
        if kernels.is_empty() {
            ctx.run(&[KI_PLUGIN, "add", &kver]);
        }
        for k in &kernels {
            ctx.run(&[KI_PLUGIN, "add", k]);
        }
    } else {
        remove_grub_entry(ctx);
        #[rustfmt::skip]
        ctx.run(&[
            "grubby", &format!("--add-kernel={kernel}"), &format!("--initrd=/boot/initramfs-{kver}.img"),
            "--copy-default", &format!("--title={TITLE} {kver}"), &format!("--args={args}"),
        ]);
        warn("no BLS entries: future kernels will not get an s6 entry automatically");
        if s.boot_only_s6 {
            warn("no BLS entries: --boot-only-s6=yes is not applied (the entry uses Fedora's initramfs)");
        }
    }
    if s.boot_only_s6 {
        println!("  The s6 entries boot with the s6 initramfs (/boot/initramfs-KVER-s6.img): no systemd at all.");
    }
    set_boot_default(ctx, s, &kver);
}

/// Make KVER's s6 entry (--default-s6) or the systemd entry the default.
/// The kernel-install plugin reads the same file and keeps the s6 entry the
/// default when a kernel is added or removed.
fn set_boot_default(ctx: &Ctx, s: &Settings, kver: &str) {
    if !s.default_s6 {
        if exists(BOOT_DEFAULT_FILE) {
            ctx.run(&["rm", "-f", BOOT_DEFAULT_FILE]);
        }
        systemd_default(ctx);
        println!("  The systemd entry is the default. Pick '{TITLE}' in the GRUB menu.");
        return;
    }
    ctx.put(
        BOOT_DEFAULT_FILE,
        &format!("# s6: the \"{TITLE}\" entry is the default boot entry (uinitconverter init --default-s6).\ns6\n"),
        0o644,
    );
    let suffix = format!("{kver}~uinitconverter.conf");
    let id = list_dir(BLS_DIR, |n| n.ends_with(&suffix))
        .last()
        .and_then(|f| f.rsplit('/').next())
        .map(|n| n.trim_end_matches(".conf").to_string());
    match id {
        Some(id) => ctx.run(&["grub2-editenv", "-", "set", &format!("saved_entry={id}")]),
        None if ctx.dry => ctx.run(&["grub2-editenv", "-", "set", &format!("saved_entry=ID-{kver}~uinitconverter")]),
        None => {
            warn(&format!("no s6 entry for {kver}: the default is unchanged"));
            return;
        }
    }
    println!("  '{TITLE}' is the default; the systemd entry stays in the GRUB menu.");
}

// ---------------------------------------------------------------- texts

fn plan(s: &Settings, backup: &str) -> String {
    let logs = if s.journal {
        "systemd-journald as an s6 service (journalctl works)".to_string()
    } else {
        format!("{} + s6-log directories, no journal (--journal=no, --syslog={})", s.syslog, s.syslog)
    };
    let dns = if s.resolved {
        "systemd-resolved as on the systemd boot"
    } else {
        "NetworkManager without systemd-resolved (s6 boot only; --resolved=no)"
    };
    let bus = if s.broker {
        "dbus-broker as on the systemd boot (--bus=broker; dbus-daemon if it can't start)"
    } else {
        "dbus-daemon (--bus=daemon)"
    };
    let grub = if !s.grub {
        "  * GRUB:    unchanged (--grub=no)\n".to_string()
    } else {
        format!(
            "  * GRUB:    add \"{TITLE}\" with init=/usr/sbin/init-selinux, no splash, SELinux {},\n\
             \x20            and keep it for every new kernel (the package's kernel-install plugin);\n\
             \x20            default entry: {}\n\
             \x20 * initramfs: {}\n",
            if s.enforcing { "enforcing" } else { "permissive" },
            if s.default_s6 { format!("{TITLE} (--default-s6)") } else { "the systemd one".into() },
            if s.boot_only_s6 {
                "the s6 entry boots with uinitconverter's s6 initramfs (s6-svscan as its PID 1,\n\
                 \x20            mdevd, passphrase prompt; --boot-only-s6=yes), built for every kernel"
            } else {
                "Fedora's (systemd inside) for both entries"
            }
        )
    };
    format!(
        "\n\
         This will set up booting Fedora with s6-linux-init + s6-rc + uinitconverter:\n\
         \n\
         \x20 * write:   /etc/uinitconverter (settings, boot options), {S6LI_BASE}, /etc/s6.conf\n\
         \x20 * enable:  chronyd, NetworkManager, gdm; default target graphical.target\n\
         \x20 * SELinux: label {S6LI_BASE}/bin/init as init_exec_t; load the policy module\n\
         \x20            {SHARE}/selinux/uinitconverter.cil (s6 tools and uinitconverter as init_exec_t,\n\
         \x20            so services run in their own domains as under systemd)\n\
         \x20 * logs:    {logs}\n\
         \x20 * DNS:     {dns}\n\
         \x20 * D-Bus:   {bus}\n\
         {grub}\
         \n\
         Nothing about the normal systemd boot changes: systemd, dbus-broker, journald\n\
         and all your units stay installed{}.\n\
         Backups of overwritten files go to {backup}.\n",
        if s.default_s6 {
            ", and the systemd entry stays in the menu"
        } else {
            "; the new entry is not made the default"
        }
    )
}

fn next_steps(s: &Settings) -> String {
    let reboot = if !s.grub {
        format!("Reboot and choose \"{TITLE}\" in the GRUB menu (if you added an entry yourself).")
    } else if s.default_s6 {
        format!("Reboot (\"{TITLE}\" is the default).")
    } else {
        format!("Reboot and choose \"{TITLE}\" in the GRUB menu.")
    };
    let selinux = if s.enforcing {
        "  4. SELinux: the entry boots enforcing. Check for denials with\n\
         \x20      ausearch -m AVC -ts boot\n"
    } else {
        "  4. SELinux: the entry boots permissive. Collect denials with\n\
         \x20      ausearch -m AVC -ts boot | audit2allow -M uinitconverter_local\n\
         \x20    review uinitconverter_local.te, load it with semodule -i, then run\n\
         \x20      sudo uinitconverter init --selinux=enforcing\n"
    };
    let only_s6 = if s.boot_only_s6 {
        "     With the s6 initramfs (--boot-only-s6=yes) the LUKS passphrase prompt is\n\
         \x20    plain text on the console. If it fails it opens a rescue shell (exit it to\n\
         \x20    try again); rd.break on the kernel command line gives a shell before the\n\
         \x20    switch. To boot one time with Fedora's initramfs, change the entry's\n\
         \x20    initrd line from initramfs-KVER-s6.img to initramfs-KVER.img (press e).\n"
    } else {
        ""
    };
    format!(
        "\n\
         Done. Next:\n\
         \x20 1. {reboot}\n\
         \x20 2. If stage 2 fails, log in on tty9 (Ctrl+Alt+F9) and look at:\n\
         \x20      /run/uncaught-logs/current        (s6 catch-all log)\n\
         \x20      s6-rc -a list                     (what is up)\n\
         \x20 3. Once booted:  systemctl status, systemctl list-units --failed,\n\
         \x20                  journalctl -b, journalctl -u systemd-logind, journalctl -u user@1000\n\
         {selinux}\
         \x20 5. If a new s6 database breaks booting, add this to the kernel command\n\
         \x20    line in GRUB (press e):\n\
         \x20      uinitconverter.db=previous    boot the database from before the last change\n\
         \x20      uinitconverter.bus=daemon     system bus with dbus-daemon for this boot\n\
         {only_s6}\
         \x20 6. To go back: boot the normal Fedora entry (hold Shift or press Esc for the\n\
         \x20    GRUB menu); make it the default again with\n\
         \x20      sudo uinitconverter init --default-systemd\n\
         \x20    To remove the s6 boot entry and what init wrote:\n\
         \x20      sudo uinitconverter init --uninstall\n"
    )
}

// ---------------------------------------------------------------- uninstall

fn uninstall(ctx: &Ctx, args: &[String]) -> ! {
    say("Removing the s6 boot entry and what uinitconverter init wrote");
    ctx.confirm(&format!("Remove the '{TITLE}' boot entry, {S6LI_BASE} and the boot settings?"));
    systemd_default(ctx);
    remove_grub_entry(ctx);
    for f in [OLD_KI_PLUGIN, BOOT_OPTIONS, BOOT_DEFAULT_FILE, BOOT_ONLY_S6_FILE] {
        if exists(f) {
            ctx.run(&["rm", "-f", f]);
        }
    }
    for img in list_dir("/boot", |n| n.starts_with("initramfs-") && n.ends_with("-s6.img")) {
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

/// The end of a --dry-run: the command that would apply what was shown.
fn dry_run_end(args: &[String]) -> String {
    let rest: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--dry-run").collect();
    let cmd = if rest.is_empty() { "sudo uinitconverter init".to_string() } else { format!("sudo uinitconverter init {}", rest.join(" ")) };
    format!("\nDry run: nothing was changed. To apply this, run:\n  {cmd}\n")
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
    let stamp = output(&["date", "+%Y%m%d-%H%M%S"]).map(|s| s.trim().to_string()).unwrap_or_else(|| "now".into());
    let ctx = Ctx {
        dry: opts.dry,
        yes: opts.yes,
        backup: cfg.state_dir.join("setup-backup").join(&stamp).display().to_string(),
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
        assert!(dry_run_end(&args(&["--journal=no", "--dry-run", "--uninstall"]))
            .contains("  sudo uinitconverter init --journal=no --uninstall\n"));
    }

    #[test]
    fn options() {
        let o = parse_args(&args(&["--default-s6", "--journal=no", "--selinux=enforcing", "-y"])).unwrap().unwrap();
        assert_eq!(o.default_s6, Some(true));
        assert_eq!(o.journal, Some(false));
        assert_eq!(o.resolved, None);
        assert_eq!(o.enforcing, Some(true));
        assert!(o.yes && o.grub && !o.dry);
        let o = parse_args(&args(&["--grub=no", "--boot-only-s6=yes", "--resolved=yes"])).unwrap().unwrap();
        assert!(!o.grub);
        assert_eq!((o.boot_only_s6, o.resolved), (Some(true), Some(true)));
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert_eq!(parse_args(&args(&["--bus=broker"])).unwrap().unwrap().broker, Some(true));
        assert_eq!(parse_args(&args(&["--bus=daemon"])).unwrap().unwrap().broker, Some(false));
        assert_eq!(parse_args(&args(&["--syslog=syslog-ng"])).unwrap().unwrap().syslog.as_deref(), Some("syslog-ng"));
        for bad in ["--journal", "--journal=off", "--selinux=disabled", "--no-journal", "--rpms=/x", "--yes=no", "--bus", "--bus=yes", "--syslog", "--syslog=metalog"] {
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
        assert_eq!(off, "# comment\nDEBUG=no\n\n# set by uinitconverter init --journal=no\nJOURNAL=no\n");
        assert_eq!(set_conf_key(&off, "JOURNAL", false), None);
        assert_eq!(set_conf_key(&off, "JOURNAL", true).unwrap(), base);
        // BUS=: dbus-daemon unless the last setting says broker; BUS_ADDRESS is another key.
        assert!(!conf_broker("BUS_ADDRESS=broker\n") && conf_broker("BUS=daemon\nBUS= Broker\n"));
        assert_eq!(set_conf_bus(base, false), None);
        let broker = set_conf_bus(base, true).unwrap();
        assert_eq!(broker, "# comment\nDEBUG=no\n\n# set by uinitconverter init --bus=broker\nBUS=broker\n");
        assert_eq!(set_conf_bus(&broker, true), None);
        assert_eq!(set_conf_bus(&broker, false).unwrap(), base);
        assert_eq!(set_conf_bus("BUS=broker\nBUS_ADDRESS=x\n", false).unwrap(), "BUS_ADDRESS=x\n");
        // SYSLOG=: the same, with the value in the marker; quotes are ignored.
        let ng = set_conf_choice(base, "SYSLOG", "syslog", "syslog-ng", "rsyslog").unwrap();
        assert_eq!(ng, "# comment\nDEBUG=no\n\n# set by uinitconverter init --syslog=syslog-ng\nSYSLOG=syslog-ng\n");
        assert_eq!(conf_choice(&ng, "SYSLOG").as_deref(), Some("syslog-ng"));
        assert_eq!(conf_choice("SYSLOG=\"Syslog-NG\"\n", "SYSLOG").as_deref(), Some("syslog-ng"));
        assert_eq!(set_conf_choice(&ng, "SYSLOG", "syslog", "syslog-ng", "rsyslog"), None);
        assert_eq!(set_conf_choice(&ng, "SYSLOG", "syslog", "rsyslog", "rsyslog").unwrap(), base);
        assert_eq!(set_conf_choice(base, "SYSLOG", "syslog", "rsyslog", "rsyslog"), None);
        // The old script's marker goes too.
        let old = "A=1\n\n# set by setup-fedora.sh --no-resolved\nRESOLVED=no\n";
        assert_eq!(set_conf_key(old, "RESOLVED", true).unwrap(), "A=1\n");
    }

    #[test]
    fn boot_settings() {
        assert_eq!(boot_args(true), "init=/usr/sbin/init-selinux plymouth.enable=0 rd.plymouth=0");
        assert!(boot_args(false).ends_with(" enforcing=0"));
        assert_eq!(words("# s6: comment\ns6\n"), ["s6"]);
        assert!(words("# x\ninit=/a enforcing=0 # y\n").contains(&"enforcing=0".to_string()));
    }

    #[test]
    fn luks() {
        let tab = "# c\nluks-a UUID=a none\nluks-b UUID=b none\nhome /dev/sdb1 none\n";
        let late = |cmdline| -> Vec<String> {
            luks_volumes(tab, cmdline).into_iter().filter(|(_, early)| !early).map(|(n, _)| n).collect()
        };
        assert_eq!(late("root=x rd.luks.uuid=luks-a quiet\n"), ["luks-b", "home"]);
        assert_eq!(late("rd.luks.uuid=a rd.luks.name=b=x rd.luks.uuid=/dev/sdb1"), Vec::<String>::new());
        assert_eq!(luks_volumes(tab, "").len(), 3);
    }
}
