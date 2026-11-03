//! Type=forking: start the daemon and find its main process through PIDFile= or the cgroup.

use super::*;

/// Whether a user other than root could have written `path` or made it
/// point elsewhere (systemd's CHASE_SAFE check and the file owner): a
/// component that is a symlink not owned by root, a directory on the way
/// not owned by root or writable by group or others, or a file not owned
/// by root.
pub(super) fn pid_file_questionable(path: &Path) -> bool {
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
    let Ok(real) = fs::canonicalize(path) else {
        return true;
    };
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

impl Runner {
    /// Type=forking: stay around as a subreaper and supervise the daemon.
    pub(super) fn run_forking(&mut self, main: &ExecCommand) -> i32 {
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if let Err(e) = self.setup_listen_fds() {
            self.soft_fail(&e);
            return 1;
        }
        self.st.stamp(keys::EXEC_MAIN_START);
        let st = self.spawn_wait(main, &[]);
        let deadline = self.deadline.take();
        if !st.ok() {
            self.st.set(
                keys::RESULT,
                if self.timed_out.get() {
                    "timeout"
                } else {
                    result_for(st)
                },
            );
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
            for sig in [
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGHUP,
                libc::SIGQUIT,
                libc::SIGUSR1,
                libc::SIGUSR2,
            ] {
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
                let mut p = libc::pollfd {
                    fd: pidfd,
                    events: libc::POLLIN,
                    revents: 0,
                };
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
    pub(super) fn wait_pid_file(
        &self,
        pidfile: &str,
        me: i32,
        deadline: Option<std::time::Instant>,
    ) -> Result<i32, &'static str> {
        let mut refused = false;
        loop {
            if let Some(p) =
                util::read_trimmed(Path::new(pidfile)).and_then(|s| s.parse::<i32>().ok())
            {
                // As systemd's service_load_pid_file(): a PID file someone
                // other than root could have written names a process of the
                // unit only, or the runner would signal any process as root.
                let cgroup = self.st.get(keys::CGROUP);
                let ok = p > 1
                    && p != me
                    && (!pid_file_questionable(Path::new(pidfile))
                        || main_pid_ok(p as u32, u32::MAX, me as u32, cgroup.as_deref()));
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
    pub(super) fn guess_main_pid(&self, me: i32) -> Option<i32> {
        let mut kids = Vec::new();
        for e in fs::read_dir("/proc").ok()?.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
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

    /// Type=notify with NotifyAccess=all: the main process may hand over to
    /// another with MAINPID= (podman's quadlet units: `podman run -d` exits
    /// once conmon, which sent MAINPID= and READY=1, runs the container).
    /// The runner stays as s6's process, forks the command, forwards
    /// signals to the current main process, and follows MAIN_PID (which
    /// the notify helper sets) instead of ending with the first process.
    pub(super) fn run_guarded(&mut self, main: &ExecCommand) -> i32 {
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        // Forwarding in place and signals blocked until FORWARD_PID names
        // the child, as exec_with_pam.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::sigprocmask(libc::SIG_BLOCK, &all, &mut old);
            for sig in [
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGHUP,
                libc::SIGQUIT,
                libc::SIGUSR1,
                libc::SIGUSR2,
            ] {
                libc::signal(sig, forward_signal as *const () as libc::sighandler_t);
            }
        }
        let child = unsafe { libc::fork() };
        if child == 0 {
            let (code, msg) = self.exec_command(main, &[]);
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
            return EXIT_EXEC;
        }
        FORWARD_PID.store(child, std::sync::atomic::Ordering::SeqCst);
        unsafe { libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
        if let Some(fd) = EXEC_FAIL_PIPE.with(|c| c.take()) {
            unsafe { libc::close(fd) };
        }
        self.st.set(keys::MAIN_PID, &child.to_string());

        let alive = |p: i32| unsafe { libc::kill(p, 0) } == 0;
        let announced = |st: &UnitState| {
            st.get(keys::MAIN_PID)
                .and_then(|p| p.parse::<i32>().ok())
                .filter(|&p| p > 1)
        };
        let mut main_pid = child;
        loop {
            // Until the current main process ends; MAINPID= may move it
            // meanwhile (the old one then is an ordinary process).
            let mut status: Option<i32> = None;
            let pidfd = |p: i32| unsafe { libc::syscall(libc::SYS_pidfd_open, p, 0) } as i32;
            let mut fd = pidfd(main_pid);
            loop {
                loop {
                    let mut s = 0;
                    let r = unsafe { libc::waitpid(-1, &mut s, libc::WNOHANG) };
                    if r <= 0 {
                        break;
                    }
                    if r == main_pid {
                        status = Some(s);
                    }
                }
                if status.is_some() || !alive(main_pid) {
                    break;
                }
                if let Some(p) = announced(&self.st).filter(|&p| p != main_pid && alive(p)) {
                    main_pid = p;
                    FORWARD_PID.store(p, std::sync::atomic::Ordering::SeqCst);
                    if fd >= 0 {
                        unsafe { libc::close(fd) };
                    }
                    fd = pidfd(p);
                    continue;
                }
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if fd >= 0 {
                    unsafe { libc::poll(&mut pfd, 1, 200) };
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            if fd >= 0 {
                unsafe { libc::close(fd) };
            }
            // Handed over? The MAINPID= may race the old process's exit:
            // after a clean exit, give it a second.
            let clean = status.is_none_or(|s| Status::from_wait(s) == Status::Exited(0));
            let mut next = None;
            for _ in 0..20 {
                if let Some(p) = announced(&self.st).filter(|&p| p != main_pid && alive(p)) {
                    next = Some(p);
                    break;
                }
                if !clean {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Some(p) = next {
                main_pid = p;
                FORWARD_PID.store(p, std::sync::atomic::Ordering::SeqCst);
                continue;
            }
            return match status.map(Status::from_wait) {
                Some(Status::Killed(sig)) => {
                    unsafe {
                        libc::signal(sig, libc::SIG_DFL);
                        libc::kill(libc::getpid(), sig);
                    }
                    128 + sig
                }
                Some(Status::Exited(c)) => c,
                None => 0,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_file_trust() {
        // Root's file in root's directories: trusted as written.
        assert!(!pid_file_questionable(Path::new("/etc/passwd")));
        // A file this (unprivileged) test owns, or one that doesn't exist.
        let dir =
            std::env::temp_dir().join(format!("uinitconverter-pidfile-{}", std::process::id()));
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
