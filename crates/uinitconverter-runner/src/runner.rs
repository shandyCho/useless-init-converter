//! The service runner: what systemd's PID 1 does between fork() and exec().
//!
//! Called from generated s6-rc scripts:
//!   run      longrun `run` script; ends by exec()ing the main process
//!   finish   longrun `finish` script; cleanup and Restart= policy
//!   up/down  oneshot scripts
//! and by the daemon:
//!   stop-exec / reload   ExecStop= / ExecReload= with $MAINPID

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::cgroup;
use crate::conditions;
use crate::config::Config;
use crate::execcmd::{self, ExecCommand};
use crate::install;
use crate::loader::{LoadState, Loader, Unit};
use crate::sandbox::{self, Sandbox};
use crate::socket;
use crate::supervisor;
use crate::tools;
use crate::state::{keys, UnitState};
use crate::util::{self, UserInfo};

// systemd's exit codes for setup failures (EXIT_CHDIR etc).
const EXIT_CHDIR: i32 = 200;
const EXIT_EXEC: i32 = 203;
const EXIT_MEMORY: i32 = 204;
const EXIT_LIMITS: i32 = 205;
const EXIT_STDIN: i32 = 208;
const EXIT_STDOUT: i32 = 209;
const EXIT_CHROOT: i32 = 210;
const EXIT_GROUP: i32 = 216;
const EXIT_USER: i32 = 217;
const EXIT_CAPABILITIES: i32 = 218;
const EXIT_NAMESPACE: i32 = 226;
const EXIT_SECCOMP: i32 = 228;

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn cstr(s: &str) -> CString {
    CString::new(s.as_bytes().iter().copied().filter(|&b| b != 0).collect::<Vec<u8>>()).unwrap()
}

pub struct Runner {
    pub cfg: Config,
    pub unit: Unit,
    pub st: UnitState,
    user: Option<UserInfo>,
    gid: Option<u32>,
    groups: Vec<u32>,
    /// Why User=/Group= didn't resolve, for a command other than a start:
    /// the exit code and message for any process spawned.
    cred_error: Option<(i32, String)>,
    pub env: Vec<(String, String)>,
    writable_dirs: Vec<String>,
    sandbox: Sandbox,
    notify_fd: Option<i32>,
    is_root: bool,
    /// Start timeout for commands run by the runner itself (oneshots, forking).
    deadline: std::cell::Cell<Option<std::time::Instant>>,
    /// What `deadline` limits, for the log: "start", or "stop" for ExecStop=.
    deadline_what: std::cell::Cell<&'static str>,
    timed_out: std::cell::Cell<bool>,
    /// `run --on-demand` (uinitconverter-gen): bind the sockets and wait for
    /// activity before the first start, as systemd's socket activation.
    pub on_demand: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Exited(i32),
    Killed(i32),
}

impl Status {
    fn from_wait(s: i32) -> Status {
        if libc::WIFSIGNALED(s) {
            Status::Killed(libc::WTERMSIG(s))
        } else {
            Status::Exited(libc::WEXITSTATUS(s))
        }
    }
    fn ok(&self) -> bool {
        *self == Status::Exited(0)
    }
}

fn wait_pid(pid: i32) -> i32 {
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

fn log(unit: &str, msg: &str) {
    eprintln!("uinitconverter[{unit}]: {msg}");
    util::syslog_line(libc::LOG_ERR, &format!("{unit}: {msg}"));
}

impl Runner {
    /// `starting`: the command starts the unit (`run`, `up`), so a User=,
    /// Group= or SupplementaryGroups= that doesn't resolve fails it.
    pub fn new(cfg: Config, name: &str, starting: bool) -> Result<Runner, String> {
        let unit = Loader::new(&cfg).load(name);
        if unit.load_state != LoadState::Loaded {
            return Err(format!("unit {name}: {}", unit.load_state.as_str()));
        }
        let st = crate::state::for_unit(&cfg.run_dir, &unit.id);
        st.ensure();
        let is_root = unsafe { libc::geteuid() } == 0;

        // Only a start needs the credentials. For the other commands a user
        // that is gone by now (GDM's greeter user, dropped from userdb before
        // its user@ stops) must not skip the stop's bookkeeping; as systemd,
        // only a process spawned with them fails (217/USER, 216/GROUP).
        let (user, gid, groups, cred_error) = match resolve_credentials(&unit) {
            Ok((user, gid, groups)) => (user, gid, groups, None),
            Err((_, e)) if starting => return Err(e),
            Err(e) => (None, None, Vec::new(), Some(e)),
        };

        let mut r = Runner {
            on_demand: false,
            sandbox: Sandbox::from_unit(&unit, &st.dir, Vec::new()),
            cfg,
            unit,
            st,
            user,
            gid,
            groups,
            cred_error,
            env: Vec::new(),
            writable_dirs: Vec::new(),
            notify_fd: None,
            is_root,
            deadline: std::cell::Cell::new(None),
            deadline_what: std::cell::Cell::new("start"),
            timed_out: std::cell::Cell::new(false),
        };
        r.build_env()?;
        Ok(r)
    }

    fn id(&self) -> &str {
        &self.unit.id
    }

    pub fn setenv(&mut self, k: &str, v: &str) {
        env_set(&mut self.env, k, v);
    }

    /// The manager's and the runner's own variables (systemd's our_env);
    /// the unit's settings go on top in unit_env().
    fn build_env(&mut self) -> Result<(), String> {
        self.setenv("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin");
        if let Ok(vars) = crate::envfile::load(Path::new("/etc/locale.conf")) {
            for (k, v) in vars {
                self.setenv(&k, &v);
            }
        }
        if let Ok(vars) = crate::envfile::load(&self.cfg.run_dir.join("manager.env")) {
            for (k, v) in vars {
                self.setenv(&k, &v);
            }
        }
        if let Some(inv) = self.st.get(keys::INVOCATION) {
            self.setenv("INVOCATION_ID", &inv);
        }
        self.memory_pressure_env();
        if let Some(u) = &self.user {
            let (name, home, shell) = (u.name.clone(), u.home.clone(), u.shell.clone());
            self.setenv("USER", &name);
            self.setenv("LOGNAME", &name);
            self.setenv("HOME", &home);
            self.setenv("SHELL", &shell);
        }
        for (key, base, var) in DIRS {
            let dirs: Vec<String> = self
                .unit
                .get_words("Service", key)
                .iter()
                .map(|d| format!("{base}/{}", d.split(':').next().unwrap_or(d)))
                .collect();
            if !dirs.is_empty() {
                self.setenv(var, &dirs.join(":"));
            }
        }
        if self.uses_tty() && !self.env.iter().any(|(k, _)| k == "TERM") {
            let tty = self.tty_path();
            let term = if tty.starts_with("/dev/ttyS") || tty.starts_with("/dev/ttyAMA") { "vt220" } else { "linux" };
            self.setenv("TERM", term);
        }
        // A missing EnvironmentFile= fails the start here already.
        self.unit_env(&[])?;
        Ok(())
    }

    /// The environment of a command, in systemd's order (exec_invoke()):
    /// our_env, PassEnvironment=, Environment=, EnvironmentFile=, each
    /// overriding the ones before, then UnsetEnvironment=. The files are
    /// read for every command, as systemd does: an ExecStartPre= may write
    /// the file ExecStart= reads.
    fn unit_env(&self, extra_env: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
        let mut env = self.env.clone();
        for (k, v) in extra_env {
            env_set(&mut env, k, v);
        }
        for k in self.unit.get_words("Service", "PassEnvironment") {
            if let Ok(v) = std::env::var(&k) {
                env_set(&mut env, &k, &v);
            }
        }
        for w in self.unit.get_words("Service", "Environment") {
            if let Some((k, v)) = w.split_once('=') {
                env_set(&mut env, k, v);
            }
        }
        for f in self.unit.get_all("Service", "EnvironmentFile") {
            let (optional, path) = match f.strip_prefix('-') {
                Some(p) => (true, p.trim().to_string()),
                None => (false, f.clone()),
            };
            match crate::envfile::load(Path::new(&path)) {
                Ok(vars) => {
                    for (k, v) in vars {
                        env_set(&mut env, &k, &v);
                    }
                }
                Err(_) if optional => {}
                Err(e) => return Err(format!("EnvironmentFile {path}: {e}")),
            }
        }
        for k in self.unit.get_words("Service", "UnsetEnvironment") {
            match k.split_once('=') {
                Some(_) => env.retain(|(ek, ev)| format!("{ek}={ev}") != k),
                None => env.retain(|(ek, _)| *ek != k),
            }
        }
        Ok(env)
    }

    fn uses_tty(&self) -> bool {
        let input = self.unit.get("Service", "StandardInput").unwrap_or_default();
        let output = self.unit.get("Service", "StandardOutput").unwrap_or_default();
        input.starts_with("tty") || output == "tty"
    }

    fn tty_path(&self) -> String {
        self.unit.get("Service", "TTYPath").unwrap_or_else(|| "/dev/console".into())
    }

    // ---- directories ----

    fn create_directories(&mut self) -> Result<(), String> {
        for (key, base, _) in DIRS {
            let mode = self
                .unit
                .get("Service", &format!("{key}Mode"))
                .and_then(|m| u32::from_str_radix(&m, 8).ok())
                .unwrap_or(0o755);
            for d in self.unit.get_words("Service", key) {
                let d = d.split(':').next().unwrap_or(&d).to_string();
                let path = PathBuf::from(base).join(&d);
                fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                crate::selinux::restorecon(&path.display().to_string());
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
                if *key != "ConfigurationDirectory" {
                    if let Some(u) = &self.user {
                        let c = cstr(&path.display().to_string());
                        unsafe { libc::chown(c.as_ptr(), u.uid, self.gid.unwrap_or(u.gid)) };
                    }
                }
                self.writable_dirs.push(path.display().to_string());
            }
        }
        self.sandbox.writable_dirs = self.writable_dirs.clone();
        Ok(())
    }

    fn remove_runtime_dirs(&self) {
        let preserve = self.unit.get("Service", "RuntimeDirectoryPreserve").unwrap_or_default();
        if preserve == "yes" || preserve == "restart" {
            return;
        }
        for d in self.unit.get_words("Service", "RuntimeDirectory") {
            let d = d.split(':').next().unwrap_or(&d).to_string();
            if !d.is_empty() && !d.contains("..") {
                let _ = fs::remove_dir_all(Path::new("/run").join(d));
            }
        }
    }

    // ---- process setup ----

    /// Apply everything systemd applies to a service process, then exec().
    /// Only returns on failure, with the exit code to use.
    fn exec_command(&self, cmd: &ExecCommand, extra_env: &[(String, String)]) -> (i32, String) {
        unsafe {
            // Reset signal state inherited from the supervisor (or the PAM
            // wrapper's forwarding handlers): dispositions first, so a
            // signal pending in the mask is not caught by a handler.
            for sig in 1..=libc::SIGRTMAX() {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                    libc::signal(sig, libc::SIG_DFL);
                }
            }
            if self.unit.get_bool("Service", "IgnoreSIGPIPE", true) {
                libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            }
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
        }

        if let Err(e) = self.setup_stdio(cmd) {
            return e;
        }
        if let Some(e) = &self.cred_error {
            return e.clone();
        }

        let sandboxed = !cmd.privileged && self.is_root;
        if sandboxed {
            if let Err(e) = self.sandbox.apply() {
                return (EXIT_NAMESPACE, e);
            }
        }

        if let Err(e) = self.apply_limits() {
            return (EXIT_LIMITS, e);
        }

        if !cmd.privileged {
            if let Some(root) = self.unit.get("Service", "RootDirectory") {
                let c = cstr(&root);
                if unsafe { libc::chroot(c.as_ptr()) } < 0 {
                    return (EXIT_CHROOT, format!("chroot {root}: {}", errno()));
                }
            }
        }

        let wd = self.unit.get("Service", "WorkingDirectory").unwrap_or_else(|| "/".into());
        let (wd_optional, wd) = match wd.strip_prefix('-') {
            Some(w) => (true, w.to_string()),
            None => (false, wd),
        };
        let wd = if wd == "~" {
            self.user.as_ref().map(|u| u.home.clone()).unwrap_or_else(|| "/root".into())
        } else {
            wd
        };
        let c = cstr(&wd);
        if unsafe { libc::chdir(c.as_ptr()) } < 0 && !wd_optional {
            return (EXIT_CHDIR, format!("chdir {wd}: {}", errno()));
        }

        // Root once exec()ed: the user switch below does not happen.
        let switches_user = !cmd.privileged && !cmd.no_setuid && self.user.as_ref().map(|u| u.uid != 0).unwrap_or(false);
        let stays_root = self.is_root && !switches_user;
        // '+' commands skip the sandbox, NoNewPrivileges= included (systemd
        // sets it only with needs_sandboxing).
        let nnp = !cmd.privileged && sandbox::no_new_privileges(&self.unit, stays_root);
        if nnp {
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        }

        let bset = if self.is_root && !cmd.privileged { sandbox::bounding_set(&self.unit) } else { None };
        if let Some(bset) = bset {
            if let Err(e) = sandbox::drop_bounding_set(bset) {
                return (EXIT_CAPABILITIES, e);
            }
        }

        // As systemd, ambient capabilities outside the bounding set are left
        // out: capset() would refuse them and fail the start.
        let ambient = sandbox::parse_capset(&self.unit, "AmbientCapabilities").map(|a| bset.map_or(a, |b| a & b));
        let switch_ids = !cmd.privileged && !cmd.no_setuid && self.is_root;
        if let (Some(u), true) = (&self.user, switch_ids) {
            if u.uid != 0 {
                if ambient.is_some() {
                    unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 1, 0, 0, 0) };
                }
                let gid = self.gid.unwrap_or(u.gid);
                let groups: Vec<libc::gid_t> = self.groups.clone();
                unsafe {
                    if libc::setgroups(groups.len(), groups.as_ptr()) < 0 {
                        return (EXIT_GROUP, format!("setgroups: {}", errno()));
                    }
                    if libc::setresgid(gid, gid, gid) < 0 {
                        return (EXIT_GROUP, format!("setgid {gid}: {}", errno()));
                    }
                    if libc::setresuid(u.uid, u.uid, u.uid) < 0 {
                        return (EXIT_USER, format!("setuid {}: {}", u.uid, errno()));
                    }
                }
            } else if let Some(g) = self.gid {
                unsafe { libc::setresgid(g, g, g) };
            }
        } else if self.user.is_none() && switch_ids {
            // Group=/SupplementaryGroups= without User=.
            unsafe {
                if !self.groups.is_empty() && libc::setgroups(self.groups.len(), self.groups.as_ptr()) < 0 {
                    return (EXIT_GROUP, format!("setgroups: {}", errno()));
                }
                if let Some(g) = self.gid {
                    if libc::setresgid(g, g, g) < 0 {
                        return (EXIT_GROUP, format!("setgid {g}: {}", errno()));
                    }
                }
            }
        }
        // Ambient capabilities matter only after dropping root; a root service
        // keeps its full capability set (raising "ambient" would shrink it).
        let non_root_user = self.user.as_ref().map(|u| u.uid != 0).unwrap_or(false);
        if let (Some(caps), true, true) = (ambient, switch_ids, non_root_user) {
            if let Err(e) = sandbox::raise_ambient(caps) {
                return (EXIT_CAPABILITIES, e);
            }
        }

        // Final environment and argv.
        let env = match self.unit_env(extra_env) {
            Ok(env) => env,
            Err(e) => return (EXIT_MEMORY, e),
        };
        let lookup = |k: &str| env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone());
        let argv = if cmd.no_env_expand {
            cmd.argv.clone()
        } else {
            execcmd::expand_argv(&cmd.argv, &lookup)
        };
        let path = execcmd::resolve_path(&cmd.path);

        let c_path = cstr(&path);
        let c_argv: Vec<CString> = argv.iter().map(|a| cstr(a)).collect();
        let mut p_argv: Vec<*const libc::c_char> = c_argv.iter().map(|c| c.as_ptr()).collect();
        p_argv.push(std::ptr::null());
        let c_env: Vec<CString> = env.iter().map(|(k, v)| cstr(&format!("{k}={v}"))).collect();
        let mut p_env: Vec<*const libc::c_char> = c_env.iter().map(|c| c.as_ptr()).collect();
        p_env.push(std::ptr::null());
        // Last, as systemd does: from here on only execve() has to be allowed.
        if !cmd.privileged {
            if let Err(e) = crate::seccomp::apply_restrictions(&self.unit) {
                return (EXIT_SECCOMP, e);
            }
            match crate::seccomp::Filter::from_unit(&self.unit) {
                Ok(Some(f)) => {
                    if let Err(e) = f.apply() {
                        return (EXIT_SECCOMP, e);
                    }
                }
                Ok(None) => {}
                Err(e) => return (EXIT_SECCOMP, format!("SystemCallFilter=: {e}")),
            }
        }
        unsafe { libc::execve(c_path.as_ptr(), p_argv.as_ptr(), p_env.as_ptr()) };
        (EXIT_EXEC, format!("exec {path}: {}", errno()))
    }

    /// Connect to journald's stdout stream (the protocol PID 1 uses). journald
    /// attributes the stream to the unit through our cgroup.
    fn journal_stream(&self, cmd: &ExecCommand) -> Option<i32> {
        if !self.cfg.journal || self.unit.id == "systemd-journald.service" {
            return None;
        }
        let path_buf = self.cfg.journal_stdout.display().to_string();
        let path = path_buf.as_str();
        if !Path::new(path).exists() || path.len() >= 100 {
            return None;
        }
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return None;
        }
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as u16;
        for (i, b) in path.bytes().enumerate() {
            addr.sun_path[i] = b as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + path.len() + 1) as u32;
        if unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) } < 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        unsafe { libc::shutdown(fd, libc::SHUT_RD) };
        let ident = self.unit.get("Service", "SyslogIdentifier").unwrap_or_else(|| {
            let a0 = cmd.argv.first().map(String::as_str).unwrap_or(&cmd.path);
            a0.rsplit('/').next().unwrap_or(a0).to_string()
        });
        let prio = self
            .unit
            .get("Service", "SyslogLevel")
            .map(|l| match l.as_str() {
                "emerg" => 0,
                "alert" => 1,
                "crit" => 2,
                "err" => 3,
                "warning" => 4,
                "notice" => 5,
                "debug" => 7,
                _ => 6,
            })
            .unwrap_or(6);
        let prefix = i32::from(self.unit.get_bool("Service", "SyslogLevelPrefix", true));
        let header = format!("{ident}\n{}\n{prio}\n{prefix}\n0\n0\n0\n", self.unit.id);
        let r = unsafe { libc::write(fd, header.as_ptr() as *const libc::c_void, header.len()) };
        if r != header.len() as isize {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(fd)
    }

    fn setup_stdio(&self, cmd: &ExecCommand) -> Result<(), (i32, String)> {
        let input = self.unit.get("Service", "StandardInput").unwrap_or_else(|| "null".into());
        let default_out = if input.starts_with("tty") { "inherit" } else { "journal" };
        let output = self.unit.get("Service", "StandardOutput").unwrap_or_else(|| default_out.into());
        let error = self.unit.get("Service", "StandardError").unwrap_or_else(|| "inherit".into());

        // fd:NAME from systemd-run --pipe/--stdio (see daemon::transient).
        // Taken once: the store's copies would keep the caller's pipes open.
        let stdio = if [&input, &output, &error].iter().any(|s| s.starts_with("fd:")) {
            let store = self.cfg.run_dir.join("fdstore");
            let key = format!("{}#stdio", self.unit.id);
            let fds = crate::fdstore::get(&store, &key);
            crate::fdstore::del(&store, &key, None);
            fds
        } else {
            Vec::new()
        };
        let stdio_fd = |spec: &str| {
            use std::os::fd::AsRawFd;
            let name = spec.strip_prefix("fd:")?;
            stdio.iter().find(|(n, _)| n == name).map(|(_, f)| f.as_raw_fd())
        };

        let open =|path: &str, flags: i32| -> Result<i32, String> {
            let c = cstr(path);
            let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC, 0o644) };
            if fd < 0 {
                Err(format!("open {path}: {}", errno()))
            } else {
                Ok(fd)
            }
        };
        let place = |fd: i32, target: i32| unsafe {
            if fd != target {
                libc::dup2(fd, target);
                libc::close(fd);
            } else {
                let fl = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, fl & !libc::FD_CLOEXEC);
            }
        };

        let wants_journal = |x: &str| x.starts_with("journal") || x.starts_with("kmsg") || x == "syslog";
        let journal = if wants_journal(&output) || wants_journal(&error) { self.journal_stream(cmd) } else { None };
        let mut tty_fd: Option<i32> = None;
        if input.starts_with("tty") || output == "tty" || error == "tty" {
            tty_fd = Some(self.open_tty(input == "tty-force").map_err(|e| (EXIT_STDIN, e))?);
        }

        match input.as_str() {
            "null" | "" => place(open("/dev/null", libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?, 0),
            s if s.starts_with("tty") => unsafe {
                libc::dup2(tty_fd.unwrap(), 0);
            },
            s if s.starts_with("file:") => {
                place(open(&s[5..], libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?, 0)
            }
            s if stdio_fd(s).is_some() => unsafe {
                libc::dup2(stdio_fd(s).unwrap(), 0);
            },
            _ => place(open("/dev/null", libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?, 0),
        }

        let redirect = |spec: &str, target: i32, code: i32| -> Result<(), (i32, String)> {
            match spec {
                "null" => place(open("/dev/null", libc::O_WRONLY).map_err(|e| (code, e))?, target),
                "tty" => unsafe {
                    libc::dup2(tty_fd.unwrap(), target);
                },
                "inherit" if input.starts_with("tty") && target == 1 => unsafe {
                    libc::dup2(0, 1);
                },
                s if s.starts_with("file:") => place(
                    open(&s[5..], libc::O_WRONLY | libc::O_CREAT).map_err(|e| (code, e))?,
                    target,
                ),
                s if s.starts_with("append:") => place(
                    open(&s[7..], libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND).map_err(|e| (code, e))?,
                    target,
                ),
                s if s.starts_with("truncate:") => place(
                    open(&s[9..], libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC).map_err(|e| (code, e))?,
                    target,
                ),
                s if stdio_fd(s).is_some() => unsafe {
                    libc::dup2(stdio_fd(s).unwrap(), target);
                },
                // journal, kmsg, syslog: journald's stream if available,
                // otherwise keep the s6 log pipe.
                s if s.starts_with("journal") || s.starts_with("kmsg") || s == "syslog" || s == "inherit" => {
                    if let Some(fd) = journal.filter(|_| s != "inherit") {
                        unsafe { libc::dup2(fd, target) };
                    }
                }
                _ => {}
            }
            Ok(())
        };
        redirect(&output, 1, EXIT_STDOUT)?;
        if error == "inherit" {
            unsafe { libc::dup2(1, 2) };
        } else {
            redirect(&error, 2, EXIT_STDOUT)?;
        }
        if let Some(fd) = tty_fd {
            if fd > 2 {
                unsafe { libc::close(fd) };
            }
        }
        if let Some(fd) = journal {
            unsafe { libc::close(fd) };
        }
        Ok(())
    }

    fn open_tty(&self, force: bool) -> Result<i32, String> {
        let path = self.tty_path();
        let c = cstr(&path);
        unsafe {
            libc::setsid();
            if self.unit.get_bool("Service", "TTYVHangup", false) {
                let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
                if fd >= 0 {
                    let old = libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    libc::ioctl(fd, libc::TIOCSCTTY, 1);
                    libc::vhangup();
                    libc::signal(libc::SIGHUP, old);
                    libc::close(fd);
                }
            }
            let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if fd < 0 {
                return Err(format!("open {path}: {}", errno()));
            }
            if libc::ioctl(fd, libc::TIOCSCTTY, i32::from(force)) < 0 {
                libc::close(fd);
                return Err(format!("TIOCSCTTY {path}: {}", errno()));
            }
            Ok(fd)
        }
    }

    fn apply_limits(&self) -> Result<(), String> {
        if let Some(m) = self.unit.get("Service", "UMask").and_then(|m| u32::from_str_radix(&m, 8).ok()) {
            unsafe { libc::umask(m) };
        } else {
            unsafe { libc::umask(0o022) };
        }
        if let Some(n) = self.unit.get("Service", "Nice").and_then(|n| n.parse::<i32>().ok()) {
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, n) };
        }
        if let Some(adj) = self.unit.get("Service", "OOMScoreAdjust") {
            let _ = fs::write("/proc/self/oom_score_adj", adj);
        }
        for (key, res, bytes) in LIMITS {
            let Some(v) = self.unit.get("Service", key) else { continue };
            let parse = |s: &str| -> Option<u64> {
                let s = s.trim();
                if s == "infinity" {
                    return Some(libc::RLIM_INFINITY);
                }
                if *bytes {
                    let (n, m) = match s.chars().last()? {
                        'K' => (&s[..s.len() - 1], 1u64 << 10),
                        'M' => (&s[..s.len() - 1], 1 << 20),
                        'G' => (&s[..s.len() - 1], 1 << 30),
                        'T' => (&s[..s.len() - 1], 1 << 40),
                        _ => (s, 1),
                    };
                    return n.parse::<u64>().ok().map(|n| n * m);
                }
                if *res == libc::RLIMIT_CPU {
                    return crate::unit::parse_timespan_usec(s, 1_000_000).map(|u| u / 1_000_000);
                }
                s.parse().ok()
            };
            let (soft, hard) = match v.split_once(':') {
                Some((s, h)) => (parse(s), parse(h)),
                None => (parse(&v), parse(&v)),
            };
            let (Some(soft), Some(hard)) = (soft, hard) else {
                return Err(format!("bad {key}={v}"));
            };
            let rl = libc::rlimit { rlim_cur: soft, rlim_max: hard };
            if unsafe { libc::setrlimit(*res as _, &rl) } < 0 {
                return Err(format!("setrlimit {key}: {}", errno()));
            }
        }
        Ok(())
    }

    /// fork + exec a command and wait for it.
    fn spawn_wait(&self, cmd: &ExecCommand, extra_env: &[(String, String)]) -> Status {
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let (code, msg) = self.exec_command(cmd, extra_env);
            log(self.id(), &msg);
            unsafe { libc::_exit(code) };
        }
        if pid < 0 {
            log(self.id(), &format!("fork: {}", errno()));
            return Status::Exited(EXIT_EXEC);
        }
        let Some(deadline) = self.deadline.get() else {
            return Status::from_wait(wait_pid(pid));
        };
        // TimeoutStartSec= (TimeoutStopSec=): SIGTERM at the deadline,
        // SIGKILL 5 s later.
        let mut status = 0;
        let mut killed_at: Option<std::time::Instant> = None;
        loop {
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if r == pid {
                return Status::from_wait(status);
            }
            if r < 0 {
                return Status::Exited(0);
            }
            let now = std::time::Instant::now();
            match killed_at {
                None if now >= deadline => {
                    log(self.id(), &format!("{} timeout, terminating", self.deadline_what.get()));
                    self.timed_out.set(true);
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    killed_at = Some(now);
                }
                Some(t) if now >= t + std::time::Duration::from_secs(5) => {
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn start_timeout(&self) -> Option<std::time::Duration> {
        let u = self.unit.start_timeout_usec();
        (u != u64::MAX).then(|| std::time::Duration::from_micros(u))
    }

    /// A required unit (Requires=/Requisite=/BindsTo=) that failed. s6-rc
    /// only orders units; requirement failures are enforced here.
    fn failed_requirement(&self) -> Option<String> {
        let ld = Loader::new(&self.cfg);
        let idx = crate::index::Index::load(&self.cfg);
        let mut deps = self.unit.get_words("Unit", "Requires");
        deps.extend(self.unit.get_words("Unit", "Requisite"));
        deps.extend(self.unit.get_words("Unit", "BindsTo"));
        deps.extend(ld.dir_deps(&self.unit, "requires"));
        for d in deps {
            let d = idx.resolve(&d).to_string();
            if d == self.unit.id {
                continue;
            }
            let st = crate::state::for_unit(&self.cfg.run_dir, &d);
            if st.has(keys::START_FAILED) || st.has(keys::DEPENDENCY_FAILED) {
                return Some(d);
            }
            let failed = st.get(keys::RESULT).map(|r| !matches!(r.as_str(), "success" | "running")).unwrap_or(false);
            if failed && st.get_u64(keys::INACTIVE_ENTER) >= st.get_u64(keys::INACTIVE_EXIT) {
                return Some(d);
            }
        }
        None
    }

    /// Start failed: record it, but still tell s6-rc "ready" so units that
    /// merely want this one are not blocked (systemd semantics).
    fn soft_fail(&mut self, reason: &str) {
        log(self.id(), reason);
        self.st.set(keys::START_FAILED, reason);
        self.signal_ready();
    }

    /// Run a list of Exec*= settings in order. Returns false on a non-ignored failure.
    fn run_list(&self, key: &str, extra_env: &[(String, String)]) -> Result<(), Status> {
        for line in self.unit.get_all("Service", key) {
            let cmds = match execcmd::parse(&line) {
                Ok(c) => c,
                Err(e) => {
                    log(self.id(), &format!("{key}: {e}"));
                    return Err(Status::Exited(EXIT_EXEC));
                }
            };
            for cmd in cmds {
                let st = self.spawn_wait(&cmd, extra_env);
                // A oneshot's ExecStart= processes are its main processes:
                // SuccessExitStatus= applies to them (not to Pre/Post).
                let success = key == "ExecStart" && status_in(&self.unit.get_words("Service", "SuccessExitStatus"), st);
                if !st.ok() && !success && !cmd.ignore_failure {
                    log(self.id(), &format!("{key}={} failed: {}", cmd.path, describe(st)));
                    return Err(st);
                }
            }
        }
        Ok(())
    }

    fn signal_ready(&mut self) {
        if let Some(fd) = self.notify_fd.take() {
            unsafe {
                libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
                libc::close(fd);
            }
        }
    }

    /// s6 hands us the notification pipe on fd 3; move it out of the way so
    /// fds 3.. are free for LISTEN_FDS.
    fn take_notify_fd(&mut self) {
        unsafe {
            if libc::fcntl(3, libc::F_GETFD) >= 0 {
                let fd = libc::fcntl(3, libc::F_DUPFD_CLOEXEC, 64);
                libc::close(3);
                if fd >= 0 {
                    self.notify_fd = Some(fd);
                }
            }
        }
    }

    fn enter_cgroup(&self) {
        let pid = std::process::id();
        let owner = self.user.as_ref().map(|u| (u.uid, self.gid.unwrap_or(u.gid)));
        match cgroup::enter(&self.unit, pid, owner) {
            Ok(dir) => {
                self.st.set(keys::CGROUP, &dir.display().to_string());
                // Before anything else runs in the cgroup, ExecStartPre= included.
                if self.is_root {
                    for m in crate::bpf::apply(&self.unit, &dir) {
                        log(self.id(), &m);
                    }
                }
            }
            Err(e) => {
                if self.is_root {
                    log(self.id(), &format!("cgroup: {e}"));
                }
            }
        }
    }

    fn cgroup_dir(&self) -> Option<PathBuf> {
        self.st.get(keys::CGROUP).map(PathBuf::from)
    }

    /// MEMORY_PRESSURE_WATCH=/WRITE= for the cgroup entered last. A User=
    /// service gets the file, so it can write its trigger, as under systemd.
    fn memory_pressure_env(&mut self) {
        for (k, v) in cgroup::memory_pressure_env(&self.unit, self.cgroup_dir().as_deref()) {
            if k == "MEMORY_PRESSURE_WATCH" && v != "/dev/null" && self.is_root {
                if let Some(u) = self.user.as_ref().filter(|u| u.uid != 0) {
                    let p = cstr(&v);
                    unsafe { libc::chown(p.as_ptr(), u.uid, self.gid.unwrap_or(u.gid)) };
                }
            }
            self.setenv(&k, &v);
        }
    }

    fn prepare_start(&mut self) -> Result<(), String> {
        for k in [
            keys::CONDITION_FAILED, keys::ASSERT_FAILED, keys::STATUS_TEXT, keys::MAIN_PID,
            keys::START_FAILED, keys::DEPENDENCY_FAILED, keys::PARKED, keys::EXTEND_UNTIL,
            // A new run starts with a clean result, as systemd's
            // service_start() does; otherwise the exit handler keeps a
            // previous run's failure.
            keys::RESULT,
        ] {
            self.st.clear(k);
        }
        let inv = util::read_trimmed(Path::new("/proc/sys/kernel/random/uuid"))
            .unwrap_or_default()
            .replace('-', "");
        self.st.set(keys::INVOCATION, &inv);
        // journald reads the invocation ID from this xattr on the unit's cgroup
        // (systemctl status uses it to pick the current run's log lines).
        if let Some(dir) = self.cgroup_dir() {
            let bytes: Vec<u8> = (0..inv.len() / 2).filter_map(|i| u8::from_str_radix(&inv[i * 2..i * 2 + 2], 16).ok()).collect();
            let p = cstr(&dir.display().to_string());
            unsafe {
                libc::setxattr(p.as_ptr(), c"trusted.invocation_id".as_ptr(), bytes.as_ptr() as *const libc::c_void, bytes.len(), 0);
            }
        }
        self.setenv("INVOCATION_ID", &inv);
        self.memory_pressure_env();
        self.st.stamp(keys::INACTIVE_EXIT);
        self.create_directories()?;
        self.sandbox.prepare()?;
        Ok(())
    }

    /// Conditions and ExecCondition=. Ok(false) means "skip".
    fn check_conditions(&self) -> Result<bool, String> {
        let ev = conditions::evaluate(&self.unit);
        if !ev.asserts_ok {
            let why = ev.failure.unwrap_or_default();
            self.st.set(keys::ASSERT_FAILED, &why);
            return Err(format!("assertion failed: {why}"));
        }
        if !ev.conditions_ok {
            self.st.set(keys::CONDITION_FAILED, &ev.failure.unwrap_or_default());
            return Ok(false);
        }
        for line in self.unit.get_all("Service", "ExecCondition") {
            for cmd in execcmd::parse(&line)? {
                match self.spawn_wait(&cmd, &[]) {
                    Status::Exited(0) => {}
                    Status::Exited(c) if (1..255).contains(&c) => {
                        self.st.set(keys::CONDITION_FAILED, &format!("ExecCondition={} exited {c}", cmd.path));
                        return Ok(false);
                    }
                    other => return Err(format!("ExecCondition failed: {}", describe(other))),
                }
            }
        }
        Ok(true)
    }

    fn service_type(&self) -> String {
        self.unit.get("Service", "Type").unwrap_or_else(|| {
            if self.unit.file.get("Service", "ExecStart").is_none() {
                "oneshot".into()
            } else if self.unit.file.get("Service", "BusName").is_some() {
                "dbus".into()
            } else {
                "simple".into()
            }
        })
    }

    fn main_command(&self) -> Result<ExecCommand, String> {
        let lines = self.unit.get_all("Service", "ExecStart");
        if lines.len() != 1 {
            return Err(format!("expected exactly one ExecStart= for a long-running service, found {}", lines.len()));
        }
        execcmd::parse(&lines[0])?
            .into_iter()
            .next()
            .ok_or_else(|| "empty ExecStart=".into())
    }

    /// Socket units that hand fds to this service.
    fn socket_units(&self) -> Vec<Unit> {
        let ld = Loader::new(&self.cfg);
        let mut names = self.unit.get_words("Service", "Sockets");
        if names.is_empty() {
            let stem = self.unit.id.trim_end_matches(".service");
            let default = format!("{stem}.socket");
            let wanted = self.unit.get_words("Unit", "Requires").contains(&default)
                || self.unit.get_words("Unit", "Wants").contains(&default);
            if wanted || install::is_enabled(&self.cfg, &default) || socket::is_requested(&self.cfg.run_dir, &default) {
                names.push(default);
            }
            // Sockets that name this service with Service= (e.g. systemd-logind-varlink.socket).
            for dir in &self.cfg.unit_path {
                let Ok(rd) = fs::read_dir(dir) else { continue };
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if !n.ends_with(".socket") || names.contains(&n) {
                        continue;
                    }
                    let Ok(text) = fs::read_to_string(e.path()) else { continue };
                    if !text.contains("Service=") {
                        continue;
                    }
                    let s = ld.load(&n);
                    if s.get("Socket", "Service").map(|v| self.unit.names.contains(&v)).unwrap_or(false)
                        && (install::is_enabled(&self.cfg, &n) || socket::is_requested(&self.cfg.run_dir, &n))
                    {
                        names.push(n);
                    }
                }
            }
        }
        names
            .into_iter()
            .map(|n| ld.load(&n))
            .filter(|u| u.is_loaded())
            .collect()
    }

    /// Bind the socket units' sockets and leave them with the fd store.
    fn bind_sockets(&self, socks: &[Unit]) -> Result<Vec<(String, std::os::fd::OwnedFd)>, String> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let store = self.cfg.run_dir.join("fdstore");
        let key = format!("{}#sockets", self.unit.id);
        // As systemd: the sockets carry the label of the service's domain.
        let label = self
            .main_command()
            .ok()
            .and_then(|m| crate::selinux::exec_label(&execcmd::resolve_path(&m.path)));
        crate::selinux::set_socket_label(label.as_deref());
        let bound: Result<Vec<_>, String> = socks.iter().map(socket::bind_all).collect();
        crate::selinux::set_socket_label(None);
        let mut fds = Vec::new();
        for b in bound?.into_iter().flatten() {
            let owned = unsafe { OwnedFd::from_raw_fd(b.fd) };
            // Best effort: without the daemon we just rebind next time.
            let _ = crate::fdstore::put(&store, &key, &b.name, owned.as_raw_fd(), crate::fdstore::SOCKETS_MAX);
            fds.push((b.name, owned));
        }
        Ok(fds)
    }

    /// The first start of an on-demand service: listen on its sockets
    /// (held by the fd store) and wait for activity, like systemd's socket
    /// unit in front of an inactive service. Not when s6 restarts one that
    /// ran (Restart=, s6-rc-update) or a job asked for the start; and
    /// without the fd store it just starts.
    fn should_wait_on_demand(&self) -> bool {
        if !self.on_demand || self.st.has(keys::ACTIVATED) || self.st.take(keys::START_REQUESTED) {
            return false;
        }
        let socks = self.socket_units();
        if socks.is_empty() {
            return false;
        }
        let store = self.cfg.run_dir.join("fdstore");
        let key = format!("{}#sockets", self.unit.id);
        if crate::fdstore::get(&store, &key).is_empty() {
            // Dropped here; the store keeps its own copies.
            if let Err(e) = self.bind_sockets(&socks) {
                log(self.id(), &format!("on demand: {e}; starting it now"));
                return false;
            }
        }
        !crate::fdstore::get(&store, &key).is_empty()
    }

    /// Sockets (kept open by the daemon across restarts) and FDSTORE=
    /// descriptors, arranged on fds 3.. for the final exec.
    fn setup_listen_fds(&mut self) -> Result<(), String> {
        use std::os::fd::{IntoRawFd, OwnedFd};
        let store = self.cfg.run_dir.join("fdstore");
        let mut fds: Vec<(String, OwnedFd)> = Vec::new();
        let socks = self.socket_units();
        if !socks.is_empty() {
            let held = crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id));
            fds = if held.is_empty() { self.bind_sockets(&socks)? } else { held };
        }
        let max: usize = self
            .unit
            .get("Service", "FileDescriptorStoreMax")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if max > 0 {
            fds.extend(crate::fdstore::get(&store, &self.unit.id));
        }
        if fds.is_empty() {
            return Ok(());
        }
        // Move every fd above the target range first to avoid clobbering.
        let mut high = Vec::new();
        let mut names = Vec::new();
        for (name, f) in fds {
            let raw = f.into_raw_fd();
            let fd = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 100) };
            unsafe { libc::close(raw) };
            high.push(fd);
            names.push(name);
        }
        for (i, fd) in high.iter().enumerate() {
            unsafe {
                libc::dup2(*fd, 3 + i as i32);
                libc::close(*fd);
            }
        }
        self.setenv("LISTEN_FDS", &high.len().to_string());
        self.setenv("LISTEN_PID", &std::process::id().to_string());
        self.setenv("LISTEN_FDNAMES", &names.join(":"));
        Ok(())
    }

    /// The service exited on its own (systemd-rfkill after a few idle
    /// seconds): like systemd's socket unit, wait for activity on the sockets
    /// the daemon holds, then start it again. Ready for s6 meanwhile, and the
    /// daemon reports the unit inactive. SIGTERM (stop) ends the wait; the
    /// daemon starts it at once by removing SOCKET_WAIT and sending SIGUSR1
    /// (`s6-svc -1`). SIGUSR1 is blocked except inside ppoll(), so a wakeup
    /// between the check and the wait is not lost.
    fn wait_for_sockets(&self) {
        use std::os::fd::AsRawFd;
        unsafe {
            let mut usr1: libc::sigset_t = std::mem::zeroed();
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut usr1);
            libc::sigaddset(&mut usr1, libc::SIGUSR1);
            libc::sigprocmask(libc::SIG_BLOCK, &usr1, &mut old);
            libc::signal(libc::SIGUSR1, wake_signal as *const () as libc::sighandler_t);
            let mut during = old;
            libc::sigdelset(&mut during, libc::SIGUSR1);

            let store = self.cfg.run_dir.join("fdstore");
            let held = crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id));
            let mut fds: Vec<libc::pollfd> = held
                .iter()
                .map(|(_, f)| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 })
                .collect();
            // Nothing held (the daemon is not reachable): start it now.
            while !fds.is_empty() && self.st.has(keys::SOCKET_WAIT) {
                let r = libc::ppoll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, std::ptr::null(), &during);
                if r > 0 || (r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)) {
                    break;
                }
            }
            // The daemon removed the key first: its SIGUSR1 may still be on
            // the way, and must not reach the service after exec.
            if !self.st.take(keys::SOCKET_WAIT) && !WOKEN.load(std::sync::atomic::Ordering::SeqCst) {
                let ts = libc::timespec { tv_sec: 2, tv_nsec: 0 };
                libc::sigtimedwait(&usr1, std::ptr::null_mut(), &ts);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
            libc::signal(libc::SIGUSR1, libc::SIG_DFL);
        }
    }

    // ---- subcommands ----

    /// longrun `run` script.
    pub fn cmd_run(mut self) -> i32 {
        self.take_notify_fd();
        if self.should_wait_on_demand() {
            self.st.set(keys::SOCKET_WAIT, "1");
        }
        if self.st.has(keys::SOCKET_WAIT) {
            // Ready for s6 while waiting, like a listening socket unit: an
            // `s6-rc -u change` recording a unit started beside the boot's
            // change would otherwise wait for readiness until its timeout,
            // then bring it down. The daemon's start waits on the unit state.
            self.signal_ready();
            self.wait_for_sockets();
            self.start_waiting_requirements();
        }
        self.st.set(keys::ACTIVATED, "1");
        self.st.clear(keys::START_REQUESTED);
        self.enter_cgroup();
        if let Err(e) = self.prepare_start() {
            self.soft_fail(&e);
            return 1;
        }
        if let Some(dep) = self.failed_requirement() {
            // Like systemd's "dependency" job result: not started, not failed.
            log(self.id(), &format!("not starting: required unit {dep} failed"));
            self.st.set(keys::DEPENDENCY_FAILED, &dep);
            self.signal_ready();
            loop {
                unsafe { libc::pause() };
            }
        }
        match self.check_conditions() {
            Ok(true) => {}
            Ok(false) => {
                // Stay "up" for s6 but report the unit as skipped.
                log(self.id(), "condition check failed, not starting");
                self.signal_ready();
                loop {
                    unsafe { libc::pause() };
                }
            }
            Err(e) => {
                self.st.set(keys::RESULT, "exit-code");
                self.soft_fail(&e);
                return 1;
            }
        }
        self.deadline.set(self.start_timeout().map(|t| std::time::Instant::now() + t));
        if let Err(st) = self.run_list("ExecStartPre", &[]) {
            self.st.set(keys::RESULT, if self.timed_out.get() { "timeout" } else { result_for(st) });
            self.soft_fail("ExecStartPre= failed");
            return exit_code_for(st);
        }
        let main = match self.main_command() {
            Ok(c) => c,
            Err(e) => {
                self.soft_fail(&e);
                return EXIT_EXEC;
            }
        };
        let ty = self.service_type();
        if ty == "forking" {
            return self.run_forking(&main);
        }
        self.deadline.set(None);

        if let Err(e) = self.setup_listen_fds() {
            self.st.set(keys::RESULT, "resources");
            self.soft_fail(&e);
            return 1;
        }

        let pid = std::process::id();
        self.st.set(keys::MAIN_PID, &pid.to_string());
        self.st.stamp(keys::EXEC_MAIN_START);

        match ty.as_str() {
            "notify" | "notify-reload" => match self.start_notify_helper(pid) {
                Ok(sock) => self.setenv("NOTIFY_SOCKET", &sock),
                Err(e) => {
                    self.soft_fail(&e);
                    return 1;
                }
            },
            "dbus" => self.start_dbus_helper(pid),
            _ => {
                if let Err(e) = self.start_exec_helper() {
                    self.soft_fail(&e);
                    return 1;
                }
            }
        }
        // The notify fd now belongs to the helper.
        if let Some(fd) = self.notify_fd.take() {
            unsafe { libc::close(fd) };
        }
        if let (Some(service), true) = (self.unit.get("Service", "PAMName"), self.is_root) {
            return self.exec_with_pam(&service, &main);
        }
        let (code, msg) = self.exec_command(&main, &[]);
        log(self.id(), &msg);
        if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.get()) {
            let b = (code as u32).to_ne_bytes();
            unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 4) };
        }
        code
    }

    /// PAMName=: open a PAM session here, run the service in a child, and
    /// stay around (forwarding signals) to close the session afterwards.
    fn exec_with_pam(&self, service: &str, main: &ExecCommand) -> i32 {
        const EXIT_PAM: i32 = 224;
        let user = self.user.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "root".into());
        let tty = self.uses_tty().then(|| self.tty_path());
        let (session, env) = match crate::pam::Session::open(service, &user, tty.as_deref()) {
            Ok(x) => x,
            Err(e) => {
                log(self.id(), &format!("PAM: {e}"));
                if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.get()) {
                    let b = (EXIT_PAM as u32).to_ne_bytes();
                    unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 4) };
                }
                return EXIT_PAM;
            }
        };
        // Forwarding handlers in place and every signal blocked until
        // FORWARD_PID names the child: a stop arriving in between is
        // forwarded, not fatal to the wrapper alone. exec_command() resets
        // both in the child.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::sigprocmask(libc::SIG_BLOCK, &all, &mut old);
            for sig in 1..=libc::SIGRTMAX() {
                if ![libc::SIGKILL, libc::SIGSTOP, libc::SIGCHLD, libc::SIGSEGV, libc::SIGBUS, libc::SIGFPE, libc::SIGILL]
                    .contains(&sig)
                {
                    libc::signal(sig, forward_signal as *const () as libc::sighandler_t);
                }
            }
        }
        let child = unsafe { libc::fork() };
        if child == 0 {
            let (code, msg) = self.exec_command(main, &env);
            log(self.id(), &msg);
            if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.get()) {
                let b = (code as u32).to_ne_bytes();
                unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 4) };
            }
            unsafe { libc::_exit(code) };
        }
        if child < 0 {
            log(self.id(), &format!("fork: {}", errno()));
            unsafe { libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
            session.close();
            return EXIT_EXEC;
        }
        FORWARD_PID.store(child, std::sync::atomic::Ordering::SeqCst);
        unsafe { libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
        // Our copy of the exec-status pipe must go, or the readiness helper
        // never sees the child's exec.
        if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.take()) {
            unsafe { libc::close(fd) };
        }
        self.st.set(keys::MAIN_PID, &child.to_string());
        let status = Status::from_wait(wait_pid(child));
        let t = std::time::Instant::now();
        session.close();
        if t.elapsed() >= std::time::Duration::from_secs(2) {
            log(self.id(), &format!("closing the PAM session took {:.1} s", t.elapsed().as_secs_f64()));
        }
        match status {
            Status::Exited(c) => c,
            Status::Killed(sig) => {
                unsafe {
                    libc::signal(sig, libc::SIG_DFL);
                    libc::kill(libc::getpid(), sig);
                }
                128 + sig
            }
        }
    }

    /// OnSuccess=/OnFailure= and SuccessAction=/FailureAction= after the unit finished.
    /// StartUnit for each of `units` through the daemon, detached: the
    /// daemon may be busy with the very job that ran us.
    fn start_units_detached(&self, why: &str, units: Vec<String>) {
        if units.is_empty() {
            return;
        }
        let cfg = self.cfg.clone();
        let id = self.unit.id.clone();
        let run = || -> Result<(), String> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(async {
                let conn = crate::client::connect(&cfg).await?;
                for u in &units {
                    if let Err(e) = crate::client::manager_call(&conn, "StartUnit", &(u.as_str(), "replace")).await {
                        eprintln!("uinitconverter[{id}]: {why}={u}: {e}");
                    }
                }
                Ok(())
            })
        };
        let _ = double_fork(|| {
            if let Err(e) = run() {
                log(self.id(), &format!("{why}: {e}"));
            }
        });
    }

    /// Leaving the socket wait: requirements that are on demand too and
    /// still wait for their sockets start with this one, as systemd starts
    /// them (iscsiuio Requires=iscsid). Their sockets already listen, so
    /// this one need not wait for them.
    fn start_waiting_requirements(&self) {
        let ld = Loader::new(&self.cfg);
        let mut names = Vec::new();
        for key in ["Requires", "BindsTo", "Wants"] {
            names.extend(self.unit.get_words("Unit", key));
        }
        names.extend(ld.dir_deps(&self.unit, "requires"));
        names.extend(ld.dir_deps(&self.unit, "wants"));
        let mut ids: Vec<String> = Vec::new();
        for n in names {
            let id = ld.load(&n).id;
            if id != self.unit.id
                && !ids.contains(&id)
                && crate::state::for_unit(&self.cfg.run_dir, &id).has(keys::SOCKET_WAIT)
            {
                ids.push(id);
            }
        }
        self.start_units_detached("on demand", ids);
    }

    fn on_result(&self, success: bool) {
        let (units_key, action_key) = if success { ("OnSuccess", "SuccessAction") } else { ("OnFailure", "FailureAction") };
        let units: Vec<String> = self
            .unit
            .get_words("Unit", units_key)
            .into_iter()
            .map(|u| u.replace("%n", &self.unit.id))
            .collect();
        self.start_units_detached(units_key, units);
        if let Some(action) = self.unit.get("Unit", action_key) {
            let cmd = match action.as_str() {
                "none" => None,
                a if a.starts_with("reboot") || a == "soft-reboot" || a == "kexec" => Some(self.cfg.reboot_cmd.clone()),
                a if a.starts_with("poweroff") || a.starts_with("exit") => Some(self.cfg.poweroff_cmd.clone()),
                a if a.starts_with("halt") => Some(self.cfg.halt_cmd.clone()),
                other => {
                    log(self.id(), &format!("{action_key}={other} is not supported"));
                    None
                }
            };
            if let Some(cmd) = cmd {
                log(self.id(), &format!("{action_key}={action}: running {cmd}"));
                let _ = std::process::Command::new("/bin/sh").arg("-c").arg(cmd).spawn();
            }
        }
    }

    fn after_ready(&self, main_pid: u32) {
        self.st.stamp(keys::ACTIVE_ENTER);
        if let Err(st) = self.run_list("ExecStartPost", &[("MAINPID".into(), main_pid.to_string())]) {
            self.st.set(keys::RESULT, result_for(st));
            unsafe { libc::kill(main_pid as i32, libc::SIGTERM) };
        }
    }

    /// Type=simple/exec: ready once exec() succeeds (detected via a CLOEXEC pipe).
    fn start_exec_helper(&mut self) -> Result<(), String> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(format!("pipe: {}", errno()));
        }
        let (r, w) = (fds[0], fds[1]);
        let main_pid = std::process::id();
        let notify = self.notify_fd;
        double_fork(|| {
            unsafe { libc::close(w) };
            let mut buf = [0u8; 4];
            let n = unsafe { libc::read(r, buf.as_mut_ptr() as *mut libc::c_void, 4) };
            if n != 0 {
                // exec() failed: record it, still release s6-rc.
                self.st.set(keys::START_FAILED, "exec failed");
            }
            if let Some(fd) = notify {
                unsafe { libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1) };
            }
            if n == 0 {
                self.after_ready(main_pid);
            }
        })?;
        unsafe { libc::close(r) };
        EXEC_FAIL_PIPE.with(|c| c.set(Some(w)));
        Ok(())
    }

    /// Type=notify: bind NOTIFY_SOCKET and relay READY=1 to s6.
    fn start_notify_helper(&mut self, main_pid: u32) -> Result<String, String> {
        let dir = self.cfg.run_dir.join("notify");
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        crate::selinux::restorecon(&dir.display().to_string());
        let inv = self.st.get(keys::INVOCATION).unwrap_or_else(|| main_pid.to_string());
        let path = dir.join(&inv[..inv.len().min(32)]);
        let _ = fs::remove_file(&path);
        let path_s = path.display().to_string();

        // sun_path is 108 bytes; fall back to an abstract socket for long paths.
        let path_s = if path_s.len() >= 100 { format!("@uinitconverter/notify/{inv}") } else { path_s };
        let fd = socket::bind_unix(&path_s, libc::SOCK_DGRAM, &self.unit)?;
        // World-writable, as systemd's notify socket: daemons such as chronyd
        // start as root, drop to their own user, and only then send READY=1.
        // Senders are checked by their credentials instead (notify_sender_ok).
        let on: libc::c_int = 1;
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                &on as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as u32,
            );
        }
        if !path_s.starts_with('@') {
            let c = cstr(&path_s);
            unsafe { libc::chmod(c.as_ptr(), 0o666) };
        }
        self.st.set(keys::NOTIFY_SOCKET, &path_s);
        // Without User= the service runs as we do. Matching the uid also
        // covers a notifying child that exits before its cgroup is read.
        let unit_uid = Some(self.user.as_ref().map(|u| u.uid).unwrap_or_else(|| unsafe { libc::geteuid() }));

        let h = NotifyHelper {
            fd,
            notify: self.notify_fd,
            main_pid,
            unit_uid,
            // None: no start timeout (TimeoutStartSec=infinity or 0).
            timeout_us: self.start_timeout().map(|t| t.as_micros().min(u64::MAX as u128) as u64),
            st_dir: self.st.dir.clone(),
            store: self.cfg.run_dir.join("fdstore"),
            unit_id: self.unit.id.clone(),
            fdstore_max: self.unit.get("Service", "FileDescriptorStoreMax").and_then(|v| v.parse().ok()).unwrap_or(0),
            notify_reload: self.service_type() == "notify-reload",
            path: path_s.clone(),
            post: !self.unit.get_all("Service", "ExecStartPost").is_empty(),
            config: self.cfg.config_path.clone(),
        };
        double_fork(|| {
            // A fresh image: a fork would keep all of the runner's memory
            // (~1.7 MiB) for as long as the service runs. Returns only if
            // the exec failed; then the loop runs here.
            h.exec();
            h.run(&|pid| self.after_ready(pid));
        })?;
        unsafe { libc::close(fd) };
        Ok(path_s)
    }

    /// Type=dbus: ready once BusName= appears on the system bus.
    fn start_dbus_helper(&mut self, main_pid: u32) {
        let notify = self.notify_fd;
        let tries = self.start_timeout().map(|t| (t.as_millis() / 100) as u32).unwrap_or(u32::MAX);
        let Some(bus_name) = self.unit.get("Service", "BusName") else {
            let _ = self.start_exec_helper();
            return;
        };
        let _ = double_fork(|| {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(_) => return,
            };
            // Ok(true) ready, Ok(false) died, Err timeout/bus error.
            let outcome: Result<bool, &str> = rt.block_on(async {
                let Ok(conn) = zbus::Connection::system().await else { return Err("no system bus") };
                let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else { return Err("no system bus") };
                let Ok(name) = zbus::names::BusName::try_from(bus_name.as_str()) else { return Err("bad BusName=") };
                for _ in 0..tries {
                    if unsafe { libc::kill(main_pid as i32, 0) } < 0 {
                        return Ok(false);
                    }
                    if proxy.name_has_owner(name.clone()).await.unwrap_or(false) {
                        return Ok(true);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err("start timeout")
            });
            match outcome {
                Ok(true) => {}
                Ok(false) => self.st.set(keys::START_FAILED, "exited before acquiring its bus name"),
                Err(why) => {
                    self.st.set(keys::START_FAILED, why);
                    if why == "start timeout" {
                        self.st.set(keys::RESULT, "timeout");
                        unsafe { libc::kill(main_pid as i32, libc::SIGTERM) };
                    }
                }
            }
            if let Some(fd) = notify {
                unsafe { libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1) };
            }
            if outcome == Ok(true) {
                self.after_ready(main_pid);
            }
        });
    }

    /// Type=forking: stay around as a subreaper and supervise the daemon.
    fn run_forking(&mut self, main: &ExecCommand) -> i32 {
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if let Err(e) = self.setup_listen_fds() {
            self.soft_fail(&e);
            return 1;
        }
        self.st.stamp(keys::EXEC_MAIN_START);
        let st = self.spawn_wait(main, &[]);
        let deadline = self.deadline.take();
        if !st.ok() {
            self.st.set(keys::RESULT, if self.timed_out.get() { "timeout" } else { result_for(st) });
            self.soft_fail(&format!("start command failed: {}", describe(st)));
            return exit_code_for(st);
        }
        let me = std::process::id() as i32;
        let main_pid = match self.unit.get("Service", "PIDFile") {
            Some(pidfile) => match self.wait_pid_file(&pidfile, me, deadline) {
                Ok(p) => Some(p),
                Err(result) => {
                    self.st.set(keys::RESULT, result);
                    self.soft_fail(&format!("PIDFile={pidfile} not written ({result})"));
                    return 1;
                }
            },
            None => self.guess_main_pid(me),
        };
        let Some(main_pid) = main_pid else {
            log(self.id(), "could not determine main PID");
            self.signal_ready();
            self.after_ready(0);
            return 0;
        };
        self.st.set(keys::MAIN_PID, &main_pid.to_string());
        FORWARD_PID.store(main_pid, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT, libc::SIGUSR1, libc::SIGUSR2] {
                libc::signal(sig, forward_signal as *const () as libc::sighandler_t);
            }
        }
        self.signal_ready();
        self.after_ready(main_pid as u32);

        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, main_pid, 0) } as i32;
        let mut main_status: Option<i32> = None;
        loop {
            loop {
                let mut s = 0;
                let r = unsafe { libc::waitpid(-1, &mut s, libc::WNOHANG) };
                if r <= 0 {
                    break;
                }
                if r == main_pid {
                    main_status = Some(s);
                }
            }
            if main_status.is_some() {
                break;
            }
            if pidfd >= 0 {
                let mut p = libc::pollfd { fd: pidfd, events: libc::POLLIN, revents: 0 };
                unsafe { libc::poll(&mut p, 1, 1000) };
                if p.revents != 0 {
                    // Reap it if it is our (reparented) child.
                    let mut s = 0;
                    if unsafe { libc::waitpid(main_pid, &mut s, libc::WNOHANG) } == main_pid {
                        main_status = Some(s);
                    }
                    break;
                }
            } else {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if unsafe { libc::kill(main_pid, 0) } < 0 {
                    break;
                }
            }
        }
        match main_status.map(Status::from_wait) {
            Some(Status::Killed(sig)) => {
                unsafe {
                    libc::signal(sig, libc::SIG_DFL);
                    libc::kill(me, sig);
                }
                128 + sig
            }
            Some(Status::Exited(c)) => c,
            None => 0,
        }
    }

    /// As systemd: wait for the PID file up to TimeoutStartSec= ("timeout"),
    /// or until no process is left that could write it ("protocol").
    fn wait_pid_file(&self, pidfile: &str, me: i32, deadline: Option<std::time::Instant>) -> Result<i32, &'static str> {
        let mut refused = false;
        loop {
            if let Some(p) = util::read_trimmed(Path::new(pidfile)).and_then(|s| s.parse::<i32>().ok()) {
                // As systemd's service_load_pid_file(): a PID file someone
                // other than root could have written names a process of the
                // unit only, or the runner would signal any process as root.
                let cgroup = self.st.get(keys::CGROUP);
                let ok = p > 1
                    && p != me
                    && (!pid_file_questionable(Path::new(pidfile)) || main_pid_ok(p as u32, u32::MAX, me as u32, cgroup.as_deref()));
                if ok {
                    return Ok(p);
                }
                if !refused {
                    log(self.id(), &format!("PIDFile={pidfile}: PID {p} is not a process of the unit and the file is not root's alone: refusing it"));
                    refused = true;
                }
            }
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Err("timeout");
            }
            let in_cgroup = self
                .cgroup_dir()
                .is_some_and(|d| util::cgroup_pids(&d).iter().any(|&p| p != me as u32));
            if !in_cgroup && self.guess_main_pid(me).is_none() {
                return Err("protocol");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// GuessMainPID: our (reparented) children.
    fn guess_main_pid(&self, me: i32) -> Option<i32> {
        let mut kids = Vec::new();
        for e in fs::read_dir("/proc").ok()?.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
            let stat = fs::read_to_string(e.path().join("stat")).unwrap_or_default();
            let ppid = stat
                .rsplit_once(')')
                .and_then(|(_, r)| r.split_whitespace().nth(1))
                .and_then(|p| p.parse::<i32>().ok());
            if ppid == Some(me) {
                kids.push(pid);
            }
        }
        kids.sort();
        kids.first().copied()
    }

    /// longrun `finish` script: cleanup and Restart= policy.
    pub fn cmd_finish(self, exit_code: i32, signum: i32) -> i32 {
        let stopped = !supervisor::wanted_up();
        // Stopped: an on-demand service waits for activity again next time.
        if stopped {
            self.st.clear(keys::ACTIVATED);
        }
        if self.st.has(keys::SOCKET_WAIT) {
            // Ended while waiting for its sockets: the service wasn't running.
            let requested_restart = self.st.has(keys::RESTART_REQUESTED);
            self.st.clear(keys::RESTART_REQUESTED);
            if stopped || requested_restart {
                self.st.clear(keys::SOCKET_WAIT);
            }
            if stopped {
                for s in self.socket_units() {
                    socket::remove_paths(&s);
                }
            }
            return 0;
        }
        let status = if exit_code == 256 || exit_code < 0 && signum > 0 {
            Status::Killed(signum)
        } else {
            Status::Exited(exit_code)
        };
        self.st.stamp(keys::EXEC_MAIN_EXIT);
        let (code, val) = match status {
            Status::Exited(c) => (1, c),
            Status::Killed(s) => (2, s),
        };
        self.st.set(keys::EXEC_MAIN_CODE, &code.to_string());
        self.st.set(keys::EXEC_MAIN_STATUS, &val.to_string());
        self.st.stamp(keys::ACTIVE_EXIT);

        let requested_restart = self.st.has(keys::RESTART_REQUESTED);
        self.st.clear(keys::RESTART_REQUESTED);
        let stopping = stopped || requested_restart;
        let kill_signal = self
            .unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .unwrap_or(libc::SIGTERM);
        let clean_exit = self.is_clean(status) || (stopping && status == Status::Killed(kill_signal));
        let mut result = if clean_exit {
            "success"
        } else {
            match status {
                Status::Killed(_) => "signal",
                Status::Exited(_) => "exit-code",
            }
        }
        .to_string();
        if let Some(r) = self.st.get(keys::RESULT) {
            if r != "success" && !stopping {
                result = r;
            }
        }
        // A start timeout or failed ExecStartPost= ends in our own SIGTERM,
        // which on its own would count as clean.
        let clean = clean_exit && result == "success";

        self.cleanup(&result, status, stopping);

        if stopping {
            self.st.set(keys::RESULT, &result);
            if !requested_restart {
                self.st.stamp(keys::INACTIVE_ENTER);
            }
            return 0;
        }
        if self.st.has(keys::ASSERT_FAILED) {
            self.st.set(keys::RESULT, "exit-code");
            return 125;
        }

        let restart_mode = self.unit.get("Service", "Restart").unwrap_or_else(|| "no".into());
        let mut restart = match restart_mode.as_str() {
            "always" => true,
            "on-success" => clean,
            "on-failure" => !clean,
            "on-abnormal" | "on-watchdog" => result == "timeout" || matches!(status, Status::Killed(_)) && !clean_exit,
            "on-abort" => matches!(status, Status::Killed(_)) && !clean_exit,
            _ => false,
        };
        if status_in(&self.unit.get_words("Service", "RestartPreventExitStatus"), status) {
            restart = false;
        }
        if status_in(&self.unit.get_words("Service", "RestartForceExitStatus"), status) {
            restart = true;
        }

        if restart {
            let interval = self
                .unit
                .get("Unit", "StartLimitIntervalSec")
                .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
                .unwrap_or(10_000_000);
            let burst: usize = self
                .unit
                .get("Unit", "StartLimitBurst")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5);
            let now = util::now_monotonic_usec();
            // No limit: nothing to remember (the list would only grow).
            let limited = interval > 0 && burst > 0;
            let mut times: Vec<u64> = self
                .st
                .get(keys::RESTART_TIMES)
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .filter(|&t: &u64| limited && now.saturating_sub(t) < interval)
                .collect();
            if limited {
                times.push(now);
            }
            self.st.set(
                keys::RESTART_TIMES,
                &times.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" "),
            );
            if limited && times.len() > burst {
                log(self.id(), "start request repeated too quickly, giving up");
                self.st.set(keys::RESULT, "start-limit-hit");
                self.st.stamp(keys::INACTIVE_ENTER);
                self.on_result(false);
                return 125;
            }
            let n = self.st.get_u64(keys::NRESTARTS) + 1;
            self.st.set(keys::NRESTARTS, &n.to_string());
            self.st.set(keys::RESULT, &result);
            let delay = self
                .unit
                .get("Service", "RestartSec")
                .and_then(|v| crate::unit::parse_timespan_usec(&v, 1_000_000))
                .unwrap_or(100_000);
            // s6-supervise already waits ~1s between restarts.
            if delay > 1_000_000 {
                std::thread::sleep(std::time::Duration::from_micros(delay - 1_000_000));
            }
            return 0;
        }
        self.st.set(keys::RESULT, &result);
        self.st.stamp(keys::INACTIVE_ENTER);
        self.on_result(result == "success");
        if result == "success" && self.has_held_sockets() {
            // Stay up for s6: the next run waits for socket activity.
            self.st.set(keys::SOCKET_WAIT, "1");
            return 0;
        }
        125
    }

    fn has_held_sockets(&self) -> bool {
        if self.socket_units().is_empty() {
            return false;
        }
        let store = self.cfg.run_dir.join("fdstore");
        !crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id)).is_empty()
    }

    fn is_clean(&self, status: Status) -> bool {
        if status_in(&self.unit.get_words("Service", "SuccessExitStatus"), status) {
            return true;
        }
        match status {
            Status::Exited(0) => true,
            Status::Killed(s) => [libc::SIGHUP, libc::SIGINT, libc::SIGTERM, libc::SIGPIPE].contains(&s),
            _ => false,
        }
    }

    fn cleanup(&self, result: &str, status: Status, stopping: bool) {
        let (code, st) = match status {
            Status::Exited(c) => ("exited", c.to_string()),
            Status::Killed(s) => ("killed", util::signal_name(s)),
        };
        let env = [
            ("SERVICE_RESULT".to_string(), result.to_string()),
            ("EXIT_CODE".to_string(), code.to_string()),
            ("EXIT_STATUS".to_string(), st),
        ];
        // KillMode=control-group / mixed: leftover processes go too, before
        // ExecStopPost= and again after it (systemd's stop-sigterm,
        // stop-post and final-sigterm states).
        let mode = self.unit.get("Service", "KillMode").unwrap_or_else(|| "control-group".into());
        let kill_leftovers = || {
            if mode != "control-group" && mode != "mixed" {
                return;
            }
            let Some(dir) = self.cgroup_dir() else { return };
            let me = std::process::id();
            if cgroup::kill_all(&dir, libc::SIGTERM, me) > 0 {
                for _ in 0..20 {
                    if util::cgroup_pids(&dir).is_empty() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if self.unit.get_bool("Service", "SendSIGKILL", true) {
                    cgroup::kill_all(&dir, libc::SIGKILL, me);
                }
            }
        };
        kill_leftovers();
        let _ = self.run_list("ExecStopPost", &env);
        kill_leftovers();
        if let Some(sock) = self.st.get(keys::NOTIFY_SOCKET) {
            let _ = fs::remove_file(sock);
            self.st.clear(keys::NOTIFY_SOCKET);
        }
        // Listening sockets stay in the daemon's fd store across restarts;
        // only a real stop removes their paths (RemoveOnStop=).
        if stopping {
            for s in self.socket_units() {
                socket::remove_paths(&s);
            }
        }
        self.remove_runtime_dirs();
        let _ = fs::remove_dir_all(self.st.dir.join("private"));
    }

    /// .mount/.swap: mount(8) or swapon(8), with the usual state bookkeeping.
    fn mount_job(&self, up: bool) -> i32 {
        use crate::mountunit;
        let code = match (self.unit.utype(), up) {
            ("mount", true) => mountunit::mount_up(&self.unit),
            ("mount", false) => mountunit::mount_down(&self.unit),
            ("swap", true) => mountunit::swap_up(&self.unit),
            _ => mountunit::swap_down(&self.unit),
        };
        if up {
            self.st.stamp(keys::INACTIVE_EXIT);
            self.st.set(keys::RESULT, if code == 0 { "success" } else { "exit-code" });
            self.st.set(keys::EXEC_MAIN_STATUS, &code.to_string());
            if code == 0 {
                self.st.stamp(keys::ACTIVE_ENTER);
            } else {
                self.st.stamp(keys::INACTIVE_ENTER);
            }
        } else {
            self.st.stamp(keys::ACTIVE_EXIT);
            self.st.stamp(keys::INACTIVE_ENTER);
        }
        code
    }

    /// oneshot `up` script.
    pub fn cmd_up(mut self) -> i32 {
        // Every outcome is reported to s6-rc as success: a failing oneshot
        // must not block units that only want it. The result is recorded and
        // dependents with Requires= check it (see failed_requirement()).
        if let Some(dep) = self.failed_requirement() {
            log(self.id(), &format!("not starting: required unit {dep} failed"));
            self.st.set(keys::DEPENDENCY_FAILED, &dep);
            return 0;
        }
        if matches!(self.unit.utype(), "mount" | "swap") {
            match conditions::evaluate(&self.unit) {
                ev if !ev.conditions_ok => {
                    self.st.set(keys::CONDITION_FAILED, &ev.failure.unwrap_or_default());
                    return 0;
                }
                _ => self.st.clear(keys::CONDITION_FAILED),
            }
            let code = self.mount_job(true);
            if code != 0 {
                self.on_result(false);
            }
            return 0;
        }
        self.enter_cgroup();
        if let Err(e) = self.prepare_start() {
            log(self.id(), &e);
            self.st.set(keys::RESULT, "resources");
            self.st.stamp(keys::INACTIVE_ENTER);
            return 0;
        }
        match self.check_conditions() {
            Ok(true) => {}
            Ok(false) => {
                log(self.id(), "condition check failed, skipped");
                self.st.set(keys::RESULT, "success");
                return 0;
            }
            Err(e) => {
                log(self.id(), &e);
                self.st.set(keys::RESULT, "exit-code");
                self.st.stamp(keys::INACTIVE_ENTER);
                self.on_result(false);
                return 0;
            }
        }
        self.st.set(keys::RESULT, "running");
        self.st.stamp(keys::EXEC_MAIN_START);
        self.deadline.set(self.start_timeout().map(|t| std::time::Instant::now() + t));
        let mut failed: Option<Status> = None;
        for key in ["ExecStartPre", "ExecStart", "ExecStartPost"] {
            if let Err(st) = self.run_list(key, &[]) {
                failed = Some(st);
                break;
            }
        }
        self.deadline.set(None);
        self.st.stamp(keys::EXEC_MAIN_EXIT);
        if let Some(st) = failed {
            let (code, val) = match st {
                Status::Exited(c) => (1, c),
                Status::Killed(s) => (2, s),
            };
            let result = if self.timed_out.get() { "timeout" } else { result_for(st) };
            self.st.set(keys::EXEC_MAIN_CODE, &code.to_string());
            self.st.set(keys::EXEC_MAIN_STATUS, &val.to_string());
            self.st.set(keys::RESULT, result);
            self.cleanup(result, st, true);
            self.st.stamp(keys::INACTIVE_ENTER);
            self.on_result(false);
            return 0;
        }
        self.st.set(keys::EXEC_MAIN_CODE, "1");
        self.st.set(keys::EXEC_MAIN_STATUS, "0");
        self.st.set(keys::RESULT, "success");
        self.st.stamp(keys::ACTIVE_ENTER);
        if !self.unit.get_bool("Service", "RemainAfterExit", false) {
            // systemd stops a oneshot right after it finished.
            let _ = self.run_list("ExecStop", &[]);
            self.cleanup("success", Status::Exited(0), true);
            self.st.stamp(keys::INACTIVE_ENTER);
        }
        self.on_result(true);
        0
    }


    /// oneshot `down` script.
    pub fn cmd_down(self) -> i32 {
        if matches!(self.unit.utype(), "mount" | "swap") {
            return self.mount_job(false);
        }
        if self.unit.get_bool("Service", "RemainAfterExit", false) {
            let _ = self.run_list("ExecStop", &[]);
            self.cleanup("success", Status::Exited(0), true);
        }
        self.st.stamp(keys::ACTIVE_EXIT);
        self.st.stamp(keys::INACTIVE_ENTER);
        0
    }

    /// ExecStop= of a longrun (called by the daemon before s6-rc -d).
    pub fn cmd_stop_exec(self, main_pid: Option<u32>) -> i32 {
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        match self.run_list("ExecStop", &env) {
            Ok(()) => 0,
            Err(st) => exit_code_for(st),
        }
    }

    /// `down` of the longrun's UNIT.stop oneshot (uinitconverter-gen), which s6-rc
    /// takes down before the longrun: ExecStop= with $MAINPID, as systemd
    /// runs it before the KillSignal=, if the service runs. Skipped, as
    /// systemd's stop of an inactive unit, when it was not started or waits
    /// for socket activity. Always 0: s6-rc would keep a service whose
    /// ExecStop= failed up.
    pub fn cmd_stop_atom(self) -> i32 {
        let skipped = [
            keys::SOCKET_WAIT,
            keys::CONDITION_FAILED,
            keys::DEPENDENCY_FAILED,
            keys::START_FAILED,
            keys::PARKED,
        ]
        .into_iter()
        .any(|k| self.st.has(k));
        if skipped {
            return 0;
        }
        let Some(sv_pid) = supervisor::up_pid(&self.cfg, self.id()) else { return 0 };
        // The service's own PID: under PAMName= or Type=forking the
        // supervised process is the runner around it.
        let main_pid = self
            .st
            .get(keys::MAIN_PID)
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|&p| p > 0 && unsafe { libc::kill(p as i32, 0) } == 0)
            .or(sv_pid);
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        let t = self.unit.stop_timeout_usec();
        if t != u64::MAX {
            self.deadline.set(Some(std::time::Instant::now() + std::time::Duration::from_micros(t)));
            self.deadline_what.set("stop");
        }
        let _ = self.run_list("ExecStop", &env);
        0
    }

    /// ExecReload=, or ReloadSignal for Type=notify-reload.
    pub fn cmd_reload(self, main_pid: Option<u32>) -> i32 {
        if self.unit.get_all("Service", "ExecReload").is_empty() {
            if self.service_type() == "notify-reload" {
                if let Some(p) = main_pid {
                    let sig = self
                        .unit
                        .get("Service", "ReloadSignal")
                        .and_then(|s| util::signal_number(&s))
                        .unwrap_or(libc::SIGHUP);
                    let before = self.st.get(keys::RELOADS_DONE).map(|_| self.st.get_u64(keys::RELOADS_DONE));
                    if unsafe { libc::kill(p as i32, sig) } < 0 {
                        return 1;
                    }
                    // Like systemd: done once the service has sent RELOADING=1
                    // and then READY=1, within TimeoutStartSec=.
                    let Some(before) = before else { return 0 };
                    let deadline = self.start_timeout().map(|t| std::time::Instant::now() + t);
                    while self.st.get_u64(keys::RELOADS_DONE) == before {
                        if unsafe { libc::kill(p as i32, 0) } < 0 {
                            log(self.id(), "exited while reloading");
                            return 1;
                        }
                        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                            log(self.id(), "reload timed out");
                            return 1;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    return 0;
                }
            }
            log(self.id(), "no ExecReload= configured");
            return 1;
        }
        let env: Vec<(String, String)> = main_pid
            .map(|p| vec![("MAINPID".to_string(), p.to_string())])
            .unwrap_or_default();
        match self.run_list("ExecReload", &env) {
            Ok(()) => 0,
            Err(st) => exit_code_for(st),
        }
    }
}

const DIRS: &[(&str, &str, &str)] = &[
    ("RuntimeDirectory", "/run", "RUNTIME_DIRECTORY"),
    ("StateDirectory", "/var/lib", "STATE_DIRECTORY"),
    ("CacheDirectory", "/var/cache", "CACHE_DIRECTORY"),
    ("LogsDirectory", "/var/log", "LOGS_DIRECTORY"),
    ("ConfigurationDirectory", "/etc", "CONFIGURATION_DIRECTORY"),
];

const LIMITS: &[(&str, libc::__rlimit_resource_t, bool)] = &[
    ("LimitCPU", libc::RLIMIT_CPU, false),
    ("LimitFSIZE", libc::RLIMIT_FSIZE, true),
    ("LimitDATA", libc::RLIMIT_DATA, true),
    ("LimitSTACK", libc::RLIMIT_STACK, true),
    ("LimitCORE", libc::RLIMIT_CORE, true),
    ("LimitRSS", libc::RLIMIT_RSS, true),
    ("LimitNOFILE", libc::RLIMIT_NOFILE, false),
    ("LimitAS", libc::RLIMIT_AS, true),
    ("LimitNPROC", libc::RLIMIT_NPROC, false),
    ("LimitMEMLOCK", libc::RLIMIT_MEMLOCK, true),
    ("LimitLOCKS", libc::RLIMIT_LOCKS, false),
    ("LimitSIGPENDING", libc::RLIMIT_SIGPENDING, false),
    ("LimitMSGQUEUE", libc::RLIMIT_MSGQUEUE, true),
    ("LimitNICE", libc::RLIMIT_NICE, false),
    ("LimitRTPRIO", libc::RLIMIT_RTPRIO, false),
    ("LimitRTTIME", libc::RLIMIT_RTTIME, false),
];

thread_local! {
    static EXEC_FAIL_PIPE: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

static FORWARD_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

static WOKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn wake_signal(_: libc::c_int) {
    WOKEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

extern "C" fn forward_signal(sig: libc::c_int) {
    let pid = FORWARD_PID.load(std::sync::atomic::Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, sig) };
    }
}

/// The Type=notify helper: receives on NOTIFY_SOCKET for as long as the
/// service runs, relays READY=1 to s6 and records STATUS=, MAINPID=,
/// RELOADING=1, EXTEND_TIMEOUT_USEC= and FDSTORE=. It re-executes the runner
/// (`notify-helper`, internal) with these on its command line.
pub struct NotifyHelper {
    /// The bound notify socket.
    fd: i32,
    /// s6's readiness pipe.
    notify: Option<i32>,
    main_pid: u32,
    unit_uid: Option<u32>,
    timeout_us: Option<u64>,
    st_dir: PathBuf,
    store: PathBuf,
    unit_id: String,
    fdstore_max: usize,
    notify_reload: bool,
    /// The socket's path (or @abstract name), removed at the end.
    path: String,
    /// The unit has ExecStartPost=.
    post: bool,
    config: PathBuf,
}

impl NotifyHelper {
    fn to_args(&self) -> Vec<String> {
        let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
        let mut a = vec![
            "notify-helper".to_string(),
            "--fd".into(),
            self.fd.to_string(),
            "--notify-fd".into(),
            opt(self.notify.map(|f| f.to_string())),
            "--main-pid".into(),
            self.main_pid.to_string(),
            "--uid".into(),
            opt(self.unit_uid.map(|u| u.to_string())),
            "--timeout-us".into(),
            opt(self.timeout_us.map(|t| t.to_string())),
            "--state".into(),
            self.st_dir.display().to_string(),
            "--store".into(),
            self.store.display().to_string(),
            "--fdstore-max".into(),
            self.fdstore_max.to_string(),
            "--path".into(),
            self.path.clone(),
            "--config-file".into(),
            self.config.display().to_string(),
        ];
        if self.notify_reload {
            a.push("--reload".into());
        }
        if self.post {
            a.push("--post".into());
        }
        // Last, so `pgrep -f '... --unit NAME$'` finds it.
        a.push("--unit".into());
        a.push(self.unit_id.clone());
        a
    }

    pub fn from_args(args: &[String]) -> Result<NotifyHelper, String> {
        let mut h = NotifyHelper {
            fd: -1,
            notify: None,
            main_pid: 0,
            unit_uid: None,
            timeout_us: None,
            st_dir: PathBuf::new(),
            store: PathBuf::new(),
            unit_id: String::new(),
            fdstore_max: 0,
            notify_reload: false,
            path: String::new(),
            post: false,
            config: PathBuf::new(),
        };
        let mut it = args.iter();
        while let Some(k) = it.next() {
            if k == "--reload" {
                h.notify_reload = true;
                continue;
            }
            if k == "--post" {
                h.post = true;
                continue;
            }
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            let num = |v: &str| v.parse::<u64>().map_err(|_| format!("{k}: bad number {v}"));
            let opt = |v: &str| if v == "-" { Ok(None) } else { num(v).map(Some) };
            match k.as_str() {
                "--fd" => h.fd = num(v)? as i32,
                "--notify-fd" => h.notify = opt(v)?.map(|f| f as i32),
                "--main-pid" => h.main_pid = num(v)? as u32,
                "--uid" => h.unit_uid = opt(v)?.map(|u| u as u32),
                "--timeout-us" => h.timeout_us = opt(v)?,
                "--state" => h.st_dir = PathBuf::from(v),
                "--store" => h.store = PathBuf::from(v),
                "--fdstore-max" => h.fdstore_max = num(v)? as usize,
                "--path" => h.path = v.clone(),
                "--config-file" => h.config = PathBuf::from(v),
                "--unit" => h.unit_id = v.clone(),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        if h.fd < 0 || h.main_pid == 0 || h.unit_id.is_empty() {
            return Err("--fd, --main-pid and --unit are required".into());
        }
        Ok(h)
    }

    /// exec() the runner as `notify-helper`; returns only on failure.
    fn exec(&self) {
        // The installed path, so ps shows uinitconverter-runner; after an
        // update replaced it, /proc/self/exe still is this binary.
        let exe = std::env::current_exe()
            .ok()
            .filter(|p| !p.to_string_lossy().ends_with(" (deleted)"))
            .unwrap_or_else(|| PathBuf::from("/proc/self/exe"));
        let mut argv: Vec<CString> = vec![cstr("uinitconverter-runner")];
        argv.extend(self.to_args().iter().map(|a| cstr(a)));
        let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let path = cstr(&exe.display().to_string());
        unsafe {
            for fd in [Some(self.fd), self.notify].into_iter().flatten() {
                libc::fcntl(fd, libc::F_SETFD, 0);
            }
            libc::execv(path.as_ptr(), ptrs.as_ptr());
        }
        eprintln!("uinitconverter[{}]: notify helper: exec {}: {}", self.unit_id, exe.display(), errno());
    }

    /// The receive loop; `after_ready` runs once READY=1 came (ExecStartPost=).
    fn run(&self, after_ready: &dyn Fn(u32)) {
        let (fd, notify, main_pid, unit_uid) = (self.fd, self.notify, self.main_pid, self.unit_uid);
        let (store, unit_id, fdstore_max) = (&self.store, &self.unit_id, self.fdstore_max);
        let st = UnitState::new(self.st_dir.clone());
        // Present only with a helper that counts reloads, so cmd_reload
        // doesn't wait on a service started by an older binary.
        if self.notify_reload {
            st.set(keys::RELOADS_DONE, &st.get_u64(keys::RELOADS_DONE).to_string());
        }
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, main_pid as i32, 0) } as i32;
        let (mut ready, mut reloading) = (false, false);
        let release = |reason: Option<&str>| {
            if let Some(r) = reason {
                st.set(keys::START_FAILED, r);
            }
            if let Some(nfd) = notify {
                unsafe {
                    libc::write(nfd, b"\n".as_ptr() as *const libc::c_void, 1);
                    libc::close(nfd);
                }
            }
        };
        let mut deadline = self.timeout_us.map(|t| util::now_monotonic_usec().saturating_add(t));
        loop {
            let mut pfds = [
                libc::pollfd { fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: pidfd, events: libc::POLLIN, revents: 0 },
            ];
            let nfds = if pidfd >= 0 { 2 } else { 1 };
            let wait = match (ready, deadline) {
                _ if pidfd < 0 => 1000,
                (false, Some(d)) => (d.saturating_sub(util::now_monotonic_usec()) / 1000).min(i32::MAX as u64) as i32,
                // -1 blocks until a message arrives or the service exits.
                _ => -1,
            };
            unsafe { libc::poll(pfds.as_mut_ptr(), nfds, wait) };
            let died = if pidfd >= 0 {
                pfds[1].revents != 0
            } else {
                (unsafe { libc::kill(main_pid as i32, 0) }) < 0
            };
            if pfds[0].revents & libc::POLLIN != 0 {
                let Ok((data, fds, cred)) = crate::fdstore::recv(fd) else { continue };
                if !notify_sender_ok(cred, main_pid, unit_uid) {
                    continue;
                }
                let msg = String::from_utf8_lossy(&data).into_owned();
                let mut fdname = "stored".to_string();
                let (mut store_fds, mut remove) = (false, false);
                for line in msg.lines() {
                    if line == "READY=1" && !ready {
                        ready = true;
                        release(None);
                        after_ready(main_pid);
                    } else if line == "RELOADING=1" {
                        reloading = true;
                    } else if line == "READY=1" && reloading {
                        reloading = false;
                        st.set(keys::RELOADS_DONE, &(st.get_u64(keys::RELOADS_DONE) + 1).to_string());
                    } else if let Some(s) = line.strip_prefix("STATUS=") {
                        st.set(keys::STATUS_TEXT, s);
                    } else if let (Some(n), false) = (line.strip_prefix("EXTEND_TIMEOUT_USEC="), ready) {
                        // As systemd: the start timeout ends no earlier
                        // than N usec from now (MariaDB's crash recovery
                        // sends it); the daemon's wait reads it too.
                        if let (Some(d), Ok(n)) = (deadline.as_mut(), n.parse::<u64>()) {
                            let until = util::now_monotonic_usec().saturating_add(n);
                            if until > *d {
                                *d = until;
                                st.set(keys::EXTEND_UNTIL, &until.to_string());
                            }
                        }
                    } else if let Some(p) = line.strip_prefix("MAINPID=") {
                        let sender_uid = cred.map(|c| c.uid).unwrap_or(u32::MAX);
                        match p.parse::<u32>() {
                            Ok(p) if main_pid_ok(p, sender_uid, main_pid, st.get(keys::CGROUP).as_deref()) => {
                                st.set(keys::MAIN_PID, &p.to_string())
                            }
                            _ => eprintln!("uinitconverter[{unit_id}]: MAINPID={p} is not a process of the unit, refused"),
                        }
                    } else if let Some(n) = line.strip_prefix("FDNAME=") {
                        // systemd's fdname_is_valid(): printable ASCII
                        // without ':', at most 255 bytes; else "stored".
                        if n.len() <= 255 && n.bytes().all(|b| (b' '..=b'~').contains(&b) && b != b':') {
                            fdname = n.to_string();
                        } else {
                            eprintln!("uinitconverter[{unit_id}]: invalid FDNAME=, ignored");
                        }
                    } else if line == "FDSTORE=1" {
                        store_fds = true;
                    } else if line == "FDSTOREREMOVE=1" {
                        remove = true;
                    }
                }
                if store_fds {
                    for f in &fds {
                        use std::os::fd::AsRawFd;
                        if let Err(e) = crate::fdstore::put(store, unit_id, &fdname, f.as_raw_fd(), fdstore_max) {
                            eprintln!("uinitconverter[{unit_id}]: FDSTORE: {e}");
                        }
                    }
                }
                if remove {
                    crate::fdstore::del(store, unit_id, Some(&fdname));
                }
                continue;
            }
            if died {
                if !ready {
                    release(Some("exited before sending READY=1"));
                }
                break;
            }
            if !ready && deadline.map(|d| util::now_monotonic_usec() >= d).unwrap_or(false) {
                // TimeoutStartSec=: fail the start and terminate the service.
                st.set(keys::RESULT, "timeout");
                release(Some("start timeout"));
                unsafe { libc::kill(main_pid as i32, libc::SIGTERM) };
                ready = true;
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// `uinitconverter-runner notify-helper ...` (started by `run`, see
/// NotifyHelper). Only the fds it was given stay open: the runner's
/// LISTEN_FDS are the service's.
pub fn notify_helper_main(args: &[String]) -> i32 {
    let h = match NotifyHelper::from_args(args) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("uinitconverter-runner notify-helper: {e}");
            return 2;
        }
    };
    let keep = [Some(h.fd), h.notify];
    let open: Vec<i32> = fs::read_dir("/proc/self/fd")
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default();
    for fd in open {
        if fd > 2 && !keep.contains(&Some(fd)) {
            unsafe { libc::close(fd) };
        }
    }
    for fd in [Some(h.fd), h.notify].into_iter().flatten() {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let st = UnitState::new(h.st_dir.clone());
    h.run(&|pid| {
        if !h.post {
            st.stamp(keys::ACTIVE_ENTER);
            return;
        }
        // ExecStartPost= needs the whole runner: in a child, so this
        // process stays small.
        let child = unsafe { libc::fork() };
        if child == 0 {
            let code = match Config::load(Some(&h.config)).and_then(|cfg| Runner::new(cfg, &h.unit_id, true)) {
                Ok(r) => {
                    r.after_ready(pid);
                    0
                }
                Err(e) => {
                    eprintln!("uinitconverter[{}]: ExecStartPost=: {e}", h.unit_id);
                    st.stamp(keys::ACTIVE_ENTER);
                    1
                }
            };
            unsafe { libc::_exit(code) };
        }
        if child < 0 {
            st.stamp(keys::ACTIVE_ENTER);
        } else {
            wait_pid(child);
        }
    });
    0
}

/// Run `f` in a grandchild that is not our child (so the service never has
/// to reap it).
fn double_fork<F: FnOnce()>(f: F) -> Result<(), String> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!("fork: {}", errno()));
    }
    if pid == 0 {
        unsafe { libc::setsid() };
        let pid2 = unsafe { libc::fork() };
        if pid2 == 0 {
            f();
            unsafe { libc::_exit(0) };
        }
        unsafe { libc::_exit(0) };
    }
    wait_pid(pid);
    Ok(())
}

/// May this sender talk to a unit's notify socket? Root, the unit's User=,
/// the main process, or any process in the main process's cgroup (systemd's
/// NotifyAccess=all; children may notify via systemd-notify).
fn notify_sender_ok(cred: Option<libc::ucred>, main_pid: u32, unit_uid: Option<u32>) -> bool {
    let Some(c) = cred else { return false };
    if c.uid == 0 || Some(c.uid) == unit_uid || c.pid as u32 == main_pid {
        return true;
    }
    let cg = |pid: u32| fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    matches!((cg(c.pid as u32), cg(main_pid)), (Some(a), Some(b)) if a == b)
}

/// Whether a user other than root could have written `path` or made it
/// point elsewhere (systemd's CHASE_SAFE check and the file owner): a
/// component that is a symlink not owned by root, a directory on the way
/// not owned by root or writable by group or others, or a file not owned
/// by root.
fn pid_file_questionable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let mut prefix = PathBuf::new();
    for c in path.components() {
        prefix.push(c);
        match fs::symlink_metadata(&prefix) {
            Ok(m) if m.file_type().is_symlink() && m.uid() != 0 => return true,
            Ok(_) => {}
            Err(_) => return true,
        }
    }
    let Ok(real) = fs::canonicalize(path) else { return true };
    let mut dir = real.parent();
    while let Some(d) = dir {
        match fs::metadata(d) {
            Ok(m) if m.uid() != 0 || m.mode() & 0o022 != 0 => return true,
            Ok(_) => {}
            Err(_) => return true,
        }
        dir = d.parent();
    }
    fs::metadata(&real).map_or(true, |m| m.uid() != 0)
}

/// MAINPID=: systemd's service_is_suitable_main_pid(), an existing process
/// in the unit's cgroup (not the helper); a root sender may name any.
fn main_pid_ok(pid: u32, sender_uid: u32, main_pid: u32, cgroup: Option<&str>) -> bool {
    if pid <= 1 || pid == std::process::id() || !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    if sender_uid == 0 {
        return true;
    }
    let cg = |pid: u32| fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    match cgroup {
        // Subgroups too (Delegate=yes).
        Some(dir) => cg(pid).and_then(|c| c.lines().find_map(|l| l.strip_prefix("0::")).map(str::to_string)).is_some_and(|rel| {
            let full = format!("{}{rel}", cgroup::CGROUP_ROOT);
            full == dir || full.starts_with(&format!("{dir}/"))
        }),
        None => matches!((cg(pid), cg(main_pid)), (Some(a), Some(b)) if a == b),
    }
}

fn env_set(env: &mut Vec<(String, String)>, k: &str, v: &str) {
    if let Some(e) = env.iter_mut().find(|(ek, _)| ek == k) {
        e.1 = v.to_string();
    } else {
        env.push((k.to_string(), v.to_string()));
    }
}

/// SuccessExitStatus= and friends: numbers, exit status names (systemd's
/// exit_status_from_string(): DATAERR, TEMPFAIL, ...) and signal names.
fn status_in(list: &[String], status: Status) -> bool {
    list.iter().any(|w| match status {
        Status::Exited(c) => w.parse::<i32>().ok().or_else(|| exit_status_number(w)) == Some(c),
        Status::Killed(s) => {
            w.parse::<i32>().is_err() && exit_status_number(w).is_none() && util::signal_number(w) == Some(s)
        }
    })
}

fn exit_status_number(name: &str) -> Option<i32> {
    const NAMES: &[(&str, i32)] = &[
        ("SUCCESS", 0), ("FAILURE", 1), ("INVALIDARGUMENT", 2), ("NOTIMPLEMENTED", 3),
        ("NOPERMISSION", 4), ("NOTINSTALLED", 5), ("NOTCONFIGURED", 6), ("NOTRUNNING", 7),
        ("USAGE", 64), ("DATAERR", 65), ("NOINPUT", 66), ("NOUSER", 67), ("NOHOST", 68),
        ("UNAVAILABLE", 69), ("SOFTWARE", 70), ("OSERR", 71), ("OSFILE", 72), ("CANTCREAT", 73),
        ("IOERR", 74), ("TEMPFAIL", 75), ("PROTOCOL", 76), ("NOPERM", 77), ("CONFIG", 78),
        ("CHDIR", 200), ("NICE", 201), ("FDS", 202), ("EXEC", 203), ("MEMORY", 204),
        ("LIMITS", 205), ("OOM_ADJUST", 206), ("SIGNAL_MASK", 207), ("STDIN", 208),
        ("STDOUT", 209), ("CHROOT", 210), ("IOPRIO", 211), ("TIMERSLACK", 212),
        ("SECUREBITS", 213), ("SETSCHEDULER", 214), ("CPUAFFINITY", 215), ("GROUP", 216),
        ("USER", 217), ("CAPABILITIES", 218), ("CGROUP", 219), ("SETSID", 220),
        ("CONFIRM", 221), ("STDERR", 222), ("PAM", 224), ("NETWORK", 225), ("NAMESPACE", 226),
        ("NO_NEW_PRIVILEGES", 227), ("SECCOMP", 228), ("SELINUX_CONTEXT", 229),
        ("PERSONALITY", 230), ("APPARMOR_PROFILE", 231), ("ADDRESS_FAMILIES", 232),
        ("RUNTIME_DIRECTORY", 233), ("CHOWN", 235), ("SMACK_PROCESS_LABEL", 236),
        ("KEYRING", 237), ("STATE_DIRECTORY", 238), ("CACHE_DIRECTORY", 239),
        ("LOGS_DIRECTORY", 240), ("CONFIGURATION_DIRECTORY", 241), ("NUMA_POLICY", 242),
        ("CREDENTIALS", 243), ("BPF", 245),
    ];
    NAMES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

fn describe(st: Status) -> String {
    match st {
        Status::Exited(c) => format!("exit status {c}"),
        Status::Killed(s) => format!("killed by SIG{}", util::signal_name(s)),
    }
}

fn result_for(st: Status) -> &'static str {
    match st {
        Status::Exited(_) => "exit-code",
        Status::Killed(_) => "signal",
    }
}

fn exit_code_for(st: Status) -> i32 {
    match st {
        Status::Exited(c) => c.max(1),
        Status::Killed(s) => 128 + s,
    }
}

fn group_list(user: &str, gid: u32) -> Vec<u32> {
    let c = cstr(user);
    let mut n: libc::c_int = 64;
    let mut groups: Vec<libc::gid_t> = vec![0; n as usize];
    unsafe {
        if libc::getgrouplist(c.as_ptr(), gid, groups.as_mut_ptr(), &mut n) < 0 {
            groups.resize(n as usize, 0);
            libc::getgrouplist(c.as_ptr(), gid, groups.as_mut_ptr(), &mut n);
        }
    }
    groups.truncate(n.max(0) as usize);
    groups
}

/// User= (DynamicUser= creates it), Group= and SupplementaryGroups=; the
/// error carries systemd's exit code for it.
fn resolve_credentials(unit: &Unit) -> Result<(Option<UserInfo>, Option<u32>, Vec<u32>), (i32, String)> {
    let user_err = |e| (EXIT_USER, e);
    let group_err = |g: &str| (EXIT_GROUP, format!("group {g} does not exist"));
    let mut user = None;
    if let Some(u) = unit.get("Service", "User") {
        user = Some(match util::lookup_user(&u) {
            Some(info) => info,
            None if unit.get_bool("Service", "DynamicUser", false) => ensure_system_user(&u).map_err(user_err)?,
            None => return Err(user_err(format!("user {u} does not exist"))),
        });
    } else if unit.get_bool("Service", "DynamicUser", false) {
        let name = unit.id.trim_end_matches(".service").replace(['@', '.'], "-");
        user = Some(ensure_system_user(&name).map_err(user_err)?);
    }
    let gid = match unit.get("Service", "Group") {
        Some(g) => Some(util::lookup_group(&g).ok_or_else(|| group_err(&g))?),
        None => user.as_ref().map(|u| u.gid),
    };
    let mut groups = Vec::new();
    if let (Some(u), Some(g)) = (&user, gid) {
        groups = group_list(&u.name, g);
    }
    for g in unit.get_words("Service", "SupplementaryGroups") {
        groups.push(util::lookup_group(&g).ok_or_else(|| group_err(&g))?);
    }
    Ok((user, gid, groups))
}

/// DynamicUser= approximation: a persistent system user.
fn ensure_system_user(name: &str) -> Result<UserInfo, String> {
    if let Some(u) = util::lookup_user(name) {
        return Ok(u);
    }
    let status = std::process::Command::new(tools::path("useradd"))
        .args(["--system", "--no-create-home", "--home-dir", "/", "--shell", "/usr/sbin/nologin", name])
        .status()
        .map_err(|e| format!("useradd: {e}"))?;
    if !status.success() {
        return Err(format!("useradd {name} failed"));
    }
    util::lookup_user(name).ok_or_else(|| format!("user {name} still missing after useradd"))
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The notify helper's command line carries every field across exec().
    #[test]
    fn notify_helper_args_round_trip() {
        let h = NotifyHelper {
            fd: 7,
            notify: None,
            main_pid: 1234,
            unit_uid: Some(0),
            timeout_us: None,
            st_dir: PathBuf::from("/run/u/units/a b.service"),
            store: PathBuf::from("/run/u/fdstore"),
            unit_id: "a b.service".into(),
            fdstore_max: 3,
            notify_reload: true,
            path: "@uinitconverter/notify/x".into(),
            post: false,
            config: PathBuf::from("/etc/u.conf"),
        };
        let args = h.to_args();
        assert_eq!(args[0], "notify-helper");
        assert_eq!(&args[args.len() - 2..], ["--unit", "a b.service"]);
        let g = NotifyHelper::from_args(&args[1..]).unwrap();
        assert_eq!(
            (g.fd, g.notify, g.main_pid, g.unit_uid, g.timeout_us, g.fdstore_max),
            (7, None, 1234, Some(0), None, 3)
        );
        assert_eq!((g.st_dir, g.store, g.config), (h.st_dir.clone(), h.store.clone(), h.config.clone()));
        assert_eq!((g.unit_id.as_str(), g.path.as_str(), g.notify_reload, g.post), ("a b.service", h.path.as_str(), true, false));
        let mut a = args[1..].to_vec();
        a.retain(|x| x != "--reload");
        a.extend(["--post".to_string(), "--notify-fd".into(), "9".into(), "--timeout-us".into(), "90000000".into()]);
        let g = NotifyHelper::from_args(&a).unwrap();
        assert_eq!((g.notify, g.timeout_us, g.notify_reload, g.post), (Some(9), Some(90_000_000), false, true));
        assert!(NotifyHelper::from_args(&["--fd".into(), "x".into()]).is_err());
        assert!(NotifyHelper::from_args(&["--bogus".into(), "1".into()]).is_err());
        assert!(NotifyHelper::from_args(&[]).is_err());
    }

    #[test]
    fn pid_file_trust() {
        // Root's file in root's directories: trusted as written.
        assert!(!pid_file_questionable(Path::new("/etc/passwd")));
        // A file this (unprivileged) test owns, or one that doesn't exist.
        let dir = std::env::temp_dir().join(format!("uinitconverter-pidfile-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let f = dir.join("x.pid");
        fs::write(&f, "1\n").unwrap();
        assert!(pid_file_questionable(&f));
        assert!(pid_file_questionable(&dir.join("missing.pid")));
        // A symlink not owned by root, even to root's file.
        let l = dir.join("link.pid");
        let _ = fs::remove_file(&l);
        std::os::unix::fs::symlink("/etc/passwd", &l).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(pid_file_questionable(&l));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
