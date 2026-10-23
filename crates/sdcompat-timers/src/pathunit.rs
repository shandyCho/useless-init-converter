//! .path units. Each becomes an s6 longrun running `sdcompat-timers path-run`,
//! which watches with inotify and asks the daemon to start the unit.
//!
//! PathExists=, PathExistsGlob=, DirectoryNotEmpty= are levels: the unit is
//! started whenever the condition holds and the unit is not active.
//! PathChanged= / PathModified= are edges: started on each matching event.

use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;
use crate::loader::Loader;
use crate::state::{self, keys};
use crate::util;
pub use sdcompat_base::pathspec::*;

fn glob_any(pattern: &str) -> bool {
    let Ok(c) = CString::new(pattern) else { return false };
    unsafe {
        let mut g: libc::glob_t = std::mem::zeroed();
        let r = libc::glob(c.as_ptr(), 0, None, &mut g);
        let found = r == 0 && g.gl_pathc > 0;
        libc::globfree(&mut g);
        found
    }
}

fn level_holds(kind: &str, p: &str) -> bool {
    match kind {
        "PathExists" => Path::new(p).exists(),
        "PathExistsGlob" => glob_any(p),
        "DirectoryNotEmpty" => fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false),
        _ => false,
    }
}

/// The nearest existing ancestor (inotify needs something to watch).
fn watch_target(p: &str) -> PathBuf {
    let mut cur = PathBuf::from(p);
    if p.contains(['*', '?', '[']) {
        cur = cur.parent().map(Path::to_path_buf).unwrap_or_else(|| "/".into());
    }
    while !cur.exists() {
        match cur.parent() {
            Some(par) => cur = par.to_path_buf(),
            None => return "/".into(),
        }
    }
    cur
}

struct Inotify {
    fd: i32,
}

impl Inotify {
    fn new() -> Option<Inotify> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        (fd >= 0).then_some(Inotify { fd })
    }

    fn watch(&self, p: &Path) -> i32 {
        let Ok(c) = CString::new(p.as_os_str().as_encoded_bytes()) else { return -1 };
        let mask = libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM
            | libc::IN_CLOSE_WRITE | libc::IN_MODIFY | libc::IN_ATTRIB | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), mask) }
    }

    fn unwatch(&self, wd: i32) {
        unsafe { libc::inotify_rm_watch(self.fd, wd) };
    }

    /// Wait up to `ms` for events; returns (any event, any write-type event, any modify event).
    fn wait(&self, ms: i32) -> (bool, bool, bool) {
        let mut p = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, ms) } <= 0 {
            return (false, false, false);
        }
        let mut buf = [0u8; 8192];
        let (mut any, mut changed, mut modified) = (false, false, false);
        loop {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            let mut off = 0usize;
            while off + std::mem::size_of::<libc::inotify_event>() <= n as usize {
                // The byte buffer is not aligned for inotify_event.
                let ev: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off) as *const libc::inotify_event) };
                any = true;
                let m = ev.mask;
                if m & (libc::IN_CLOSE_WRITE | libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_TO
                    | libc::IN_MOVED_FROM | libc::IN_ATTRIB | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                {
                    changed = true;
                }
                if m & libc::IN_MODIFY != 0 {
                    modified = true;
                }
                off += std::mem::size_of::<libc::inotify_event>() + ev.len as usize;
            }
        }
        (any, changed, modified)
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

async fn unit_active(conn: &zbus::Connection, unit: &str) -> bool {
    let Ok(reply) = crate::client::manager_call(conn, "LoadUnit", &(unit,)).await else { return false };
    let Ok((path,)) = reply.body().deserialize::<(zbus::zvariant::OwnedObjectPath,)>() else { return false };
    let r = conn
        .call_method(
            Some("org.freedesktop.systemd1"),
            path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.systemd1.Unit", "ActiveState"),
        )
        .await;
    let Ok(r) = r else { return false };
    let Ok((v,)) = r.body().deserialize::<(zbus::zvariant::OwnedValue,)>() else { return false };
    let s: String = <&str>::try_from(&*v).map(|s| s.to_string()).unwrap_or_default();
    matches!(s.as_str(), "active" | "activating" | "reloading" | "deactivating")
}

pub fn cmd_path_run(cfg: Config, name: &str) -> i32 {
    let unit = Loader::new(&cfg).load(name);
    if !unit.is_loaded() {
        eprintln!("sdcompat: path unit {name}: {}", unit.load_state.as_str());
        return 1;
    }
    let target = trigger_unit(&unit);
    let specs = specs(&unit);
    if unit.get_bool("Path", "MakeDirectory", false) {
        let mode = unit
            .get("Path", "DirectoryMode")
            .and_then(|m| u32::from_str_radix(&m, 8).ok())
            .unwrap_or(0o755);
        for (_, p) in &specs {
            if !p.contains(['*', '?', '[']) && fs::create_dir_all(p).is_ok() {
                if let Ok(c) = CString::new(p.as_str()) {
                    unsafe { libc::chmod(c.as_ptr(), mode) };
                }
            }
        }
    }
    let st = state::for_unit(&cfg.run_dir, &unit.id);
    st.ensure();
    st.stamp(keys::ACTIVE_ENTER);
    unsafe {
        if libc::fcntl(3, libc::F_GETFD) >= 0 {
            libc::write(3, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(3);
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("sdcompat: {e}");
            return 1;
        }
    };
    rt.block_on(async move {
        let mut conn: Option<zbus::Connection> = None;
        // TriggerLimitBurst within TriggerLimitIntervalSec (defaults 200 / 2 s).
        let burst: usize = unit.get("Path", "TriggerLimitBurst").and_then(|v| v.parse().ok()).unwrap_or(200);
        let mut recent: Vec<u64> = Vec::new();
        // One inotify instance for the whole run: a new one each round would
        // lose the events between closing the old and watching with the new.
        let Some(ino) = Inotify::new().map(std::sync::Arc::new) else {
            eprintln!("sdcompat[{name}]: inotify unavailable");
            return 1;
        };
        let mut wds: Vec<i32> = Vec::new();
        loop {
            // Watched paths may have appeared, vanished or been re-created:
            // watch what is there now (adding an existing watch returns its
            // descriptor), and drop the rest, such as the ancestor watched
            // while the path did not exist yet.
            let mut now: Vec<i32> = Vec::new();
            for (_, p) in &specs {
                now.push(ino.watch(&watch_target(p)));
                if Path::new(p).is_dir() {
                    now.push(ino.watch(Path::new(p)));
                }
            }
            for wd in wds.iter().filter(|w| !now.contains(w)) {
                ino.unwatch(*wd);
            }
            wds = now;
            if conn.is_none() {
                conn = crate::client::connect(&cfg).await.ok();
            }
            let Some(c) = conn.clone() else {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            };
            let level = specs.iter().any(|(k, p)| level_holds(k, p));
            let mut fire = level && !unit_active(&c, &target).await;
            if !fire {
                // On the blocking pool, so the connection keeps reading what
                // the daemon sends (its signals) meanwhile; unread, they fill
                // the socket and the daemon drops us.
                let ino = ino.clone();
                let Ok((_, changed, modified)) = tokio::task::spawn_blocking(move || ino.wait(2000)).await else {
                    return 1;
                };
                let edge = specs.iter().any(|(k, _)| {
                    (k == "PathChanged" && changed) || (k == "PathModified" && (changed || modified))
                });
                fire = edge;
            }
            if fire {
                let now = util::now_monotonic_usec();
                recent.retain(|&t| now - t < 2_000_000);
                recent.push(now);
                if recent.len() > burst {
                    eprintln!("sdcompat[{name}]: trigger limit hit, waiting");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                match crate::client::manager_call(&c, "StartUnit", &(target.as_str(), "replace")).await {
                    Ok(_) => {
                        st.set(crate::timer::LAST_TRIGGER, &util::now_realtime_usec().to_string());
                        // Let the unit become active before re-checking levels.
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    Err(e) => {
                        eprintln!("sdcompat[{name}]: starting {target}: {e}");
                        if !e.contains("org.freedesktop") {
                            conn = None;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    })
}
