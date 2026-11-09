//! Connection instances of Accept=yes sockets as units. The runner's
//! `connection` runs them in the foreground of s6-ipcserverd/s6-tcpserverd,
//! not as s6 services; each one is a file named after the instance in its
//! socket's `connections/` directory (holding the runner's pid) and its
//! state directory (MAIN_PID while ExecStart= runs). From those the daemon
//! lists them, reports their state and stops them.

use super::s6::Status;
use super::*;
use crate::acceptspec;

impl Daemon {
    /// Running connections: (instance, socket, runner pid).
    pub(super) fn connections(&self) -> Vec<(String, String, i32)> {
        // The database's sockets.
        let sockets: Vec<String> = self
            .index
            .read()
            .unwrap()
            .units
            .keys()
            .filter(|n| n.ends_with(".socket"))
            .cloned()
            .collect();
        let mut out = Vec::new();
        for sock in sockets {
            let dir = state::for_unit(&self.cfg.run_dir, &sock)
                .dir
                .join(acceptspec::CONNECTIONS_DIR);
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let pid = std::fs::read_to_string(e.path())
                    .ok()
                    .and_then(|p| p.trim().parse::<i32>().ok());
                // A runner killed outright leaves its file: not a connection.
                let Some(pid) = pid.filter(|&p| alive(p)) else {
                    continue;
                };
                out.push((
                    e.file_name().to_string_lossy().into_owned(),
                    sock.clone(),
                    pid,
                ));
            }
        }
        out
    }

    /// The socket and runner pid of a running connection instance.
    pub(super) fn connection(&self, id: &str) -> Option<(String, i32)> {
        if !id.contains('@') || !id.ends_with(".service") {
            return None;
        }
        self.connections()
            .into_iter()
            .find(|(i, _, _)| i == id)
            .map(|(_, s, p)| (s, p))
    }

    /// Where a connection is: before ExecStart= (activating), while it runs,
    /// or in ExecStop=/ExecStopPost= after it (deactivating).
    pub(super) fn connection_status(&self, id: &str, s: &mut Status) {
        let st = state::for_unit(&self.cfg.run_dir, id);
        let main = st.get(keys::MAIN_PID).and_then(|p| p.parse::<u32>().ok());
        (s.active, s.sub) = match main {
            Some(pid) => {
                s.main_pid = pid;
                ("active", "running")
            }
            None if st.has(keys::EXEC_MAIN_EXIT) => ("deactivating", "stop-post"),
            None => ("activating", "start"),
        };
    }

    /// `systemctl stop` of a connection: KillSignal= to its main process,
    /// SIGKILL after TimeoutStopSec=; the runner then runs ExecStopPost= and
    /// removes it. Before ExecStart= there's no main process: the runner's
    /// children (ExecStartPre=) get the signal instead.
    pub(super) async fn stop_connection(&self, id: &str, unit: &Unit) -> &'static str {
        let Some((_, runner)) = self.connection(id) else {
            return "done";
        };
        let st = state::for_unit(&self.cfg.run_dir, id);
        let sig = unit
            .get("Service", "KillSignal")
            .and_then(|s| util::signal_number(&s))
            .unwrap_or(libc::SIGTERM);
        let targets = || -> Vec<i32> {
            match st.get(keys::MAIN_PID).and_then(|p| p.parse::<i32>().ok()) {
                Some(p) => vec![p],
                None => children(runner),
            }
        };
        for p in targets() {
            unsafe { libc::kill(p, sig) };
        }
        let timeout = Duration::from_micros(unit.stop_timeout_usec().max(1_000_000));
        let start = std::time::Instant::now();
        let mut killed = false;
        while self.connection(id).is_some() {
            if !killed && start.elapsed() >= timeout {
                eprintln!("initconvertd: {id}: stop timed out, killing");
                for p in targets() {
                    unsafe { libc::kill(p, libc::SIGKILL) };
                }
                killed = true;
            }
            if start.elapsed() >= timeout + Duration::from_secs(10) {
                return "timeout";
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.emit(
            MANAGER_PATH,
            MANAGER_IFACE,
            "UnitRemoved",
            &(id.to_string(), op(unit_path(id))),
        )
        .await;
        "done"
    }
}

fn alive(pid: i32) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// Direct children of a process.
fn children(pid: i32) -> Vec<i32> {
    util::proc_children(pid as u32)
        .into_iter()
        .map(|p| p as i32)
        .collect()
}
