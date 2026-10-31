//! Socket activation: binding the unit's sockets, LISTEN_FDS, and waiting for the first client.

use super::*;

impl Runner {
    /// Socket units that hand fds to this service.
    pub(super) fn socket_units(&self) -> Vec<Unit> {
        let ld = Loader::new(&self.cfg);
        let mut names = self.unit.get_words("Service", "Sockets");
        if names.is_empty() {
            let stem = self.unit.id.trim_end_matches(".service");
            let default = format!("{stem}.socket");
            let wanted = self.unit.get_words("Unit", "Requires").contains(&default)
                || self.unit.get_words("Unit", "Wants").contains(&default);
            if wanted
                || install::is_enabled(&self.cfg, &default)
                || socket::is_requested(&self.cfg.run_dir, &default)
            {
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
                    let Ok(text) = fs::read_to_string(e.path()) else {
                        continue;
                    };
                    if !text.contains("Service=") {
                        continue;
                    }
                    let s = ld.load(&n);
                    if s.get("Socket", "Service")
                        .map(|v| self.unit.names.contains(&v))
                        .unwrap_or(false)
                        && (install::is_enabled(&self.cfg, &n)
                            || socket::is_requested(&self.cfg.run_dir, &n))
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
    pub(super) fn bind_sockets(
        &self,
        socks: &[Unit],
    ) -> Result<Vec<(String, std::os::fd::OwnedFd)>, String> {
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
            let _ = crate::fdstore::put(
                &store,
                &key,
                &b.name,
                owned.as_raw_fd(),
                crate::fdstore::SOCKETS_MAX,
            );
            fds.push((b.name, owned));
        }
        Ok(fds)
    }

    /// The first start of an on-demand service: listen on its sockets
    /// (held by the fd store) and wait for activity, like systemd's socket
    /// unit in front of an inactive service. Not when s6 restarts one that
    /// ran (Restart=, s6-rc-update) or a job asked for the start; and
    /// without the fd store it just starts.
    pub(super) fn should_wait_on_demand(&self) -> bool {
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
    pub(super) fn setup_listen_fds(&mut self) -> Result<(), String> {
        use std::os::fd::{IntoRawFd, OwnedFd};
        let store = self.cfg.run_dir.join("fdstore");
        let mut fds: Vec<(String, OwnedFd)> = Vec::new();
        let socks = self.socket_units();
        if !socks.is_empty() {
            let held = crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id));
            fds = if held.is_empty() {
                self.bind_sockets(&socks)?
            } else {
                held
            };
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
    pub(super) fn wait_for_sockets(&self) {
        use std::os::fd::AsRawFd;
        unsafe {
            let mut usr1: libc::sigset_t = std::mem::zeroed();
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut usr1);
            libc::sigaddset(&mut usr1, libc::SIGUSR1);
            libc::sigprocmask(libc::SIG_BLOCK, &usr1, &mut old);
            libc::signal(
                libc::SIGUSR1,
                wake_signal as *const () as libc::sighandler_t,
            );
            let mut during = old;
            libc::sigdelset(&mut during, libc::SIGUSR1);

            let store = self.cfg.run_dir.join("fdstore");
            let held = crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id));
            let mut fds: Vec<libc::pollfd> = held
                .iter()
                .map(|(_, f)| libc::pollfd {
                    fd: f.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            // Nothing held (the daemon is not reachable): start it now.
            while !fds.is_empty() && self.st.has(keys::SOCKET_WAIT) {
                let r = libc::ppoll(
                    fds.as_mut_ptr(),
                    fds.len() as libc::nfds_t,
                    std::ptr::null(),
                    &during,
                );
                if r > 0
                    || (r < 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR))
                {
                    break;
                }
            }
            // The daemon removed the key first: its SIGUSR1 may still be on
            // the way, and must not reach the service after exec.
            if !self.st.take(keys::SOCKET_WAIT) && !WOKEN.load(std::sync::atomic::Ordering::SeqCst)
            {
                let ts = libc::timespec {
                    tv_sec: 2,
                    tv_nsec: 0,
                };
                libc::sigtimedwait(&usr1, std::ptr::null_mut(), &ts);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
            libc::signal(libc::SIGUSR1, libc::SIG_DFL);
        }
    }

    pub(super) fn has_held_sockets(&self) -> bool {
        if self.socket_units().is_empty() {
            return false;
        }
        let store = self.cfg.run_dir.join("fdstore");
        !crate::fdstore::get(&store, &format!("{}#sockets", self.unit.id)).is_empty()
    }
}
