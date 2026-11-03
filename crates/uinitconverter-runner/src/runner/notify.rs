//! Readiness helpers that outlive exec(): Type=notify, Type=dbus and Type=exec.

use super::*;

pub(super) static FORWARD_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub(super) static WOKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) extern "C" fn wake_signal(_: libc::c_int) {
    WOKEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub(super) extern "C" fn forward_signal(sig: libc::c_int) {
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
    pub(super) fn to_args(&self) -> Vec<String> {
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
    pub(super) fn exec(&self) {
        // The installed path, so ps shows initconvert-runner; after an
        // update replaced it, /proc/self/exe still is this binary.
        let exe = std::env::current_exe()
            .ok()
            .filter(|p| !p.to_string_lossy().ends_with(" (deleted)"))
            .unwrap_or_else(|| PathBuf::from("/proc/self/exe"));
        let mut argv: Vec<CString> = vec![cstr("initconvert-runner")];
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
        eprintln!(
            "uinitconverter[{}]: notify helper: exec {}: {}",
            self.unit_id,
            exe.display(),
            errno()
        );
    }

    /// The receive loop; `after_ready` runs once READY=1 came (ExecStartPost=).
    pub(super) fn run(&self, after_ready: &dyn Fn(u32)) {
        let (fd, notify, main_pid, unit_uid) = (self.fd, self.notify, self.main_pid, self.unit_uid);
        let (store, unit_id, fdstore_max) = (&self.store, &self.unit_id, self.fdstore_max);
        let st = UnitState::new(self.st_dir.clone());
        // Present only with a helper that counts reloads, so cmd_reload
        // doesn't wait on a service started by an older binary.
        if self.notify_reload {
            st.set(
                keys::RELOADS_DONE,
                &st.get_u64(keys::RELOADS_DONE).to_string(),
            );
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
        let mut deadline = self
            .timeout_us
            .map(|t| util::now_monotonic_usec().saturating_add(t));
        loop {
            let mut pfds = [
                libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: pidfd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let nfds = if pidfd >= 0 { 2 } else { 1 };
            let wait = match (ready, deadline) {
                _ if pidfd < 0 => 1000,
                (false, Some(d)) => (d.saturating_sub(util::now_monotonic_usec()) / 1000)
                    .min(i32::MAX as u64) as i32,
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
                let Ok((data, fds, cred)) = crate::fdstore::recv(fd) else {
                    continue;
                };
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
                        st.set(
                            keys::RELOADS_DONE,
                            &(st.get_u64(keys::RELOADS_DONE) + 1).to_string(),
                        );
                    } else if line == "STOPPING=1" {
                        // The daemon sees the state change and stops the
                        // unit; BindsTo=/PartOf= units follow (gnome-session
                        // monitor's logout).
                        st.set(keys::STOPPING, "1");
                    } else if let Some(s) = line.strip_prefix("STATUS=") {
                        st.set(keys::STATUS_TEXT, s);
                    } else if let (Some(n), false) =
                        (line.strip_prefix("EXTEND_TIMEOUT_USEC="), ready)
                    {
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
                        if n.len() <= 255
                            && n.bytes().all(|b| (b' '..=b'~').contains(&b) && b != b':')
                        {
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
                        use std::os::fd::AsFd;
                        if let Err(e) =
                            crate::fdstore::put(store, unit_id, &fdname, f.as_fd(), fdstore_max)
                        {
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
            if !ready
                && deadline
                    .map(|d| util::now_monotonic_usec() >= d)
                    .unwrap_or(false)
            {
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

/// `initconvert-runner notify-helper ...` (started by `run`, see
/// NotifyHelper). Only the fds it was given stay open: the runner's
/// LISTEN_FDS are the service's.
pub fn notify_helper_main(args: &[String]) -> i32 {
    let h = match NotifyHelper::from_args(args) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("initconvert-runner notify-helper: {e}");
            return 2;
        }
    };
    let keep = [Some(h.fd), h.notify];
    let open: Vec<i32> = fs::read_dir("/proc/self/fd")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect()
        })
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
            let code = match Config::load(Some(&h.config))
                .and_then(|cfg| Runner::new(cfg, &h.unit_id, true))
            {
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
pub(super) fn double_fork<F: FnOnce()>(f: F) -> Result<(), String> {
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
pub(super) fn notify_sender_ok(
    cred: Option<libc::ucred>,
    main_pid: u32,
    unit_uid: Option<u32>,
) -> bool {
    let Some(c) = cred else { return false };
    if c.uid == 0 || Some(c.uid) == unit_uid || c.pid as u32 == main_pid {
        return true;
    }
    let cg = |pid: u32| fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    matches!((cg(c.pid as u32), cg(main_pid)), (Some(a), Some(b)) if a == b)
}

/// MAINPID=: systemd's service_is_suitable_main_pid(), an existing process
/// in the unit's cgroup (not the helper); a root sender may name any.
pub(super) fn main_pid_ok(pid: u32, sender_uid: u32, main_pid: u32, cgroup: Option<&str>) -> bool {
    if pid <= 1 || pid == std::process::id() || !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    if sender_uid == 0 {
        return true;
    }
    let cg = |pid: u32| fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    match cgroup {
        // Subgroups too (Delegate=yes).
        Some(dir) => cg(pid)
            .and_then(|c| {
                c.lines()
                    .find_map(|l| l.strip_prefix("0::"))
                    .map(str::to_string)
            })
            .is_some_and(|rel| {
                let full = format!("{}{rel}", cgroup::CGROUP_ROOT);
                full == dir || full.starts_with(&format!("{dir}/"))
            }),
        None => matches!((cg(pid), cg(main_pid)), (Some(a), Some(b)) if a == b),
    }
}

impl Runner {
    /// Type=simple/exec: ready once exec() succeeds (detected via a CLOEXEC pipe).
    pub(super) fn start_exec_helper(&mut self) -> Result<(), String> {
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
    pub(super) fn start_notify_helper(&mut self, main_pid: u32) -> Result<String, String> {
        let dir = self.cfg.run_dir.join("notify");
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        crate::selinux::restorecon(&dir.display().to_string());
        let inv = self
            .st
            .get(keys::INVOCATION)
            .unwrap_or_else(|| main_pid.to_string());
        let path = dir.join(&inv[..inv.len().min(32)]);
        let _ = fs::remove_file(&path);
        let path_s = path.display().to_string();

        // sun_path is 108 bytes; fall back to an abstract socket for long paths.
        let path_s = if path_s.len() >= 100 {
            format!("@uinitconverter/notify/{inv}")
        } else {
            path_s
        };
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
        let unit_uid = Some(
            self.user
                .as_ref()
                .map(|u| u.uid)
                .unwrap_or_else(|| unsafe { libc::geteuid() }),
        );

        let h = NotifyHelper {
            fd,
            notify: self.notify_fd,
            main_pid,
            unit_uid,
            // None: no start timeout (TimeoutStartSec=infinity or 0).
            timeout_us: self
                .start_timeout()
                .map(|t| t.as_micros().min(u64::MAX as u128) as u64),
            st_dir: self.st.dir.clone(),
            store: self.cfg.run_dir.join("fdstore"),
            unit_id: self.unit.id.clone(),
            fdstore_max: self
                .unit
                .get("Service", "FileDescriptorStoreMax")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
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

    /// Type=dbus: ready once BusName= appears on the manager's bus (the
    /// system bus, or a user manager's BUS_ADDRESS).
    pub(super) fn start_dbus_helper(&mut self, main_pid: u32) {
        let notify = self.notify_fd;
        let tries = self
            .start_timeout()
            .map(|t| (t.as_millis() / 100) as u32)
            .unwrap_or(u32::MAX);
        let Some(bus_name) = self.unit.get("Service", "BusName") else {
            let _ = self.start_exec_helper();
            return;
        };
        let user_bus = self.cfg.bus_address.clone().filter(|_| self.cfg.user_mode);
        let _ = double_fork(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            // Ok(true) ready, Ok(false) died, Err timeout/bus error.
            let outcome: Result<bool, &str> = rt.block_on(async {
                let conn = match &user_bus {
                    Some(a) => match zbus::connection::Builder::address(a.as_str()) {
                        Ok(b) => b.build().await,
                        Err(e) => Err(e),
                    },
                    None => zbus::Connection::system().await,
                };
                let Ok(conn) = conn else {
                    return Err("no bus");
                };
                let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
                    return Err("no system bus");
                };
                let Ok(name) = zbus::names::BusName::try_from(bus_name.as_str()) else {
                    return Err("bad BusName=");
                };
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
                Ok(false) => self
                    .st
                    .set(keys::START_FAILED, "exited before acquiring its bus name"),
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
            (
                g.fd,
                g.notify,
                g.main_pid,
                g.unit_uid,
                g.timeout_us,
                g.fdstore_max
            ),
            (7, None, 1234, Some(0), None, 3)
        );
        assert_eq!(
            (g.st_dir, g.store, g.config),
            (h.st_dir.clone(), h.store.clone(), h.config.clone())
        );
        assert_eq!(
            (g.unit_id.as_str(), g.path.as_str(), g.notify_reload, g.post),
            ("a b.service", h.path.as_str(), true, false)
        );
        let mut a = args[1..].to_vec();
        a.retain(|x| x != "--reload");
        a.extend([
            "--post".to_string(),
            "--notify-fd".into(),
            "9".into(),
            "--timeout-us".into(),
            "90000000".into(),
        ]);
        let g = NotifyHelper::from_args(&a).unwrap();
        assert_eq!(
            (g.notify, g.timeout_us, g.notify_reload, g.post),
            (Some(9), Some(90_000_000), false, true)
        );
        assert!(NotifyHelper::from_args(&["--fd".into(), "x".into()]).is_err());
        assert!(NotifyHelper::from_args(&["--bogus".into(), "1".into()]).is_err());
        assert!(NotifyHelper::from_args(&[]).is_err());
    }
}
