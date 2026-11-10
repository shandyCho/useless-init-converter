//! Command line and uinitconverter.conf: what `initconvert init` was asked to do.

use super::*;

#[derive(Debug, PartialEq)]
pub(super) struct Opts {
    pub(super) dry: bool,
    pub(super) yes: bool,
    pub(super) uninstall: bool,
    pub(super) default_s6: Option<bool>,
    pub(super) journal: Option<bool>,
    pub(super) resolved: Option<bool>,
    pub(super) broker: Option<bool>,
    pub(super) syslog: Option<String>,
    pub(super) enforcing: Option<bool>,
    /// Some(None): auto.
    pub(super) boot_only_s6: Option<Option<bool>>,
    pub(super) grub: bool,
}

/// What this run sets up: the options, or what the last run set.
#[derive(Debug, PartialEq)]
pub(super) struct Settings {
    pub(super) journal: bool,
    pub(super) resolved: bool,
    pub(super) broker: bool,
    pub(super) syslog: String,
    pub(super) enforcing: bool,
    pub(super) boot_only_s6: bool,
    /// Why auto kept Fedora's initramfs (None: it didn't, or not auto).
    pub(super) boot_only_s6_why: Option<String>,
    /// An explicit --boot-only-s6=no, kept for the next runs.
    pub(super) boot_only_s6_off: bool,
    pub(super) default_s6: bool,
    pub(super) grub: bool,
}

pub(super) fn yes_no(opt: &str, v: &str) -> Result<bool, String> {
    match v {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(format!("{opt}: expected yes or no, got '{v}'")),
    }
}

/// The options, or None for --help.
pub(super) fn parse_args(args: &[String]) -> Result<Option<Opts>, String> {
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
            ("--bus", Some(v)) => {
                return Err(format!("--bus: expected daemon or broker, got '{v}'"))
            }
            ("--syslog", Some(v)) if SYSLOGS.contains(&v) => o.syslog = Some(v.to_string()),
            ("--syslog", Some(v)) => {
                return Err(format!(
                    "--syslog: expected {}, got '{v}'",
                    SYSLOGS.join(" or ")
                ))
            }
            ("--boot-only-s6", Some("auto")) => o.boot_only_s6 = Some(None),
            ("--boot-only-s6", Some(v)) => o.boot_only_s6 = Some(Some(yes_no(name, v)?)),
            ("--grub", Some(v)) => o.grub = yes_no(name, v)?,
            ("--selinux", Some("enforcing")) => o.enforcing = Some(true),
            ("--selinux", Some("permissive")) => o.enforcing = Some(false),
            ("--selinux", Some(v)) => {
                return Err(format!(
                    "--selinux: expected enforcing or permissive, got '{v}'"
                ))
            }
            ("-h" | "--help", None) => return Ok(None),
            (
                "--journal" | "--resolved" | "--bus" | "--syslog" | "--boot-only-s6" | "--grub"
                | "--selinux",
                None,
            ) => return Err(format!("{name} needs a value ({name}=...)")),
            // The old setup-fedora.sh spellings.
            (
                "--no-journal" | "--no-resolved" | "--enforcing" | "--no-grub"
                | "--no-boot-only-s6",
                None,
            ) => {
                return Err(format!(
                    "unknown option {a}; see initconvert init --help (it is --NAME=yes|no now)"
                ))
            }
            _ => return Err(format!("unknown option {a} (see initconvert init --help)")),
        }
    }
    Ok(Some(o))
}

/// Whether uinitconverter.conf text sets KEY to a true value (the last setting
/// counts; unset is DEFAULT, config.rs's JOURNAL_DEFAULT/RESOLVED_DEFAULT).
pub(super) fn conf_bool(text: &str, key: &str, default: bool) -> bool {
    let v = text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(|v| v.trim().to_ascii_lowercase());
    match v.as_deref() {
        Some("no" | "false" | "0" | "off" | "n" | "f") => false,
        Some("yes" | "true" | "1" | "on" | "y" | "t") => true,
        _ => default,
    }
}

/// The marker line above a KEY=yes|no that init (or the package's update
/// scriptlet) wrote.
pub(super) fn conf_marker(key: &str, val: bool) -> String {
    format!(
        "# set by uinitconverter init --{}={}",
        key.to_ascii_lowercase(),
        if val { "yes" } else { "no" }
    )
}

/// uinitconverter.conf text with KEY set: a value other than DEFAULT is
/// written as KEY=yes|no under init's marker, the default removes the
/// setting. Other lines are kept. None if nothing changes.
fn is_setting(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|r| r.starts_with('='))
}

pub(super) fn set_conf_key(text: &str, key: &str, val: bool, default: bool) -> Option<String> {
    if conf_bool(text, key, default) == val
        && (val != default || !text.lines().any(|l| is_setting(l, key)))
    {
        return None;
    }
    let markers = [
        conf_marker(key, true),
        conf_marker(key, false),
        format!("# set by setup-fedora.sh --no-{}", key.to_ascii_lowercase()),
    ];
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !is_setting(l, key) && !markers.iter().any(|m| m == l))
        .collect();
    let mut out = kept.join("\n").trim_end_matches('\n').to_string();
    if val != default {
        out.push_str(&format!(
            "\n\n{}\n{key}={}",
            conf_marker(key, val),
            if val { "yes" } else { "no" }
        ));
    }
    out.push('\n');
    Some(out)
}

/// The value uinitconverter.conf text gives KEY (the last setting counts),
/// lowercased and without quotes; None when unset.
pub(super) fn conf_choice(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix('='))
        .last()
        .map(|v| v.trim().trim_matches('"').trim().to_ascii_lowercase())
}

/// Whether uinitconverter.conf text sets BUS=broker (unset is dbus-daemon).
pub(super) fn conf_broker(text: &str) -> bool {
    conf_choice(text, "BUS").as_deref() == Some("broker")
}

/// KEY= for option OPT, as set_conf_key does it: the text with KEY=VALUE
/// under init's marker, or without any KEY= when VALUE is the default;
/// None if it already says so.
pub(super) fn set_conf_choice(
    text: &str,
    key: &str,
    opt: &str,
    value: &str,
    default: &str,
) -> Option<String> {
    if conf_choice(text, key).as_deref().unwrap_or(default) == value {
        return None;
    }
    let marker = format!("# set by uinitconverter init --{opt}=");
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            !l.trim_start()
                .strip_prefix(key)
                .is_some_and(|r| r.starts_with('='))
                && !l.starts_with(&marker)
        })
        .collect();
    let mut out = kept.join("\n").trim_end_matches('\n').to_string();
    if value != default {
        out.push_str(&format!("\n\n{marker}{value}\n{key}={value}"));
    }
    out.push('\n');
    Some(out)
}

pub(super) fn set_conf_bus(text: &str, broker: bool) -> Option<String> {
    set_conf_choice(
        text,
        "BUS",
        "bus",
        if broker { "broker" } else { "daemon" },
        "daemon",
    )
}

/// The words of a file without its # comments.
pub(super) fn words(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace().map(String::from))
        .collect()
}

/// The s6 entry's kernel options.
pub(super) fn boot_args(enforcing: bool) -> String {
    // plymouth's splash would keep the display (plymouth-quit.service is a
    // systemd-only unit), so the s6 entry boots without it and shows messages.
    let mut args = "init=/usr/sbin/init-selinux plymouth.enable=0 rd.plymouth=0".to_string();
    if !enforcing {
        args.push_str(" enforcing=0");
    }
    args
}

pub(super) fn resolve(o: &Opts, cfg: &Config) -> Settings {
    let conf = fs::read_to_string(&cfg.config_path).unwrap_or_default();
    // --boot-only-s6: yes, no, auto (the default: uinitconverter's
    // initramfs where it can boot this root, rules 2-4); left out, what the
    // last run set (the plugin's file, or an explicit no's marker).
    let explicit_s6 = match o.boot_only_s6 {
        Some(v) => v,
        None if exists(BOOT_ONLY_S6_FILE) => Some(true),
        None if exists(BOOT_ONLY_S6_OFF_FILE) => Some(false),
        None => None,
    };
    let (boot_only_s6, boot_only_s6_why) = match explicit_s6 {
        Some(v) => (v, None),
        None => match super::checks::s6_initramfs_blocker() {
            None => (true, None),
            Some(why) => (false, Some(why)),
        },
    };
    Settings {
        journal: o
            .journal
            .unwrap_or_else(|| conf_bool(&conf, "JOURNAL", JOURNAL_DEFAULT)),
        resolved: o
            .resolved
            .unwrap_or_else(|| conf_bool(&conf, "RESOLVED", RESOLVED_DEFAULT)),
        broker: o.broker.unwrap_or_else(|| conf_broker(&conf)),
        syslog: o
            .syslog
            .clone()
            .or_else(|| conf_choice(&conf, "SYSLOG").filter(|v| SYSLOGS.contains(&v.as_str())))
            .unwrap_or_else(|| SYSLOGS[0].to_string()),
        // The last run's options file; a first run boots permissive.
        enforcing: o.enforcing.unwrap_or_else(|| {
            read(BOOT_OPTIONS).is_some_and(|t| !words(&t).iter().any(|w| w == "enforcing=0"))
        }),
        boot_only_s6,
        boot_only_s6_why,
        boot_only_s6_off: explicit_s6 == Some(false),
        default_s6: o
            .default_s6
            .unwrap_or_else(|| read(BOOT_DEFAULT_FILE).is_some_and(|t| words(&t) == ["s6"])),
        grub: o.grub,
    }
}
