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
/// capability.h's CAP_WAKE_ALARM (libc has no constant for it).
const CAP_WAKE_ALARM: libc::c_ulong = 35;

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
    // The generators' three directories sit where systemd puts them:
    // generator.early before the configuration, generator after /run,
    // generator.late last (systemd-xdg-autostart-generator writes there).
    vec![
        format!("{runtime}/systemd/transient"),
        format!("{runtime}/systemd/generator.early"),
        format!("{config}/systemd/user"),
        "/etc/xdg/systemd/user".into(),
        "/etc/systemd/user".into(),
        format!("{runtime}/systemd/user"),
        "/run/systemd/user".into(),
        format!("{runtime}/systemd/generator"),
        format!("{data}/systemd/user"),
        "/usr/local/share/systemd/user".into(),
        "/usr/share/systemd/user".into(),
        "/usr/local/lib/systemd/user".into(),
        "/usr/lib/systemd/user".into(),
        format!("{runtime}/systemd/generator.late"),
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
    use std::io::Read;
    for (name, path) in executables(dirs) {
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
        let status = wait_limited(&mut child, std::time::Duration::from_secs(5));
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

/// systemd.generator(7)'s user directories, highest priority first.
/// UINITCONVERTER_USER_UNIT_GENERATOR_PATH (colon-separated) replaces them
/// (tests).
fn unit_generator_dirs() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("UINITCONVERTER_USER_UNIT_GENERATOR_PATH") {
        return p
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    [
        "/run/systemd/user-generators",
        "/etc/systemd/user-generators",
        "/usr/local/lib/systemd/user-generators",
        "/usr/lib/systemd/user-generators",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// The generators that may run: by name, an earlier directory's name hiding
/// a later one, links to /dev/null and non-executables left out.
fn executables(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            found.entry(name).or_insert_with(|| e.path());
        }
    }
    found
        .into_iter()
        .filter(|(_, path)| {
            !fs::canonicalize(path)
                .map(|p| p == Path::new("/dev/null"))
                .unwrap_or(true)
                && fs::metadata(path)
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
        })
        .collect()
}

/// Unit generators, as systemd --user runs them at startup and at each
/// reload (systemd.generator(7)): the three output directories are made
/// empty, then each generator runs with them as arguments, in the units'
/// starting environment plus SYSTEMD_SCOPE=user, for at most 5 s.
/// systemd-xdg-autostart-generator makes the app-*@autostart.service
/// units xdg-desktop-autostart.target wants; podman-user-generator makes
/// the quadlet units.
pub fn run_unit_generators(runtime: &str, env: &[(String, String)]) {
    run_unit_generators_from(&unit_generator_dirs(), runtime, env)
}

fn run_unit_generators_from(gens: &[PathBuf], runtime: &str, env: &[(String, String)]) {
    let out = PathBuf::from(runtime).join("systemd");
    let dirs = ["generator", "generator.early", "generator.late"].map(|d| out.join(d));
    for d in &dirs {
        let _ = fs::remove_dir_all(d);
        if let Err(e) = fs::create_dir_all(d) {
            eprintln!("uinitconverterd user-manager: {}: {e}", d.display());
            return;
        }
    }
    for (name, path) in executables(gens) {
        let child = Command::new(&path)
            .args(&dirs)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .env("SYSTEMD_SCOPE", "user")
            .stdin(std::process::Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("uinitconverterd user-manager: generator {name}: {e}");
                continue;
            }
        };
        match wait_limited(&mut child, std::time::Duration::from_secs(5)) {
            Some(st) if st.success() => {}
            Some(st) => eprintln!("uinitconverterd user-manager: generator {name} failed ({st})"),
            None => eprintln!("uinitconverterd user-manager: generator {name} timed out, killed"),
        }
    }
}

/// The units' starting environment as the launcher wrote it.
pub fn written_env(run_dir: &Path) -> Vec<(String, String)> {
    fs::read_to_string(run_dir.join("manager.env"))
        .map(|t| crate::envfile::parse(&t))
        .unwrap_or_default()
}

fn wait_limited(
    child: &mut std::process::Child,
    limit: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
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
    kv("USER_FALLBACK_ON", &sys.user_fallback_on.join(" "));
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
    // This boot already fell back for this user, or the last s6 manager of
    // this boot ended without cleaning up (a crash): systemd --user.
    let uid = unsafe { libc::getuid() };
    let fb = Fallback::new(uid);
    if let Some(reason) = fb.marked() {
        eprintln!("uinitconverterd user-manager: fell back earlier in this boot ({reason}); running systemd --user");
        hand_over(delegated_cgroup().as_deref(), args);
    }
    if let Some(pid) = fb.stale_running() {
        let reason = format!("the s6 user manager of this boot (PID {pid}) ended without stopping");
        fb.mark(&reason);
        eprintln!("uinitconverterd user-manager: {reason}; running systemd --user");
        hand_over(delegated_cgroup().as_deref(), args);
    }
    fb.set_running();
    // systemd gives user@ CAP_WAKE_ALARM as an ambient capability (for
    // systemd --user's WakeSystem= timers, kept by the exec above), and
    // systemd --user does not pass it to its units. Ambient capabilities
    // survive exec, so here every unit would carry it, and bwrap refuses to
    // run with capabilities it did not expect: GNOME Shell's glycin image
    // loaders failed, and the background with them.
    unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
            0,
            0,
            0,
        );
    }
    let runtime = env_dir("XDG_RUNTIME_DIR").unwrap_or_else(|| format!("/run/user/{uid}"));
    if !Path::new(&runtime).is_dir() {
        eprintln!("uinitconverterd user-manager: no runtime directory {runtime}");
        setup_failed(&fb, args);
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
            setup_failed(&fb, args);
        }
    }
    let _ = fs::create_dir_all(PathBuf::from(&runtime).join("systemd"));
    if let Err(e) = std::os::unix::fs::symlink(base.join("scan"), base.join("live/scandir")) {
        eprintln!("uinitconverterd user-manager: {e}");
        setup_failed(&fb, args);
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
        setup_failed(&fb, args);
    }
    let _ = util::write_atomic(
        &base.join("run/manager.env"),
        manager_env(&runtime).as_bytes(),
    );
    run_unit_generators(&runtime, &written_env(&base.join("run")));
    let cfg = match Config::load(Some(&conf)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("uinitconverterd user-manager: {e}");
            setup_failed(&fb, args);
        }
    };

    let mut svscan = match Command::new(crate::tools::path("s6-svscan"))
        .arg(base.join("scan"))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("uinitconverterd user-manager: s6-svscan: {e}");
            setup_failed(&fb, args);
        }
    };
    let addr = cfg.bus_address.clone().unwrap_or_default();
    let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::serve(cfg, &["--address".into(), addr])
    }));
    // The units are stopped (or the daemon failed): end s6-svscan, which
    // ends what is left under it.
    let _ = Command::new(crate::tools::path("s6-svscanctl"))
        .arg("-t")
        .arg(base.join("scan"))
        .status();
    let _ = svscan.wait();
    let reason = match served {
        Err(_) => Some("the daemon panicked".to_string()),
        Ok(0) => crate::daemon::fallback_reason(),
        Ok(code) => Some(format!("the daemon exited with {code}")),
    };
    if let Some(reason) = reason {
        fb.mark(&reason);
        fb.clear_running();
        eprintln!("uinitconverterd user-manager: {reason}; handing over to systemd --user");
        hand_over(cgroup.as_deref(), args);
    }
    fb.clear_running();
    0
}

/// The fallback's record, for this boot: `<uid>` (the reason) and
/// `<uid>.running` (the PID of the s6 manager running now) in
/// FALLBACK_DIR, sticky and writable by all (tmpfiles.d), as only files
/// of the user's own count there; in $XDG_RUNTIME_DIR when that
/// directory is missing or the name is taken by another user's file (the
/// fallback then lasts until the user's last session ends).
pub const FALLBACK_DIR: &str = "/run/uinitconverter-user";

struct Fallback {
    uid: u32,
    dirs: Vec<PathBuf>,
}

impl Fallback {
    fn new(uid: u32) -> Self {
        let mut dirs = Vec::new();
        if let Ok(d) = std::env::var("UINITCONVERTER_USER_FALLBACK_DIR") {
            dirs.push(PathBuf::from(d));
        } else {
            dirs.push(PathBuf::from(FALLBACK_DIR));
        }
        let runtime = env_dir("XDG_RUNTIME_DIR").unwrap_or_else(|| format!("/run/user/{uid}"));
        dirs.push(PathBuf::from(runtime).join("uinitconverter-fallback"));
        Fallback { uid, dirs }
    }

    /// The user's own file `name` in the first directory that has one.
    fn read(&self, name: &str) -> Option<String> {
        use std::os::unix::fs::MetadataExt;
        self.dirs.iter().find_map(|d| {
            let p = d.join(name);
            let m = fs::symlink_metadata(&p).ok()?;
            (m.is_file() && m.uid() == self.uid)
                .then(|| fs::read_to_string(&p).ok())
                .flatten()
        })
    }

    fn write(&self, name: &str, text: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        for (i, d) in self.dirs.iter().enumerate() {
            if i > 0 {
                let _ = fs::create_dir_all(d);
            }
            let p = d.join(name);
            // Our own file is replaced; another user's is not ours to touch.
            let _ = fs::remove_file(&p);
            let r = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&p)
                .and_then(|mut f| f.write_all(text.as_bytes()));
            if r.is_ok() {
                return;
            }
        }
        eprintln!("uinitconverterd user-manager: could not record {name} for the fallback");
    }

    fn remove(&self, name: &str) {
        use std::os::unix::fs::MetadataExt;
        for d in &self.dirs {
            let p = d.join(name);
            if fs::symlink_metadata(&p)
                .map(|m| m.uid() == self.uid)
                .unwrap_or(false)
            {
                let _ = fs::remove_file(&p);
            }
        }
    }

    fn marked(&self) -> Option<String> {
        self.read(&self.uid.to_string())
            .map(|r| r.trim().to_string())
    }

    fn mark(&self, reason: &str) {
        self.write(&self.uid.to_string(), reason);
    }

    /// The PID in a running stamp left by an s6 manager that is gone.
    fn stale_running(&self) -> Option<u32> {
        let pid: u32 = self
            .read(&format!("{}.running", self.uid))?
            .trim()
            .parse()
            .ok()?;
        let alive = fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim() == "uinitconverterd")
            .unwrap_or(false);
        (!alive && pid != std::process::id()).then_some(pid)
    }

    fn set_running(&self) {
        self.write(
            &format!("{}.running", self.uid),
            &std::process::id().to_string(),
        );
    }

    fn clear_running(&self) {
        self.remove(&format!("{}.running", self.uid));
    }
}

/// Empty the delegated cgroup but for init.scope (where this process is):
/// what an s6 manager left must not run beside systemd --user, which also
/// wants the tree to itself. cgroup.kill, then the empty cgroups go,
/// deepest first; other processes in init.scope are killed.
fn empty_cgroup(base: &Path) {
    fn subdirs(d: &Path) -> Vec<PathBuf> {
        fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn remove_tree(d: &Path) {
        for s in subdirs(d) {
            remove_tree(&s);
        }
        let _ = fs::remove_dir(d);
    }
    let me = std::process::id().to_string();
    if let Ok(procs) = fs::read_to_string(base.join("init.scope/cgroup.procs")) {
        for p in procs.lines().filter(|p| *p != me) {
            if let Ok(pid) = p.parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
    let children: Vec<PathBuf> = subdirs(base)
        .into_iter()
        .filter(|d| !d.ends_with("init.scope"))
        .collect();
    for d in &children {
        let _ = fs::write(d.join("cgroup.kill"), "1");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    for d in &children {
        while fs::read_to_string(d.join("cgroup.events"))
            .map(|e| e.lines().any(|l| l == "populated 1"))
            .unwrap_or(false)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        remove_tree(d);
    }
}

/// The s6 manager could not even be set up (its message is out already):
/// systemd --user instead, for the rest of this boot.
fn setup_failed(fb: &Fallback, args: &[String]) -> ! {
    fb.mark("the s6 user manager could not be set up");
    fb.clear_running();
    hand_over(delegated_cgroup().as_deref(), args)
}

/// The fallback itself: an empty cgroup, CAP_WAKE_ALARM back in the
/// ambient set as systemd gave it (cleared for our units above), then
/// systemd --user in this process, so user@UID.service goes on.
fn hand_over(cgroup: Option<&Path>, args: &[String]) -> ! {
    if let Some(base) = cgroup {
        empty_cgroup(base);
    }
    unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong,
            CAP_WAKE_ALARM,
            0,
            0,
        );
    }
    let systemd =
        std::env::var("UINITCONVERTER_SYSTEMD_USER").unwrap_or_else(|_| SYSTEMD_USER.into());
    let err = Command::new(&systemd).arg("--user").args(args).exec();
    eprintln!("uinitconverterd user-manager: {systemd}: {err}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback record: the reason and the running stamp, read back;
    /// a stamp naming a process that is gone is stale;
    /// clear_running removes only the stamp; a name taken in the shared
    /// directory falls through to the second one.
    #[test]
    fn fallback_record() {
        let top = std::env::temp_dir().join(format!("uc-fallback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let (shared, own) = (top.join("shared"), top.join("own"));
        fs::create_dir_all(&shared).unwrap();
        let uid = unsafe { libc::getuid() };
        let fb = Fallback {
            uid,
            dirs: vec![shared.clone(), own.clone()],
        };
        assert_eq!(fb.marked(), None);
        assert_eq!(fb.stale_running(), None);
        fb.mark("broken on purpose");
        assert_eq!(fb.marked().as_deref(), Some("broken on purpose"));
        assert!(shared.join(uid.to_string()).exists());
        // The stamp naming this very process is not stale (the manager
        // reading its own); a dead PID is.
        fb.set_running();
        assert_eq!(fb.stale_running(), None);
        fs::write(shared.join(format!("{uid}.running")), "999999999").unwrap();
        assert_eq!(fb.stale_running(), Some(999999999));
        fb.clear_running();
        assert!(!shared.join(format!("{uid}.running")).exists());
        assert!(shared.join(uid.to_string()).exists());
        // A directory where the name cannot be created: the second one.
        fs::remove_file(shared.join(uid.to_string())).unwrap();
        fs::create_dir(shared.join(uid.to_string())).unwrap();
        fb.mark("elsewhere");
        assert_eq!(
            fs::read_to_string(own.join(uid.to_string())).unwrap(),
            "elsewhere"
        );
        assert_eq!(fb.marked().as_deref(), Some("elsewhere"));
        let _ = fs::remove_dir_all(&top);
    }

    /// Unit generators get the three directories, made empty first (a
    /// reload drops the last run's output), and SYSTEMD_SCOPE=user; a
    /// generator hidden by a /dev/null link of the same name earlier does
    /// not run.
    #[test]
    fn unit_generators_run_with_fresh_directories() {
        use std::os::unix::fs::PermissionsExt;
        let top = std::env::temp_dir().join(format!("uc-unitgen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let (hi, lo, rt) = (top.join("hi"), top.join("lo"), top.join("rt"));
        for d in [&hi, &lo, &rt] {
            fs::create_dir_all(d).unwrap();
        }
        let script = |p: &Path, body: &str| {
            fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(
            &lo.join("a"),
            "echo \"$SYSTEMD_SCOPE $FOO\" >\"$3/a.out\"; touch \"$1/n\" \"$2/e\"",
        );
        script(&lo.join("b"), "touch \"$3/b.out\"");
        std::os::unix::fs::symlink("/dev/null", hi.join("b")).unwrap();
        let late = rt.join("systemd/generator.late");
        fs::create_dir_all(&late).unwrap();
        fs::write(late.join("stale.service"), "").unwrap();
        let env = vec![("FOO".to_string(), "bar".to_string())];
        run_unit_generators_from(&[hi, lo], rt.to_str().unwrap(), &env);
        assert_eq!(
            fs::read_to_string(late.join("a.out")).unwrap(),
            "user bar\n"
        );
        assert!(rt.join("systemd/generator/n").exists());
        assert!(rt.join("systemd/generator.early/e").exists());
        assert!(!late.join("stale.service").exists());
        assert!(!late.join("b.out").exists());
        let _ = fs::remove_dir_all(&top);
    }

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
