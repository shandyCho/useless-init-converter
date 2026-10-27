//! systemd's SELinux access checks on D-Bus callers (src/core/selinux-access.c,
//! with the permission each method asks for in dbus-manager.c, dbus-unit.c,
//! dbus-job.c and dbus.c's mac_selinux_filter; systemd 262). They come before
//! polkit and apply to root too: a confined root daemon may only do what the
//! policy grants its domain, as under systemd.
//!
//! The target is the unit file's label in class "service" (systemd reads it
//! when it loads the fragment; none for masked, transient and not-found
//! units), otherwise our own label in class "system". Only as root with
//! SELinux enabled (libselinux loaded at run time); libselinux decides
//! permissive mode and permissive domains, and logs denials to stderr.

use zbus::Message;

use super::{err, Daemon, Peer, E_ACCESS, R};
use crate::loader::{LoadState, Unit};
use uinitconverter_exec::selinux;

/// systemd's job_type_to_access_method(). ReloadOrRestart and
/// ReloadOrTryRestart are restart jobs that may reload, so "start".
pub(super) fn job_access(jtype: &str) -> &'static str {
    match jtype {
        "start" | "restart" | "try-restart" | "reload-or-restart" | "reload-or-try-restart" => "start",
        "stop" => "stop",
        _ => "reload",
    }
}

/// sd-bus's object_path_startswith(): PATH is PREFIX or below it.
pub(super) fn path_below(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

/// The unit file's label, as systemd's unit->access_selinux_context.
fn unit_context(unit: &Unit) -> Option<String> {
    if !matches!(unit.load_state, LoadState::Loaded | LoadState::BadSetting) {
        return None;
    }
    selinux::file_context(&unit.fragment.as_ref()?.display().to_string())
}

/// The SO_PEERSEC label of a connected socket (private socket callers).
pub(super) fn peer_label(fd: std::os::fd::RawFd) -> Option<String> {
    let mut buf = vec![0u8; 256];
    loop {
        let mut len = buf.len() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERSEC, buf.as_mut_ptr().cast(), &mut len) };
        if r == 0 {
            buf.truncate(len as usize);
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) || buf.len() >= 65536 {
            return None;
        }
        buf.resize(len.max(buf.len() as libc::socklen_t * 2) as usize, 0);
    }
    let s = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
    (!s.is_empty()).then_some(s)
}

impl Daemon {
    /// The caller's label: SO_PEERSEC on the private socket, the bus's
    /// LinuxSecurityLabel (GetConnectionCredentials) otherwise. As systemd,
    /// never read from /proc, which would race with an exec.
    async fn caller_label(&self, c: &Peer, msg: &Message) -> Option<String> {
        if c.uid.is_some() {
            return c.label.clone();
        }
        let hdr = msg.header();
        let sender = hdr.sender()?.to_string();
        drop(hdr);
        if let Some(l) = self.label_cache.lock().unwrap().get(&sender) {
            return Some(l.clone());
        }
        let proxy = zbus::fdo::DBusProxy::new(&self.conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender.clone()).ok()?;
        let creds = proxy.get_connection_credentials(name).await.ok()?;
        let label = creds.linux_security_label()?;
        let label = String::from_utf8_lossy(label).trim_end_matches('\0').to_string();
        if label.is_empty() {
            return None;
        }
        let mut cache = self.label_cache.lock().unwrap();
        // Unique names are never reused, but they pile up: keep it bounded.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(sender, label.clone());
        Some(label)
    }

    /// mac_selinux_unit_access_check() for UNIT, or mac_selinux_access_check()
    /// (the manager) without one.
    pub(super) async fn mac_check(&self, c: &Peer, msg: &Message, unit: Option<&Unit>, perm: &str) -> R<()> {
        if !selinux::access_checks() {
            return Ok(());
        }
        let enforce = selinux::enforcing();
        let (tcon, tclass) = match unit.and_then(unit_context) {
            Some(l) => (l, "service"),
            None => match selinux::own_context() {
                Some(o) => (o, "system"),
                None if enforce => return Err(err(E_ACCESS, "We appear not to have any SELinux context")),
                None => return Ok(()),
            },
        };
        let Some(scon) = self.caller_label(c, msg).await else {
            if !enforce {
                return Ok(());
            }
            return Err(err(E_ACCESS, "SELinux: the caller's context is unknown"));
        };
        match selinux::check_access(&scon, &tcon, tclass, perm) {
            Ok(()) => Ok(()),
            Err(e) => {
                let hdr = msg.header();
                let member = hdr.member().map(|m| m.as_str().to_string()).unwrap_or_default();
                let path = unit.and_then(|u| u.fragment.as_ref()).map(|p| p.display().to_string());
                eprintln!(
                    "uinitconverterd: SELinux access check scon={scon} tcon={tcon} tclass={tclass} perm={perm} state={} function={member} path={}: {e}",
                    if enforce { "enforcing" } else { "permissive" },
                    path.as_deref().unwrap_or("n/a"),
                );
                if enforce {
                    Err(err(E_ACCESS, format!("SELinux policy denies access: {e}")))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// mac_check() on the unit NAME names (aliases resolved).
    pub(super) async fn mac_unit(&self, c: &Peer, msg: &Message, name: &str, perm: &str) -> R<()> {
        if !selinux::access_checks() {
            return Ok(());
        }
        let (_, unit) = self.load_unit(name);
        self.mac_check(c, msg, Some(&unit), perm).await
    }

    /// ListUnits*: as systemd, units the caller may not see ("status") are
    /// left out silently.
    pub(super) async fn visible_units(&self, c: &Peer, msg: &Message, v: Vec<super::UnitEntry>) -> Vec<super::UnitEntry> {
        if !selinux::access_checks() {
            return v;
        }
        let mut out = Vec::with_capacity(v.len());
        for e in v {
            if self.mac_unit_quiet(c, msg, &e.0).await {
                out.push(e);
            }
        }
        out
    }

    /// mac_unit(NAME, "status") without the denial line per unit.
    async fn mac_unit_quiet(&self, c: &Peer, msg: &Message, name: &str) -> bool {
        let (_, unit) = self.load_unit(name);
        let target = unit_context(&unit).map(|l| (l, "service")).or_else(|| selinux::own_context().map(|o| (o, "system")));
        let (Some((tcon, tclass)), Some(scon)) = (target, self.caller_label(c, msg).await) else {
            return !selinux::enforcing();
        };
        selinux::check_access(&scon, &tcon, tclass, "status").is_ok() || !selinux::enforcing()
    }

    /// The unit of job ID, for the job methods' unit checks.
    pub(super) fn job_unit(&self, id: u32) -> Option<String> {
        self.jobs.lock().unwrap().get(&id).map(|j| j.unit.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_types_map_as_systemd() {
        for (j, p) in [
            ("start", "start"),
            ("restart", "start"),
            ("try-restart", "start"),
            ("reload-or-restart", "start"),
            ("reload-or-try-restart", "start"),
            ("stop", "stop"),
            ("reload", "reload"),
        ] {
            assert_eq!(job_access(j), p, "{j}");
        }
    }

    #[test]
    fn object_path_prefix() {
        assert!(path_below("/org/freedesktop/systemd1", "/org/freedesktop/systemd1"));
        assert!(path_below("/org/freedesktop/systemd1/unit/a_2eservice", "/org/freedesktop/systemd1"));
        assert!(!path_below("/org/freedesktop/systemd10", "/org/freedesktop/systemd1"));
        assert!(!path_below("/org/freedesktop", "/org/freedesktop/systemd1"));
    }

    #[test]
    fn peer_label_of_a_socketpair() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) }, 0);
        let label = peer_label(fds[0]);
        let own = std::fs::read_to_string("/proc/self/attr/current").ok();
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        // Without SELinux SO_PEERSEC fails (ENOPROTOOPT).
        if std::path::Path::new("/sys/fs/selinux/enforce").exists() {
            assert_eq!(label.as_deref(), own.as_deref().map(|s| s.trim_end_matches(['\0', '\n'])));
        }
    }
}
