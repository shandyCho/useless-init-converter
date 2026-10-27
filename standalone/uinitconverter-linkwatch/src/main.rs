//! uinitconverter-linkwatch: unblocks s6-rc-update and s6-svlink when they wait
//! for a supervisor's start event that came before they listened.
//!
//! s6_supervise_link_names() (s6 2.15) first notes which service
//! directories already have a supervisor, then subscribes to the others'
//! event/ fifodirs, links them and waits for each one's "s". s6-svscan may
//! start a supervisor in between (respawning one that s6-rc-update just
//! stopped, through the link it just made): its "s" goes to nobody, and the
//! wait never ends (s6-rc-update has no timeout unless given -t), holding
//! s6-rc's lock. When such a wait has lasted --after seconds, this writes
//! the missed "s" into each fifo the waiting s6-ftrigrd holds, for the
//! directories whose supervisor runs (holds supervise/lock). Nothing else is
//! touched: a fifo whose event already came gets a duplicate, which the
//! waiter has stopped looking at.
//!
//! Standalone: reads no configuration. Polls /proc.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

const USAGE: &str = "\
usage: uinitconverter-linkwatch [--after SECONDS] [--interval SECONDS] [--under DIR] [--once]

  --after SECONDS     how long a wait lasts before it is unblocked (default 10)
  --interval SECONDS  how often to look (default 2)
  --under DIR         only service directories below DIR (a test's own s6 tree)
  --once              look once and exit
";

/// The programs whose s6-ftrigrd waits in s6_supervise_link_names().
const WAITERS: [&str; 2] = ["s6-rc-update", "s6-svlink"];

fn die(msg: &str) -> ! {
    eprintln!("uinitconverter-linkwatch: {msg}");
    exit(1)
}

/// The parent PID and start time (clock ticks after boot) in a
/// /proc/PID/stat line.
fn parse_stat(s: &str) -> Option<(u32, u64)> {
    // comm (field 2) may hold spaces and parentheses: fields after the last ')'.
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some((f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?))
}

fn comm(proc_dir: &Path, pid: u32) -> Option<String> {
    fs::read_to_string(proc_dir.join(pid.to_string()).join("comm")).ok().map(|s| s.trim_end().to_string())
}

/// Whether another process holds a lock on `path`, as skalibs'
/// fd_islocked() asks (s6-supervise's supervise/lock, fcntl locks).
fn lock_held(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = fs::File::open(path) else { return false };
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_RDLCK as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETLK, &mut fl) };
    r == 0 && fl.l_type != libc::F_UNLCK as libc::c_short
}

/// The service directory of an s6-ftrigrd fifo: DIR/event/ftrig1:...
fn servicedir_of(fifo: &Path) -> Option<&Path> {
    let name = fifo.file_name()?.to_str()?;
    let event = fifo.parent()?;
    (name.starts_with("ftrig1:") && event.file_name()? == "event").then(|| event.parent())?
}

struct Watch {
    proc_dir: PathBuf,
    after: f64,
    hz: f64,
    under: Option<PathBuf>,
    /// (s6-ftrigrd PID, fifo) already sent to.
    sent: HashSet<(u32, PathBuf)>,
}

impl Watch {
    /// Look once; returns the fifos written to.
    fn scan(&mut self) -> Vec<PathBuf> {
        let mut done = Vec::new();
        let Some(uptime) = fs::read_to_string(self.proc_dir.join("uptime"))
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        else {
            return done;
        };
        let mut alive = HashSet::new();
        let Ok(dir) = fs::read_dir(&self.proc_dir) else { return done };
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if comm(&self.proc_dir, pid).as_deref() != Some("s6-ftrigrd") {
                continue;
            }
            let Some((ppid, start)) =
                fs::read_to_string(e.path().join("stat")).ok().as_deref().and_then(parse_stat)
            else {
                continue;
            };
            let Some(parent) = comm(&self.proc_dir, ppid).filter(|c| WAITERS.contains(&c.as_str())) else {
                continue;
            };
            alive.insert(pid);
            let waited = uptime - start as f64 / self.hz;
            if waited < self.after {
                continue;
            }
            // Each fifo is open twice (read end and s6-ftrigrd's own writer).
            let mut fifos: Vec<PathBuf> = fs::read_dir(e.path().join("fd"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|fd| fs::read_link(fd.path()).ok())
                .filter(|p| !p.to_string_lossy().ends_with(" (deleted)") && servicedir_of(p).is_some())
                .filter(|p| self.under.as_ref().is_none_or(|u| p.starts_with(u)))
                .collect();
            fifos.sort();
            fifos.dedup();
            for fifo in fifos {
                let svc = servicedir_of(&fifo).unwrap();
                if self.sent.contains(&(pid, fifo.clone())) || !lock_held(&svc.join("supervise/lock")) {
                    continue;
                }
                self.sent.insert((pid, fifo.clone()));
                let res = fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo)
                    .and_then(|mut f| f.write_all(b"s"));
                match res {
                    Ok(()) => {
                        eprintln!(
                            "uinitconverter-linkwatch: {parent} (pid {ppid}) waited {waited:.0} s for {}, whose supervisor runs: sent its start event",
                            svc.display()
                        );
                        done.push(fifo);
                    }
                    Err(err) => eprintln!("uinitconverter-linkwatch: {}: {err}", fifo.display()),
                }
            }
        }
        self.sent.retain(|(pid, _)| alive.contains(pid));
        done
    }
}

fn main() {
    let mut after = 10.0;
    let mut interval = 2.0;
    let mut once = false;
    let mut under = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut secs = || {
            it.next()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v > 0.0)
                .unwrap_or_else(|| die(&format!("{a} needs a number of seconds")))
        };
        match a.as_str() {
            "--after" => after = secs(),
            "--interval" => interval = secs(),
            "--under" => {
                let d = it.next().unwrap_or_else(|| die("--under needs a directory"));
                under = Some(fs::canonicalize(&d).unwrap_or_else(|e| die(&format!("{d}: {e}"))));
            }
            "--once" => once = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                exit(0);
            }
            o => die(&format!("unknown option {o}")),
        }
    }
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let mut w = Watch { proc_dir: PathBuf::from("/proc"), after, hz, under, sent: HashSet::new() };
    loop {
        w.scan();
        if once {
            break;
        }
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read};
    use std::process::{Command, Stdio};

    #[test]
    fn stat_fields() {
        let s = "77 (s6 (x) y) S 42 77 77 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 12345 0 0";
        assert_eq!(parse_stat(s), Some((42, 12345)));
        assert_eq!(parse_stat("1 (init) S"), None);
    }

    #[test]
    fn fifo_paths() {
        assert_eq!(servicedir_of(Path::new("/l/sd/a.service/event/ftrig1:@4000:xY")), Some(Path::new("/l/sd/a.service")));
        assert_eq!(servicedir_of(Path::new("/l/sd/a.service/event/.ftrig1:@4000:xY")), None);
        assert_eq!(servicedir_of(Path::new("/l/sd/a.service/other/ftrig1:@4000:xY")), None);
    }

    /// A fake /proc: an s6-ftrigrd of an s6-rc-update, waiting long enough,
    /// with fifos in a supervised and an unsupervised service directory.
    #[test]
    fn sends_once_to_supervised_only() {
        let tmp = std::env::temp_dir().join(format!("uinitconverter-linkwatch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let proc_dir = tmp.join("proc");
        let fifo = |svc: &str| {
            let d = tmp.join(svc).join("event");
            fs::create_dir_all(&d).unwrap();
            fs::create_dir_all(tmp.join(svc).join("supervise")).unwrap();
            fs::write(tmp.join(svc).join("supervise/lock"), "").unwrap();
            let p = d.join("ftrig1:@40000000:abc");
            let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            p
        };
        let (up, down) = (fifo("up.service"), fifo("down.service"));
        let proc_entry = |pid: u32, comm: &str, ppid: u32, start: u64| {
            let d = proc_dir.join(pid.to_string());
            fs::create_dir_all(d.join("fd")).unwrap();
            fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
            fs::write(d.join("stat"), format!("{pid} ({comm}) S {ppid} 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0")).unwrap();
            d
        };
        proc_entry(50, "s6-rc-update", 1, 0);
        let d = proc_entry(100, "s6-ftrigrd", 50, 100);
        for (i, f) in [&up, &up, &down].iter().enumerate() {
            std::os::unix::fs::symlink(f, d.join("fd").join((3 + i).to_string())).unwrap();
        }
        // Another program's s6-ftrigrd (s6-rc waiting for a service) is left alone.
        proc_entry(60, "s6-rc", 1, 0);
        let d2 = proc_entry(101, "s6-ftrigrd", 60, 100);
        std::os::unix::fs::symlink(&up, d2.join("fd/3")).unwrap();
        fs::write(proc_dir.join("uptime"), "20.5 1.0\n").unwrap();

        // The supervisor's lock, held by another process.
        let mut child = Command::new("python3")
            .args(["-c", "import fcntl,sys; f=open(sys.argv[1],'w'); fcntl.lockf(f, fcntl.LOCK_EX); print('locked', flush=True); sys.stdin.read()"])
            .arg(tmp.join("up.service/supervise/lock"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "locked");
        // The waiter's read end.
        let mut reader = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&up).unwrap();

        // Started at 100 ticks = 1 s, so waited 19.5 s: not yet with --after 30.
        let mut w = Watch { proc_dir: proc_dir.clone(), after: 30.0, hz: 100.0, under: None, sent: HashSet::new() };
        assert!(w.scan().is_empty());
        w.after = 10.0;
        w.under = Some(tmp.join("elsewhere"));
        assert!(w.scan().is_empty(), "not below --under");
        w.under = Some(tmp.clone());
        assert_eq!(w.scan(), vec![up.clone()]);
        let mut got = Vec::new();
        let _ = reader.read_to_end(&mut got);
        assert_eq!(got, b"s");
        assert!(w.scan().is_empty(), "sent once only");

        drop(child.stdin.take());
        let _ = child.wait();
        let _ = fs::remove_dir_all(&tmp);
    }
}
