//! What happens between fork() and exec(): credentials, limits, sandbox, PAM.

use super::*;

thread_local! {
    pub(super) static EXEC_FAIL_PIPE: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

// systemd's exit codes for setup failures (EXIT_CHDIR etc).
pub(super) const EXIT_CHDIR: i32 = 200;
pub(super) const EXIT_EXEC: i32 = 203;
pub(super) const EXIT_MEMORY: i32 = 204;
pub(super) const EXIT_LIMITS: i32 = 205;
pub(super) const EXIT_STDIN: i32 = 208;
pub(super) const EXIT_STDOUT: i32 = 209;
pub(super) const EXIT_CHROOT: i32 = 210;
pub(super) const EXIT_GROUP: i32 = 216;
pub(super) const EXIT_USER: i32 = 217;
pub(super) const EXIT_CAPABILITIES: i32 = 218;
pub(super) const EXIT_NAMESPACE: i32 = 226;
pub(super) const EXIT_SECCOMP: i32 = 228;

pub(super) const LIMITS: &[(&str, libc::__rlimit_resource_t, bool)] = &[
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

pub(super) fn group_list(user: &str, gid: u32) -> Vec<u32> {
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
pub(super) fn resolve_credentials(
    unit: &Unit,
) -> Result<(Option<UserInfo>, Option<u32>, Vec<u32>), (i32, String)> {
    let user_err = |e| (EXIT_USER, e);
    let group_err = |g: &str| (EXIT_GROUP, format!("group {g} does not exist"));
    let mut user = None;
    if let Some(u) = unit.get("Service", "User") {
        user = Some(match util::lookup_user(&u) {
            Some(info) => info,
            None if unit.get_bool("Service", "DynamicUser", false) => {
                ensure_system_user(&u).map_err(user_err)?
            }
            None => return Err(user_err(format!("user {u} does not exist"))),
        });
    } else if unit.get_bool("Service", "DynamicUser", false) {
        let name = unit
            .id
            .trim_end_matches(".service")
            .replace(['@', '.'], "-");
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
pub(super) fn ensure_system_user(name: &str) -> Result<UserInfo, String> {
    if let Some(u) = util::lookup_user(name) {
        return Ok(u);
    }
    let status = std::process::Command::new(tools::path("useradd"))
        .args([
            "--system",
            "--no-create-home",
            "--home-dir",
            "/",
            "--shell",
            "/usr/sbin/nologin",
            name,
        ])
        .status()
        .map_err(|e| format!("useradd: {e}"))?;
    if !status.success() {
        return Err(format!("useradd {name} failed"));
    }
    util::lookup_user(name).ok_or_else(|| format!("user {name} still missing after useradd"))
}

impl Runner {
    /// Apply everything systemd applies to a service process, then exec().
    /// Only returns on failure, with the exit code to use.
    pub(super) fn exec_command(
        &self,
        cmd: &ExecCommand,
        extra_env: &[(String, String)],
    ) -> (i32, String) {
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

        // Accept=yes: the connection on fd 3 for ExecStart=, as systemd
        // passes it (and on stdin/stdout with StandardInput=socket).
        let mut with_conn = extra_env.to_vec();
        if let Some(fd) = self.main_conn() {
            unsafe { libc::dup2(fd, 3) };
            with_conn.push(("LISTEN_FDS".into(), "1".into()));
            with_conn.push(("LISTEN_PID".into(), std::process::id().to_string()));
            with_conn.push(("LISTEN_FDNAMES".into(), self.conn_name.clone()));
        }
        let extra_env = &with_conn[..];
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

        let own_ids = unsafe { (libc::getuid(), libc::getgid()) };
        let sets_ids = !cmd.privileged && !cmd.no_setuid && self.is_root;
        if sets_ids {
            if let Err(e) = self.switch_groups() {
                return e;
            }
        }
        if sandboxed {
            if let Err(e) = self.enter_user_namespace(own_ids, sets_ids) {
                return e;
            }
        }

        if !cmd.privileged {
            if let Some(root) = self.unit.get("Service", "RootDirectory") {
                let c = cstr(&root);
                if unsafe { libc::chroot(c.as_ptr()) } < 0 {
                    return (EXIT_CHROOT, format!("chroot {root}: {}", errno()));
                }
            }
        }

        let wd = self
            .unit
            .get("Service", "WorkingDirectory")
            .unwrap_or_else(|| "/".into());
        let (wd_optional, wd) = match wd.strip_prefix('-') {
            Some(w) => (true, w.to_string()),
            None => (false, wd),
        };
        let wd = if wd == "~" {
            self.user
                .as_ref()
                .map(|u| u.home.clone())
                .unwrap_or_else(|| "/root".into())
        } else {
            wd
        };
        let c = cstr(&wd);
        if unsafe { libc::chdir(c.as_ptr()) } < 0 && !wd_optional {
            return (EXIT_CHDIR, format!("chdir {wd}: {}", errno()));
        }

        // Root once exec()ed: the user switch below does not happen.
        let switches_user = !cmd.privileged
            && !cmd.no_setuid
            && self.user.as_ref().map(|u| u.uid != 0).unwrap_or(false);
        let stays_root = self.is_root && !switches_user;
        // '+' commands skip the sandbox, NoNewPrivileges= included (systemd
        // sets it only with needs_sandboxing).
        let nnp = !cmd.privileged && sandbox::no_new_privileges(&self.unit, stays_root);
        if nnp {
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        }

        let bset = if self.is_root && !cmd.privileged {
            sandbox::bounding_set(&self.unit)
        } else {
            None
        };
        if let Some(bset) = bset {
            if let Err(e) = sandbox::drop_bounding_set(bset) {
                return (EXIT_CAPABILITIES, e);
            }
        }

        // As systemd, ambient capabilities outside the bounding set are left
        // out: capset() would refuse them and fail the start.
        let ambient = sandbox::parse_capset(&self.unit, "AmbientCapabilities")
            .map(|a| bset.map_or(a, |b| a & b));
        if sets_ids {
            if let Err(e) = self.switch_user(ambient) {
                return e;
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

    /// As root: Group= and SupplementaryGroups=, or User='s. The error carries
    /// systemd's exit code for it.
    pub(super) fn switch_groups(&self) -> Result<(), (i32, String)> {
        let setgroups = |groups: &[libc::gid_t]| {
            if unsafe { libc::setgroups(groups.len(), groups.as_ptr()) } < 0 {
                return Err((EXIT_GROUP, format!("setgroups: {}", errno())));
            }
            Ok(())
        };
        let gid = match &self.user {
            Some(u) if u.uid != 0 => {
                setgroups(&self.groups)?;
                Some(self.gid.unwrap_or(u.gid))
            }
            Some(_) => self.gid,
            None => {
                if !self.groups.is_empty() {
                    setgroups(&self.groups)?;
                }
                self.gid
            }
        };
        if let Some(g) = gid {
            if unsafe { libc::setresgid(g, g, g) } < 0 {
                return Err((EXIT_GROUP, format!("setgid {g}: {}", errno())));
            }
        }
        Ok(())
    }

    /// PrivateUsers=, between switch_groups() and switch_user(). `own_ids` are the
    /// IDs from before switch_groups(); `sets_ids` says if the command gets the
    /// unit's IDs at all ('!' commands don't).
    pub(super) fn enter_user_namespace(
        &self,
        own_ids: (u32, u32),
        sets_ids: bool,
    ) -> Result<(), (i32, String)> {
        let mode = sandbox::private_users(&self.unit);
        if mode == sandbox::PrivateUsers::No {
            return Ok(());
        }
        let user = self.user.as_ref().filter(|_| sets_ids);
        let gid = self.gid.filter(|_| sets_ids).or(user.map(|u| u.gid));
        sandbox::enter_user_namespace(mode, own_ids, (user.map(|u| u.uid), gid))
            .map_err(|e| (EXIT_USER, e))
    }

    /// As root: User=, after switch_groups(), then the ambient capabilities.
    /// The error carries systemd's exit code for it.
    pub(super) fn switch_user(&self, ambient: Option<u64>) -> Result<(), (i32, String)> {
        if let Some(u) = self.user.as_ref().filter(|u| u.uid != 0) {
            if ambient.is_some() {
                unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 1, 0, 0, 0) };
            }
            if unsafe { libc::setresuid(u.uid, u.uid, u.uid) } < 0 {
                return Err((EXIT_USER, format!("setuid {}: {}", u.uid, errno())));
            }
        }
        // Ambient capabilities matter only after dropping root; a root service
        // keeps its full capability set (raising "ambient" would shrink it).
        let non_root_user = self.user.as_ref().map(|u| u.uid != 0).unwrap_or(false);
        if let (Some(caps), true) = (ambient, non_root_user) {
            if let Err(e) = sandbox::raise_ambient(caps) {
                return Err((EXIT_CAPABILITIES, e));
            }
        }
        Ok(())
    }

    /// `confine`: the unit's cgroup, limits, sandbox and user on this process,
    /// then exec() ARGV with the environment and file descriptors it was given (a
    /// run script's readiness pipe). Only returns on failure.
    pub fn cmd_confine(&mut self, argv: &[String]) -> i32 {
        if argv.is_empty() {
            eprintln!("uinitconverter-runner: confine: no command");
            return 1;
        }
        let Err(e) = self.confine(argv);
        log(self.id(), &format!("confine: {e}"));
        1
    }

    pub(super) fn confine(&mut self, argv: &[String]) -> Result<std::convert::Infallible, String> {
        self.enter_cgroup();
        if self.is_root {
            // Also tells the sandbox which of them stay writable.
            self.create_directories()?;
            self.sandbox.prepare()?;
            self.sandbox.apply()?;
        }
        self.apply_limits()?;
        if self.is_root {
            let own_ids = unsafe { (libc::getuid(), libc::getgid()) };
            self.switch_groups().map_err(|(_, e)| e)?;
            self.enter_user_namespace(own_ids, true)
                .map_err(|(_, e)| e)?;
        }
        let switches_user = self.user.as_ref().is_some_and(|u| u.uid != 0);
        if sandbox::no_new_privileges(&self.unit, self.is_root && !switches_user) {
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        }
        if self.is_root {
            let bset = sandbox::bounding_set(&self.unit);
            if let Some(bset) = bset {
                sandbox::drop_bounding_set(bset)?;
            }
            let ambient = sandbox::parse_capset(&self.unit, "AmbientCapabilities")
                .map(|a| bset.map_or(a, |b| a & b));
            self.switch_user(ambient).map_err(|(_, e)| e)?;
        }
        crate::seccomp::apply_restrictions(&self.unit)?;
        if let Some(f) = crate::seccomp::Filter::from_unit(&self.unit)
            .map_err(|e| format!("SystemCallFilter=: {e}"))?
        {
            f.apply()?;
        }
        let c_argv: Vec<CString> = argv.iter().map(|a| cstr(a)).collect();
        let mut p_argv: Vec<*const libc::c_char> = c_argv.iter().map(|c| c.as_ptr()).collect();
        p_argv.push(std::ptr::null());
        unsafe { libc::execvp(p_argv[0], p_argv.as_ptr()) };
        Err(format!("exec {}: {}", argv[0], errno()))
    }

    pub(super) fn apply_limits(&self) -> Result<(), String> {
        if let Some(m) = self
            .unit
            .get("Service", "UMask")
            .and_then(|m| u32::from_str_radix(&m, 8).ok())
        {
            unsafe { libc::umask(m) };
        } else {
            unsafe { libc::umask(0o022) };
        }
        if let Some(n) = self
            .unit
            .get("Service", "Nice")
            .and_then(|n| n.parse::<i32>().ok())
        {
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, n) };
        }
        if let Some(adj) = self.unit.get("Service", "OOMScoreAdjust") {
            let _ = fs::write("/proc/self/oom_score_adj", adj);
        }
        for (key, res, bytes) in LIMITS {
            let Some(v) = self.unit.get("Service", key) else {
                continue;
            };
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
            let rl = libc::rlimit {
                rlim_cur: soft,
                rlim_max: hard,
            };
            if unsafe { libc::setrlimit(*res as _, &rl) } < 0 {
                return Err(format!("setrlimit {key}: {}", errno()));
            }
        }
        Ok(())
    }

    /// fork + exec a command and wait for it.
    pub(super) fn spawn_wait(&self, cmd: &ExecCommand, extra_env: &[(String, String)]) -> Status {
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
        // A connection's ExecStart= process is its main process (the daemon
        // shows it, and `systemctl stop` signals it).
        if self.main_conn().is_some() {
            self.st.set(keys::MAIN_PID, &pid.to_string());
            self.st.stamp(keys::ACTIVE_ENTER);
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
                    log(
                        self.id(),
                        &format!("{} timeout, terminating", self.deadline_what.get()),
                    );
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

    /// Run a list of Exec*= settings in order. Returns false on a non-ignored failure.
    pub(super) fn run_list(&self, key: &str, extra_env: &[(String, String)]) -> Result<(), Status> {
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
                let success = key == "ExecStart"
                    && status_in(&self.unit.get_words("Service", "SuccessExitStatus"), st);
                if !st.ok() && !success && !cmd.ignore_failure {
                    log(
                        self.id(),
                        &format!("{key}={} failed: {}", cmd.path, describe(st)),
                    );
                    return Err(st);
                }
            }
        }
        Ok(())
    }

    pub(super) fn enter_cgroup(&self) {
        let pid = std::process::id();
        let owner = self
            .user
            .as_ref()
            .map(|u| (u.uid, self.gid.unwrap_or(u.gid)));
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

    pub(super) fn cgroup_dir(&self) -> Option<PathBuf> {
        self.st.get(keys::CGROUP).map(PathBuf::from)
    }

    /// PAMName=: open a PAM session here, run the service in a child, and
    /// stay around (forwarding signals) to close the session afterwards.
    pub(super) fn exec_with_pam(&self, service: &str, main: &ExecCommand) -> i32 {
        const EXIT_PAM: i32 = 224;
        let user = self
            .user
            .as_ref()
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "root".into());
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
                if ![
                    libc::SIGKILL,
                    libc::SIGSTOP,
                    libc::SIGCHLD,
                    libc::SIGSEGV,
                    libc::SIGBUS,
                    libc::SIGFPE,
                    libc::SIGILL,
                ]
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
            log(
                self.id(),
                &format!(
                    "closing the PAM session took {:.1} s",
                    t.elapsed().as_secs_f64()
                ),
            );
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
}
