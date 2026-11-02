//! Accept=yes sockets: `accept` binds the socket and execs s6-ipcserverd on
//! it; s6-ipcserverd forks `connection` for every client, which runs one
//! instance of the socket's template (`<prefix>@<n>-<pid>-<uid>.service`)
//! with the connection, the way systemd's socket unit does.

use super::*;
use crate::acceptspec;

fn socket_state(cfg: &Config, id: &str) -> UnitState {
    let st = crate::state::for_unit(&cfg.run_dir, id);
    st.ensure();
    st
}

fn load_socket(cfg: &Config, name: &str) -> Result<Unit, String> {
    let sock = Loader::new(cfg).load(name);
    if !sock.is_loaded() {
        return Err(format!("unit {name}: {}", sock.load_state.as_str()));
    }
    if !acceptspec::is_accept(&sock) {
        return Err(format!("{}: not an Accept=yes socket", sock.id));
    }
    Ok(sock)
}

/// The run script of an Accept=yes socket. Only returns on failure.
pub fn accept_main(cfg: &Config, name: &str) -> i32 {
    let sock = match load_socket(cfg, name) {
        Ok(s) => s,
        Err(e) => {
            log(name, &e);
            return 1;
        }
    };
    let id = sock.id.clone();
    let st = socket_state(cfg, &id);
    st.stamp(keys::INACTIVE_EXIT);
    st.clear(keys::START_FAILED);
    let fail = |e: &str| -> i32 {
        log(&id, e);
        st.set(keys::RESULT, "resources");
        st.stamp(keys::INACTIVE_ENTER);
        1
    };
    let tmpl = Loader::new(cfg).load(&acceptspec::template(&sock));
    if let Some(why) = acceptspec::unsupported(&sock, &tmpl) {
        return fail(&why);
    }
    // Connections still running from an earlier run of the socket keep
    // their files; only the ones whose instance is gone are dropped.
    let conns = st.dir.join(acceptspec::CONNECTIONS_DIR);
    let _ = fs::create_dir_all(&conns);
    if let Ok(rd) = fs::read_dir(&conns) {
        for e in rd.flatten() {
            let alive = fs::read_to_string(e.path())
                .ok()
                .and_then(|p| p.trim().parse::<i32>().ok())
                .is_some_and(|p| unsafe { libc::kill(p, 0) } == 0);
            if !alive {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    match cgroup::enter(&sock, std::process::id(), None) {
        Ok(dir) => st.set(keys::CGROUP, &dir.display().to_string()),
        Err(e) => {
            if unsafe { libc::geteuid() } == 0 {
                log(&id, &format!("cgroup: {e}"));
            }
        }
    }

    // As systemd: the socket carries the label of the template's program.
    let label = tmpl
        .get_all("Service", "ExecStart")
        .first()
        .and_then(|l| execcmd::parse(l).ok())
        .and_then(|c| c.into_iter().next())
        .and_then(|c| crate::selinux::exec_label(&execcmd::resolve_path(&c.path)));
    crate::selinux::set_socket_label(label.as_deref());
    let bound = socket::bind_all(&sock);
    crate::selinux::set_socket_label(None);
    let fd = match bound {
        Ok(mut b) if b.len() == 1 => b.remove(0).fd,
        Ok(_) => return fail("expected exactly one socket"),
        Err(e) => return fail(&e),
    };

    let ((c, lc), _) = acceptspec::limits(&sock);
    st.set(keys::RESULT, "success");
    st.stamp(keys::ACTIVE_ENTER);
    unsafe {
        libc::dup2(fd, 0);
        libc::close(fd);
        // Ready: clients wait in the backlog until s6-ipcserverd accepts.
        if libc::fcntl(3, libc::F_GETFD) >= 0 {
            libc::write(3, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(3);
        }
    }
    let runner = cfg.runner.display().to_string();
    let mut argv: Vec<String> = vec![
        tools::path("s6-ipcserverd"),
        "-c".into(),
        c.to_string(),
        "-C".into(),
        lc.to_string(),
        "--".into(),
        runner,
        "connection".into(),
    ];
    argv.extend(cfg.runner_args().split_whitespace().map(String::from));
    argv.push("--".into());
    argv.push(id.clone());
    let err = exec_argv(&argv);
    fail(&format!("exec {}: {err}", argv[0]))
}

fn exec_argv(argv: &[String]) -> String {
    let c: Vec<CString> = argv.iter().map(|a| cstr(a)).collect();
    let mut p: Vec<*const libc::c_char> = c.iter().map(|a| a.as_ptr()).collect();
    p.push(std::ptr::null());
    unsafe { libc::execvp(p[0], p.as_ptr()) };
    errno()
}

/// The finish script: s6-ipcserverd stopped (a stop) or died (s6 restarts it).
pub fn accept_finish(cfg: &Config, name: &str) -> i32 {
    let Ok(sock) = load_socket(cfg, name) else {
        return 0;
    };
    let st = socket_state(cfg, &sock.id);
    socket::remove_paths(&sock);
    st.stamp(keys::ACTIVE_EXIT);
    st.stamp(keys::INACTIVE_ENTER);
    0
}

/// Peer pid and uid of a unix connection.
fn peer_cred(fd: i32) -> Option<(i32, u32)> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    (r == 0 && cred.pid > 0).then_some((cred.pid, cred.uid))
}

/// systemd's n_accepted: this connection's number, counted under a lock.
fn next_number(st: &UnitState) -> u64 {
    let lock = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(st.dir.join(acceptspec::LOCK));
    let fd = lock.as_ref().map(std::os::fd::AsRawFd::as_raw_fd).ok();
    if let Some(fd) = fd {
        unsafe { libc::flock(fd, libc::LOCK_EX) };
    }
    let n = st.get_u64(acceptspec::ACCEPTED);
    st.set(acceptspec::ACCEPTED, &(n + 1).to_string());
    n
}

/// The cgroup this process is in (cgroup2's "0::/path").
fn own_cgroup() -> Option<PathBuf> {
    let s = fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = s.lines().find_map(|l| l.strip_prefix("0::"))?;
    Some(Path::new(cgroup::CGROUP_ROOT).join(rel.trim_start_matches('/')))
}

/// What s6-ipcserverd runs for each client: the connection is on stdin
/// and stdout.
pub fn connection_main(cfg: &Config, name: &str) -> i32 {
    // The connection goes out of the way; stdout is the log again (the
    // runner logs to stderr, which s6-ipcserverd inherited from s6).
    let conn = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 100) };
    unsafe {
        libc::dup2(2, 1);
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if null >= 0 {
            libc::dup2(null, 0);
            libc::close(null);
        }
    }
    if conn < 0 {
        log(name, &format!("connection fd: {}", errno()));
        return 1;
    }
    let sock = match load_socket(cfg, name) {
        Ok(s) => s,
        Err(e) => {
            log(name, &e);
            return 1;
        }
    };
    let sst = socket_state(cfg, &sock.id);
    let n = next_number(&sst);
    let instance = acceptspec::instance_name(&acceptspec::template(&sock), n, peer_cred(conn));
    let mut r = match Runner::new(cfg.clone(), &instance, true) {
        Ok(r) => r,
        Err(e) => {
            log(&instance, &e);
            return 1;
        }
    };
    let mark = sst.dir.join(acceptspec::CONNECTIONS_DIR).join(&instance);
    let _ = fs::write(&mark, std::process::id().to_string());
    r.conn = Some(conn);
    let origin = own_cgroup();
    let ok = r.run_connection();
    // Out of the instance's cgroup, so it can go.
    if let (Some(dir), Some(back)) = (r.cgroup_dir(), origin) {
        if dir != back
            && fs::write(back.join("cgroup.procs"), std::process::id().to_string()).is_ok()
        {
            cgroup::remove(&dir);
        }
    }
    let _ = fs::remove_dir_all(&r.st.dir);
    let _ = fs::remove_file(&mark);
    i32::from(!ok)
}

impl Runner {
    /// ExecStartPre=, ExecStart= with the connection, ExecStartPost=,
    /// ExecStop=, then ExecStopPost= and the usual cleanup, all in the
    /// foreground. True when it succeeded.
    fn run_connection(&mut self) -> bool {
        self.enter_cgroup();
        if let Err(e) = self.prepare_start() {
            log(self.id(), &e);
            self.close_conn();
            return false;
        }
        match self.check_conditions() {
            Ok(true) => {}
            Ok(false) => {
                log(self.id(), "condition check failed, skipped");
                self.close_conn();
                return true;
            }
            Err(e) => {
                log(self.id(), &e);
                self.close_conn();
                return false;
            }
        }
        let oneshot = self.service_type() == "oneshot";
        self.st.stamp(keys::EXEC_MAIN_START);
        let mut failed: Option<Status> = None;
        self.deadline
            .set(self.start_timeout().map(|t| std::time::Instant::now() + t));
        if let Err(st) = self.run_list("ExecStartPre", &[]) {
            failed = Some(st);
        }
        if failed.is_none() {
            if !oneshot {
                // TimeoutStartSec= limits only the start, not the session.
                self.deadline.set(None);
            }
            self.conn_main.set(true);
            let r = self.run_list("ExecStart", &[]);
            self.conn_main.set(false);
            // The client sees the end of the connection now, not after
            // ExecStopPost=.
            self.close_conn();
            if let Err(st) = r {
                failed = Some(st);
            }
        }
        self.deadline.set(None);
        if failed.is_none() {
            if let Err(st) = self.run_list("ExecStartPost", &[]) {
                failed = Some(st);
            }
        }
        self.close_conn();
        self.st.stamp(keys::EXEC_MAIN_EXIT);
        let (result, status) = match failed {
            None => {
                let _ = self.run_list("ExecStop", &[]);
                ("success", Status::Exited(0))
            }
            Some(st) => {
                let result = if self.timed_out.get() {
                    "timeout"
                } else {
                    result_for(st)
                };
                (result, st)
            }
        };
        self.st.set(keys::RESULT, result);
        self.cleanup(result, status, false);
        self.on_result(failed.is_none());
        failed.is_none()
    }

    fn close_conn(&mut self) {
        if let Some(fd) = self.conn.take() {
            unsafe { libc::close(fd) };
        }
    }

    /// The connection for the process being started: ExecStart= only.
    pub(super) fn main_conn(&self) -> Option<i32> {
        self.conn.filter(|_| self.conn_main.get())
    }
}
