//! .path units. Each becomes an s6 longrun running `initconvert-timers path-run`,
//! which watches with inotify and asks the daemon to start the unit.
//!
//! PathExists=, PathExistsGlob=, DirectoryNotEmpty= are levels: the unit is
//! started whenever the condition holds and the unit is not active.
//! PathChanged= / PathModified= are edges: started on each matching event.

use std::ffi::CString;
use std::fs;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

use crate::config::Config;

use crate::state::{self, keys};
use crate::util;
pub use uinitconverter_base::pathspec::*;

fn glob_any(pattern: &str) -> bool {
    let Ok(c) = CString::new(pattern) else {
        return false;
    };
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
        "DirectoryNotEmpty" => fs::read_dir(p)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        _ => false,
    }
}

/// The nearest existing ancestor (inotify needs something to watch).
fn watch_target(p: &str) -> PathBuf {
    let mut cur = PathBuf::from(p);
    if p.contains(['*', '?', '[']) {
        cur = cur
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| "/".into());
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
    fd: OwnedFd,
}

impl Inotify {
    fn new() -> Option<Inotify> {
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).ok()?;
        Some(Inotify { fd })
    }

    fn watch(&self, p: &Path) -> Option<i32> {
        let mask = WatchFlags::CREATE
            | WatchFlags::DELETE
            | WatchFlags::MOVED_TO
            | WatchFlags::MOVED_FROM
            | WatchFlags::CLOSE_WRITE
            | WatchFlags::MODIFY
            | WatchFlags::ATTRIB
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF;
        inotify::add_watch(&self.fd, p, mask).ok()
    }

    fn unwatch(&self, wd: i32) {
        let _ = inotify::remove_watch(&self.fd, wd);
    }

    /// Wait up to `ms` for events; returns (any event, any write-type event, any modify event).
    fn wait(&self, ms: i64) -> (bool, bool, bool) {
        let mut p = [PollFd::new(&self.fd, PollFlags::IN)];
        let timeout = Timespec {
            tv_sec: ms / 1000,
            tv_nsec: ms % 1000 * 1_000_000,
        };
        if !matches!(poll(&mut p, Some(&timeout)), Ok(n) if n > 0) {
            return (false, false, false);
        }
        let write_type = ReadFlags::CLOSE_WRITE
            | ReadFlags::CREATE
            | ReadFlags::DELETE
            | ReadFlags::MOVED_TO
            | ReadFlags::MOVED_FROM
            | ReadFlags::ATTRIB
            | ReadFlags::DELETE_SELF
            | ReadFlags::MOVE_SELF;
        let mut buf = [MaybeUninit::uninit(); 8192];
        let mut events = inotify::Reader::new(&self.fd, &mut buf);
        let (mut any, mut changed, mut modified) = (false, false, false);
        // Until the non-blocking read has nothing left.
        while let Ok(ev) = events.next() {
            any = true;
            changed |= ev.events().intersects(write_type);
            modified |= ev.events().contains(ReadFlags::MODIFY);
        }
        (any, changed, modified)
    }
}

async fn unit_active(conn: &zbus::Connection, unit: &str) -> bool {
    let Ok(reply) = crate::client::manager_call(conn, "LoadUnit", &(unit,)).await else {
        return false;
    };
    let Ok((path,)) = reply
        .body()
        .deserialize::<(zbus::zvariant::OwnedObjectPath,)>()
    else {
        return false;
    };
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
    let Ok((v,)) = r.body().deserialize::<(zbus::zvariant::OwnedValue,)>() else {
        return false;
    };
    let s: String = <&str>::try_from(&*v)
        .map(|s| s.to_string())
        .unwrap_or_default();
    matches!(
        s.as_str(),
        "active" | "activating" | "reloading" | "deactivating"
    )
}

pub fn cmd_path_run(cfg: Config, name: &str) -> i32 {
    let unit = uinitconverter_base::snapshot::Units::new(&cfg)
        .get(name)
        .unit;
    if !unit.is_loaded() {
        eprintln!(
            "uinitconverter: path unit {name}: {}",
            unit.load_state.as_str()
        );
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
                let _ = rustix::fs::chmod(p.as_str(), rustix::fs::Mode::from_raw_mode(mode));
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

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("uinitconverter: {e}");
            return 1;
        }
    };
    rt.block_on(async move {
        let mut conn: Option<zbus::Connection> = None;
        // TriggerLimitBurst within TriggerLimitIntervalSec (defaults 200 / 2 s).
        let burst: usize = unit
            .get("Path", "TriggerLimitBurst")
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let mut recent: Vec<u64> = Vec::new();
        // One inotify instance for the whole run: a new one each round would
        // lose the events between closing the old and watching with the new.
        let Some(ino) = Inotify::new().map(std::sync::Arc::new) else {
            eprintln!("uinitconverter[{name}]: inotify unavailable");
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
                now.extend(ino.watch(&watch_target(p)));
                if Path::new(p).is_dir() {
                    now.extend(ino.watch(Path::new(p)));
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
                let Ok((_, changed, modified)) =
                    tokio::task::spawn_blocking(move || ino.wait(2000)).await
                else {
                    return 1;
                };
                let edge = specs.iter().any(|(k, _)| {
                    (k == "PathChanged" && changed)
                        || (k == "PathModified" && (changed || modified))
                });
                fire = edge;
            }
            if fire {
                let now = util::now_monotonic_usec();
                recent.retain(|&t| now - t < 2_000_000);
                recent.push(now);
                if recent.len() > burst {
                    eprintln!("uinitconverter[{name}]: trigger limit hit, waiting");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                match crate::client::manager_call(&c, "StartUnit", &(target.as_str(), "replace"))
                    .await
                {
                    Ok(_) => {
                        st.set(
                            crate::timer::LAST_TRIGGER,
                            &util::now_realtime_usec().to_string(),
                        );
                        // Let the unit become active before re-checking levels.
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    Err(e) => {
                        eprintln!("uinitconverter[{name}]: starting {target}: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "uinitconverter-inotify-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn inotify_sorts_the_events() {
        let d = temp_dir("sort");
        let ino = Inotify::new().unwrap();
        assert!(ino.watch(&d).is_some());
        // Nothing yet: the wait times out.
        assert_eq!(ino.wait(0), (false, false, false));

        let f = d.join("f");
        fs::write(&f, "1").unwrap();
        let (any, changed, _) = ino.wait(2000);
        assert!(any && changed, "a new file is a change");

        // Written and left open: IN_MODIFY without IN_CLOSE_WRITE.
        use std::io::Write;
        let mut open = fs::OpenOptions::new().append(true).open(&f).unwrap();
        ino.wait(0);
        open.write_all(b"2").unwrap();
        assert_eq!(ino.wait(2000), (true, false, true));
        drop(open);
        assert_eq!(ino.wait(2000), (true, true, false), "closed after writing");

        fs::remove_file(&f).unwrap();
        let (any, changed, _) = ino.wait(2000);
        assert!(any && changed, "a removed file is a change");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn inotify_drains_more_than_one_buffer() {
        let d = temp_dir("drain");
        let ino = Inotify::new().unwrap();
        assert!(ino.watch(&d).is_some());
        // Long names, so the events take several reads of the 8 KiB buffer.
        for i in 0..200 {
            fs::write(d.join(format!("{i:0>200}")), "").unwrap();
        }
        assert!(ino.wait(2000).0);
        assert_eq!(ino.wait(0), (false, false, false), "all read at once");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn inotify_watches_can_go_and_be_refused() {
        let d = temp_dir("watch");
        let ino = Inotify::new().unwrap();
        let wd = ino.watch(&d).unwrap();
        assert_eq!(
            ino.watch(&d),
            Some(wd),
            "the same path keeps its descriptor"
        );
        assert!(ino.watch(&d.join("missing")).is_none());
        assert!(ino.watch(Path::new("bad\0path")).is_none());
        ino.unwatch(wd);
        // Removing a watch queues IN_IGNORED: not a change.
        assert_eq!(ino.wait(2000), (true, false, false));
        fs::write(d.join("f"), "").unwrap();
        assert_eq!(ino.wait(0), (false, false, false));
        let _ = fs::remove_dir_all(&d);
    }
}
