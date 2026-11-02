//! `uinitconverterd user-manager`: what user@UID.service runs instead of
//! `systemd --user` (work_history parts 136-141). Under
//! $XDG_RUNTIME_DIR/uinitconverter it makes a scan directory with its own
//! s6-svscan and a configuration in user mode, then serves
//! org.freedesktop.systemd1 there: the private socket at once
//! ($XDG_RUNTIME_DIR/systemd/private, where `systemctl --user` looks
//! first), the user bus once dbus.socket has brought it up. READY=1 goes to
//! user@UID.service when default.target is up; SIGTERM stops every unit,
//! then s6-svscan.
//!
//! USER_MANAGER=systemd in the system configuration, or
//! uinitconverter.user_manager=systemd on the kernel command line, makes it
//! exec `systemd --user` instead: user@.service can carry this command
//! whichever manager runs.

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::util;

const SYSTEMD_USER: &str = "/usr/lib/systemd/systemd";

/// USER_MANAGER=: the system configuration's, overridden by the kernel
/// command line's uinitconverter.user_manager=.
pub fn chosen(cfg: &Config, cmdline: &str) -> String {
    cfg.user_manager_now(cmdline)
}

fn env_dir(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| v.starts_with('/'))
}

/// The search path of `systemd --user` (systemd.unit(5)), the user's own
/// configuration first: enable and mask write there.
/// UINITCONVERTER_USER_UNIT_PATH (colon-separated) replaces it (tests).
pub fn unit_path(home: &str, runtime: &str) -> Vec<String> {
    if let Ok(p) = std::env::var("UINITCONVERTER_USER_UNIT_PATH") {
        return p
            .split(':')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    let config = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| format!("{home}/.config"));
    let data = env_dir("XDG_DATA_HOME").unwrap_or_else(|| format!("{home}/.local/share"));
    vec![
        format!("{config}/systemd/user"),
        format!("{runtime}/systemd/transient"),
        "/etc/systemd/user".into(),
        format!("{runtime}/systemd/user"),
        "/run/systemd/user".into(),
        format!("{data}/systemd/user"),
        "/usr/local/lib/systemd/user".into(),
        "/usr/lib/systemd/user".into(),
    ]
}

/// The delegated cgroup this process runs in (user@UID.service), if it
/// may write it. UINITCONVERTER_USER_CGROUP=no: none (tests, which run
/// inside the host's own user manager).
fn delegated_cgroup() -> Option<PathBuf> {
    if std::env::var("UINITCONVERTER_USER_CGROUP").as_deref() == Ok("no") {
        return None;
    }
    let text = fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = text.lines().find_map(|l| l.strip_prefix("0::"))?;
    let mut dir = PathBuf::from(crate::cgroup::CGROUP_ROOT).join(rel.trim_start_matches('/'));
    // A restarted manager finds itself in init.scope already.
    if dir.ends_with("init.scope") {
        dir.pop();
    }
    let procs = std::ffi::CString::new(
        dir.join("cgroup.procs")
            .into_os_string()
            .into_encoded_bytes(),
    )
    .ok()?;
    let subtree = std::ffi::CString::new(
        dir.join("cgroup.subtree_control")
            .into_os_string()
            .into_encoded_bytes(),
    )
    .ok()?;
    let ok = unsafe {
        libc::access(procs.as_ptr(), libc::W_OK) == 0
            && libc::access(subtree.as_ptr(), libc::W_OK) == 0
    };
    (ok && dir != Path::new(crate::cgroup::CGROUP_ROOT)).then_some(dir)
}

fn quote(v: &str) -> String {
    let v = v
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    format!("\"{v}\"")
}

/// The units' starting environment (systemd --user's: what user@UID.service
/// gave it, plus MANAGERPID and the bus).
fn manager_env(runtime: &str) -> String {
    let mut env: Vec<(String, String)> = Vec::new();
    let keep = |k: &str| {
        matches!(
            k,
            "HOME"
                | "USER"
                | "LOGNAME"
                | "SHELL"
                | "PATH"
                | "LANG"
                | "LANGUAGE"
                | "XDG_RUNTIME_DIR"
                | "XDG_DATA_DIRS"
                | "XDG_CONFIG_DIRS"
                | "XDG_SEAT"
                | "XDG_SESSION_ID"
                | "TZ"
        ) || k.starts_with("LC_")
    };
    for (k, v) in std::env::vars() {
        if keep(&k) {
            env.push((k, v));
        }
    }
    set(&mut env, "MANAGERPID", &std::process::id().to_string());
    set(
        &mut env,
        "DBUS_SESSION_BUS_ADDRESS",
        &format!("unix:path={runtime}/bus"),
    );
    run_generators(&mut env, &generator_dirs());
    env.iter()
        .map(|(k, v)| format!("{k}={}\n", quote(v)))
        .collect()
}

fn set(env: &mut Vec<(String, String)>, k: &str, v: &str) {
    match env.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = v.to_string(),
        None => env.push((k.to_string(), v.to_string())),
    }
}

/// systemd.environment-generator(7)'s user directories, highest priority
/// first. UINITCONVERTER_USER_GENERATOR_PATH (colon-separated) replaces
/// them (tests).
fn generator_dirs() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("UINITCONVERTER_USER_GENERATOR_PATH") {
        return p
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    [
        "/run/systemd/user-environment-generators",
        "/etc/systemd/user-environment-generators",
        "/usr/local/lib/systemd/user-environment-generators",
        "/usr/lib/systemd/user-environment-generators",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// Environment generators, as systemd --user runs them at startup: by name,
/// a name in an earlier directory hiding the same name later (a link to
/// /dev/null disables it); each one sees the environment so far and prints
/// KEY=VALUE lines that update it (30-systemd-environment-d-generator reads
/// environment.d, 60-flatpak adds its XDG_DATA_DIRS). Each gets 5 s.
fn run_generators(env: &mut Vec<(String, String)>, dirs: &[PathBuf]) {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            found.entry(name).or_insert_with(|| e.path());
        }
    }
    for (name, path) in found {
        if fs::canonicalize(&path)
            .map(|p| p == Path::new("/dev/null"))
            .unwrap_or(true)
        {
            continue;
        }
        let exec = fs::metadata(&path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
        if !exec {
            continue;
        }
        let child = Command::new(&path)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("uinitconverterd user-manager: generator {name}: {e}");
                continue;
            }
        };
        // Read on a thread: a generator that fills the pipe would block.
        let mut out = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(o) = out.as_mut() {
                let _ = o.read_to_string(&mut text);
            }
            text
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let text = reader.join().unwrap_or_default();
        match status {
            Some(st) if st.success() => {
                for (k, v) in crate::envfile::parse(&text) {
                    set(env, &k, &v);
                }
            }
            Some(st) => eprintln!(
                "uinitconverterd user-manager: generator {name} failed ({st}), its output ignored"
            ),
            None => eprintln!("uinitconverterd user-manager: generator {name} timed out, killed"),
        }
    }
}

/// The user manager's configuration file, from the system's (`sys`): its
/// tools and logging, the user's directories.
fn write_config(
    sys: &Config,
    path: &Path,
    base: &Path,
    runtime: &str,
    cgroup: Option<&Path>,
) -> Result<(), String> {
    let uid = unsafe { libc::getuid() };
    let home = env_dir("HOME")
        .or_else(|| util::lookup_uid(uid).map(|u| u.home))
        .unwrap_or_else(|| "/".into());
    let state = env_dir("XDG_STATE_HOME").unwrap_or_else(|| format!("{home}/.local/state"));
    let config = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| format!("{home}/.config"));
    let mystate = format!("{state}/uinitconverter");
    let b = base.display();
    let mut text = String::new();
    let mut kv = |k: &str, v: &str| text.push_str(&format!("{k}={}\n", quote(v)));
    kv("USER_MODE", "yes");
    kv("UNIT_PATH", &unit_path(&home, runtime).join(":"));
    kv("RUNTIME_UNIT_DIR", &format!("{runtime}/systemd/user"));
    kv(
        "PRESET_PATH",
        &[
            format!("{config}/systemd/user-preset"),
            "/etc/systemd/user-preset".into(),
            "/usr/local/lib/systemd/user-preset".into(),
            "/usr/lib/systemd/user-preset".into(),
        ]
        .join(":"),
    );
    kv("EXTRA_SOURCE", "");
    kv("STATE_DIR", &mystate);
    kv("REPO_DIR", &format!("{mystate}/repo"));
    kv("LIVE_DIR", &format!("{b}/live"));
    kv("RUN_DIR", &format!("{b}/run"));
    kv("TRANSIENT_DIR", &format!("{runtime}/systemd/transient"));
    kv("BOOTED_FLAG", "");
    kv("PRIVATE_SOCKET", &format!("{runtime}/systemd/private"));
    kv("BUS_ADDRESS", &format!("unix:path={runtime}/bus"));
    kv("ALLOW_UIDS", &uid.to_string());
    kv("RUNNER", &sys.runner.display().to_string());
    kv("TIMER_RUNNER", &sys.timer_runner.display().to_string());
    kv("JOURNAL", if sys.journal { "yes" } else { "no" });
    kv("JOURNAL_STDOUT", &sys.journal_stdout.display().to_string());
    kv("LOG_DIR", &format!("{mystate}/log"));
    kv("UDEV_MONITOR", "no");
    kv("UDEV_DIR", "");
    // The system's EXCLUDE names system units; a user manager runs all of
    // its units (USER_EXCLUDE= in the system configuration: globs it
    // leaves out).
    kv("EXCLUDE", &sys.user_exclude.join(" "));
    kv("INCLUDE", "");
    kv(
        "CGROUP_BASE",
        &cgroup.map(|c| c.display().to_string()).unwrap_or_default(),
    );
    util::write_atomic(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Make the directories, start s6-svscan, and run the daemon in user mode.
pub fn main(sys: &Config, args: &[String]) -> i32 {
    let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    // On a systemd boot, systemd's own user manager unless the kernel
    // command line asks for this one (tests/vm-user-manager.sh does).
    let systemd_boot = fs::read_to_string("/proc/1/comm")
        .map(|c| c.trim() == "systemd")
        .unwrap_or(false);
    let asked = cmdline
        .split_whitespace()
        .any(|w| w == "uinitconverter.user_manager=s6");
    if (systemd_boot && !asked) || chosen(sys, &cmdline) == "systemd" {
        let err = Command::new(SYSTEMD_USER).arg("--user").args(args).exec();
        eprintln!("uinitconverterd user-manager: {SYSTEMD_USER}: {err}; running the s6 user manager instead");
    }
    let uid = unsafe { libc::getuid() };
    let runtime = env_dir("XDG_RUNTIME_DIR").unwrap_or_else(|| format!("/run/user/{uid}"));
    if !Path::new(&runtime).is_dir() {
        eprintln!("uinitconverterd user-manager: no runtime directory {runtime}");
        return 1;
    }
    // XDG_RUNTIME_DIR is set for the units even when user@ did not.
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    let base = PathBuf::from(&runtime).join("uinitconverter");
    // Fresh for each run: the units' state is this manager's (a previous
    // one's s6-svscan went with its cgroup at its stop).
    let _ = fs::remove_dir_all(&base);
    for d in ["scan", "live", "run"] {
        if let Err(e) = fs::create_dir_all(base.join(d)) {
            eprintln!(
                "uinitconverterd user-manager: {}: {e}",
                base.join(d).display()
            );
            return 1;
        }
    }
    let _ = fs::create_dir_all(PathBuf::from(&runtime).join("systemd"));
    if let Err(e) = std::os::unix::fs::symlink(base.join("scan"), base.join("live/scandir")) {
        eprintln!("uinitconverterd user-manager: {e}");
        return 1;
    }

    // systemd's user layout: the manager in init.scope, the units in
    // app.slice and the others beside it (no process may stay in a cgroup
    // whose controllers are enabled).
    let cgroup = delegated_cgroup();
    if let Some(dir) = &cgroup {
        let init = dir.join("init.scope");
        let r = fs::create_dir_all(&init)
            .and_then(|_| fs::write(init.join("cgroup.procs"), std::process::id().to_string()));
        if let Err(e) = r {
            eprintln!("uinitconverterd user-manager: {}: {e}", init.display());
        }
    }

    let conf = base.join("uinitconverter.conf");
    if let Err(e) = write_config(sys, &conf, &base, &runtime, cgroup.as_deref()) {
        eprintln!("uinitconverterd user-manager: {e}");
        return 1;
    }
    let _ = util::write_atomic(
        &base.join("run/manager.env"),
        manager_env(&runtime).as_bytes(),
    );
    let cfg = match Config::load(Some(&conf)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("uinitconverterd user-manager: {e}");
            return 1;
        }
    };

    let mut svscan = match Command::new(crate::tools::path("s6-svscan"))
        .arg(base.join("scan"))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("uinitconverterd user-manager: s6-svscan: {e}");
            return 1;
        }
    };
    let addr = cfg.bus_address.clone().unwrap_or_default();
    let code = crate::serve(cfg, &["--address".into(), addr]);
    // The units are stopped (or the daemon failed): end s6-svscan, which
    // ends what is left under it.
    let _ = Command::new(crate::tools::path("s6-svscanctl"))
        .arg("-t")
        .arg(base.join("scan"))
        .status();
    let _ = svscan.wait();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path lists are written as Config::load splits them (":"):
    /// joined with blanks, the whole list was one directory that does not
    /// exist, and no unit was found (the first VM run).
    #[test]
    fn written_paths_split_as_config_reads_them() {
        let dir = std::env::temp_dir().join(format!("uc-user-conf-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("u.conf");
        write_config(&Config::defaults(), &conf, &dir, "/run/user/4242", None).unwrap();
        let vars = crate::envfile::load(&conf).unwrap();
        let get = |k: &str| {
            vars.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        let units: Vec<String> = get("UNIT_PATH").split(':').map(str::to_string).collect();
        assert!(units.len() > 3, "{units:?}");
        assert!(
            units.iter().all(|u| u.starts_with('/') && !u.contains(' ')),
            "{units:?}"
        );
        assert!(units.contains(&"/usr/lib/systemd/user".to_string()));
        assert!(units.contains(&"/run/user/4242/systemd/transient".to_string()));
        assert_eq!(get("PRESET_PATH").split(':').count(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generators_in_order_and_overridden() {
        use std::os::unix::fs::PermissionsExt;
        let top = std::env::temp_dir().join(format!("uc-gen-{}", std::process::id()));
        let (hi, lo) = (top.join("etc"), top.join("lib"));
        fs::create_dir_all(&hi).unwrap();
        fs::create_dir_all(&lo).unwrap();
        let script = |dir: &Path, name: &str, body: &str| {
            let p = dir.join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        };
        // 10 runs first and sees the starting environment; 20 sees 10's
        // output; lib's 20 is hidden by etc's; 30 fails and is ignored.
        script(&lo, "10-a", "echo \"A=one\"; echo \"SEEN=$START\"");
        script(&hi, "20-b", "echo \"B=$A-two\"");
        script(&lo, "20-b", "echo B=hidden");
        script(&lo, "30-c", "echo C=bad; exit 1");
        let mut env = vec![("START".to_string(), "s".to_string())];
        run_generators(&mut env, &[hi.clone(), lo.clone()]);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("A").as_deref(), Some("one"));
        assert_eq!(get("SEEN").as_deref(), Some("s"));
        assert_eq!(get("B").as_deref(), Some("one-two"));
        assert_eq!(get("C"), None);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn kernel_flag_overrides() {
        let mut cfg = Config::defaults();
        cfg.user_manager = "s6".into();
        assert_eq!(chosen(&cfg, "quiet"), "s6");
        assert_eq!(
            chosen(&cfg, "ro uinitconverter.user_manager=systemd quiet"),
            "systemd"
        );
        cfg.user_manager = "systemd".into();
        assert_eq!(chosen(&cfg, "uinitconverter.user_manager=s6"), "s6");
    }
}
