//! The console settings of getty@ and friends: TTYReset=, TTYVTDisallocate=
//! and the utmp/wtmp records of UtmpIdentifier=/UtmpMode=, as systemd's
//! exec_context_tty_reset() and utmp-wtmp.c do them. TTYVHangup= is done
//! where the tty is opened (stdio.rs). Callers: getty@, serial-getty@,
//! console-getty, container-getty@, kmsconvt@ (work_history part 255);
//! agetty runs there with --noreset --noclear because the service manager
//! does this.

use super::*;

const UTMP_PATH: &str = "/run/utmp";
const WTMP_PATH: &str = "/var/log/wtmp";

extern "C" {
    // glibc's; the libc crate doesn't declare it.
    fn updwtmpx(file: *const libc::c_char, ut: *const libc::utmpx);
}

impl Runner {
    fn tty_settings_path(&self) -> Option<String> {
        let wants = self.unit.get_bool("Service", "TTYReset", false)
            || self.unit.get_bool("Service", "TTYVHangup", false)
            || self.unit.get_bool("Service", "TTYVTDisallocate", false);
        (wants && self.unit.get("Service", "TTYPath").is_some()).then(|| self.tty_path())
    }

    /// Before the main command: reset the terminal, deallocate the VT, and
    /// write the utmp record of `pid` (the main process: the runner execs
    /// it). TTYVHangup= follows when stdio.rs opens the tty.
    pub(super) fn tty_start(&self, pid: u32) {
        if let Some(path) = self.tty_settings_path() {
            if self.unit.get_bool("Service", "TTYReset", false) {
                reset_terminal(&path);
            }
            if self.unit.get_bool("Service", "TTYVTDisallocate", false) {
                if let Err(e) = vt_disallocate(&path) {
                    log(self.id(), &format!("TTYVTDisallocate=: {e}"));
                }
            }
        }
        if let Some(id) = self.unit.get("Service", "UtmpIdentifier") {
            let mode = self
                .unit
                .get("Service", "UtmpMode")
                .unwrap_or_else(|| "init".into());
            let line = self.unit.get("Service", "TTYPath").map(|_| self.tty_path());
            // The main process's session: its own once stdio.rs's setsid()
            // ran for a tty, else the runner's.
            let sid = if self.uses_tty() {
                pid as i32
            } else {
                unsafe { libc::getsid(0) }
            };
            utmp_init(
                &id,
                pid as i32,
                sid,
                line.as_deref(),
                &mode,
                (UTMP_PATH, WTMP_PATH),
            );
        }
    }

    /// After the main process ended: its utmp/wtmp logout record, then the
    /// terminal is reset, hung up and deallocated again, so the next getty
    /// starts on a clean screen and nobody keeps the old one open (systemd's
    /// exec_context_revert_tty()).
    pub(super) fn tty_finish(&self, status: Status) {
        if let Some(id) = self.unit.get("Service", "UtmpIdentifier") {
            let pid = self
                .st
                .get(keys::MAIN_PID)
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let (code, val) = match status {
                Status::Exited(c) => (libc::CLD_EXITED, c),
                Status::Killed(s) => (libc::CLD_KILLED, s),
            };
            utmp_dead(&id, pid, code, val, (UTMP_PATH, WTMP_PATH));
        }
        let Some(path) = self.tty_settings_path() else {
            return;
        };
        if self.unit.get_bool("Service", "TTYReset", false) {
            reset_terminal(&path);
        }
        if self.unit.get_bool("Service", "TTYVHangup", false) {
            vhangup_path(&path);
        }
        if self.unit.get_bool("Service", "TTYVTDisallocate", false) {
            if let Err(e) = vt_disallocate(&path) {
                log(self.id(), &format!("TTYVTDisallocate=: {e}"));
            }
        }
    }
}

fn open_tty_nonblock(path: &str, flags: i32) -> Option<i32> {
    let c = cstr(path);
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            flags | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    (fd >= 0).then_some(fd)
}

// linux/kd.h and linux/vt.h; the libc crate has neither.
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_TEXT: libc::c_ulong = 0;
const KDSKBMODE: libc::c_ulong = 0x4B45;
const K_UNICODE: libc::c_ulong = 3;
const VT_DISALLOCATE: libc::c_ulong = 0x5608;

/// systemd's terminal reset: no exclusive mode, a VT back in text mode with
/// a Unicode keyboard, sane line settings, pending input and output dropped.
fn reset_terminal(path: &str) {
    let Some(fd) = open_tty_nonblock(path, libc::O_RDWR) else {
        return;
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCNXCL);
        libc::ioctl(fd, KDSKBMODE, K_UNICODE);
        libc::ioctl(fd, KDSETMODE, KD_TEXT);
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) == 0 {
            sane_termios(&mut t);
            libc::tcsetattr(fd, libc::TCSANOW, &t);
        }
        libc::tcflush(fd, libc::TCIOFLUSH);
        libc::close(fd);
    }
}

/// The line settings systemd's reset_terminal_fd() sets.
fn sane_termios(t: &mut libc::termios) {
    use libc::*;
    t.c_iflag &= !(IGNBRK | BRKINT | ISTRIP | INLCR | IGNCR | IUCLC);
    t.c_iflag |= ICRNL | IMAXBEL | IUTF8;
    t.c_oflag |= ONLCR | OPOST;
    t.c_cflag |= CREAD;
    t.c_lflag = ISIG | ICANON | IEXTEN | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE;
    t.c_cc[VINTR] = 0o3;
    t.c_cc[VQUIT] = 0o34;
    t.c_cc[VERASE] = 0o177;
    t.c_cc[VKILL] = 0o25;
    t.c_cc[VEOF] = 0o4;
    t.c_cc[VSTART] = 0o21;
    t.c_cc[VSTOP] = 0o23;
    t.c_cc[VSUSP] = 0o32;
    t.c_cc[VLNEXT] = 0o26;
    t.c_cc[VWERASE] = 0o27;
    t.c_cc[VREPRINT] = 0o22;
    t.c_cc[VEOL] = 0;
    t.c_cc[VEOL2] = 0;
    t.c_cc[VTIME] = 0;
    t.c_cc[VMIN] = 1;
}

/// Hang up everyone who still has the terminal open (a shell left in the
/// background), as TTYVHangup= at the start does in stdio.rs. In a child,
/// since vhangup() needs the tty as the controlling terminal.
fn vhangup_path(path: &str) {
    let c = cstr(path);
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            libc::setsid();
            let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if fd >= 0 {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                libc::ioctl(fd, libc::TIOCSCTTY, 1);
                libc::vhangup();
            }
            libc::_exit(0);
        }
        if pid > 0 {
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
        }
    }
}

/// The VT number of /dev/ttyN (N >= 1), else None (a serial line, the
/// console, a pty).
fn vt_number(path: &str) -> Option<u32> {
    let n = path.strip_prefix("/dev/tty")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok().filter(|&n| n >= 1)
}

/// Free the VT (its scrollback goes with it); when it is in use (the
/// visible one) or not a VT, clear the screen and scrollback instead.
fn vt_disallocate(path: &str) -> Result<(), String> {
    if let Some(n) = vt_number(path) {
        let Some(fd) = open_tty_nonblock("/dev/tty0", libc::O_RDWR) else {
            return Err(format!(
                "open /dev/tty0: {}",
                std::io::Error::last_os_error()
            ));
        };
        let r = unsafe { libc::ioctl(fd, VT_DISALLOCATE, n as libc::c_ulong) };
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        if r >= 0 {
            return Ok(());
        }
        if e.raw_os_error() != Some(libc::EBUSY) {
            return Err(format!("VT_DISALLOCATE {n}: {e}"));
        }
    }
    // Opening a VT allocates it, so this is only for one in use.
    if let Some(fd) = open_tty_nonblock(path, libc::O_WRONLY) {
        // Scrolling region, home, screen with scrollback, full reset.
        let seq = b"\x1b[r\x1b[H\x1b[3J\x1bc";
        unsafe {
            libc::write(fd, seq.as_ptr() as *const libc::c_void, seq.len());
            libc::close(fd);
        }
    }
    Err(format!("{path} is in use: its screen was cleared instead"))
}

fn copy_field(dst: &mut [libc::c_char], s: &[u8]) {
    for (d, b) in dst.iter_mut().zip(s.iter().chain(std::iter::repeat(&0))) {
        *d = *b as libc::c_char;
    }
}

/// ut_id: the identifier's last four bytes, as systemd.
fn ut_id(id: &str) -> [libc::c_char; 4] {
    let b = id.as_bytes();
    let tail = &b[b.len().saturating_sub(4)..];
    let mut out = [0 as libc::c_char; 4];
    copy_field(&mut out, tail);
    out
}

fn now_tv(ut: &mut libc::utmpx) {
    let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    ut.ut_tv.tv_sec = tv.tv_sec as _;
    ut.ut_tv.tv_usec = tv.tv_usec as _;
}

fn write_both(ut: &libc::utmpx, files: (&str, &str)) {
    let u = cstr(files.0);
    let w = cstr(files.1);
    unsafe {
        libc::utmpxname(u.as_ptr());
        libc::setutxent();
        libc::pututxline(ut);
        libc::endutxent();
        updwtmpx(w.as_ptr(), ut);
    }
}

/// systemd's utmp_put_init_process(): INIT_PROCESS, and for UtmpMode=login
/// or user a LOGIN_PROCESS record (user: then USER_PROCESS "LOGIN").
fn utmp_init(id: &str, pid: i32, sid: i32, line: Option<&str>, mode: &str, files: (&str, &str)) {
    let mut ut: libc::utmpx = unsafe { std::mem::zeroed() };
    now_tv(&mut ut);
    ut.ut_type = libc::INIT_PROCESS;
    ut.ut_pid = pid;
    ut.ut_session = sid as _;
    ut.ut_id = ut_id(id);
    if let Some(l) = line {
        copy_field(
            &mut ut.ut_line,
            l.strip_prefix("/dev/").unwrap_or(l).as_bytes(),
        );
    }
    write_both(&ut, files);
    if mode == "login" || mode == "user" {
        ut.ut_type = libc::LOGIN_PROCESS;
        copy_field(&mut ut.ut_user, b"LOGIN");
        write_both(&ut, files);
    }
    if mode == "user" {
        ut.ut_type = libc::USER_PROCESS;
        write_both(&ut, files);
    }
}

/// systemd's utmp_put_dead_process(): the entry with this ut_id becomes
/// DEAD_PROCESS, only if it is still the main process's (a later getty or
/// login replaced it otherwise).
fn utmp_dead(id: &str, pid: i32, code: i32, status: i32, files: (&str, &str)) {
    let u = cstr(files.0);
    let mut lookup: libc::utmpx = unsafe { std::mem::zeroed() };
    lookup.ut_type = libc::INIT_PROCESS;
    lookup.ut_id = ut_id(id);
    let found = unsafe {
        libc::utmpxname(u.as_ptr());
        libc::setutxent();
        let f = libc::getutxid(&lookup);
        let copy = if f.is_null() { None } else { Some(*f) };
        libc::endutxent();
        copy
    };
    let Some(mut ut) = found else { return };
    if pid != 0 && ut.ut_pid != pid {
        return;
    }
    ut.ut_type = libc::DEAD_PROCESS;
    ut.ut_exit.e_termination = code as libc::c_short;
    ut.ut_exit.e_exit = status as libc::c_short;
    ut.ut_user = [0; libc::__UT_NAMESIZE];
    ut.ut_host = [0; libc::__UT_HOSTSIZE];
    now_tv(&mut ut);
    write_both(&ut, files);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vt_numbers() {
        assert_eq!(vt_number("/dev/tty1"), Some(1));
        assert_eq!(vt_number("/dev/tty12"), Some(12));
        assert_eq!(vt_number("/dev/tty0"), None);
        assert_eq!(vt_number("/dev/ttyS0"), None);
        assert_eq!(vt_number("/dev/tty"), None);
        assert_eq!(vt_number("/dev/console"), None);
        assert_eq!(vt_number("/dev/pts/3"), None);
    }

    #[test]
    fn ut_id_is_the_last_four_bytes() {
        let id = ut_id("tty1");
        assert_eq!(id.map(|c| c as u8), *b"tty1");
        let id = ut_id("ttyS0");
        assert_eq!(id.map(|c| c as u8), *b"tyS0");
        let id = ut_id("pts");
        assert_eq!(id.map(|c| c as u8), *b"pts\0");
    }

    #[test]
    fn sane_line_settings() {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        t.c_iflag = libc::IGNCR | libc::INLCR;
        sane_termios(&mut t);
        assert_eq!(t.c_iflag & (libc::IGNCR | libc::INLCR), 0);
        assert_ne!(t.c_iflag & libc::ICRNL, 0);
        assert_ne!(t.c_lflag & libc::ICANON, 0);
        assert_eq!(t.c_cc[libc::VINTR], 3);
        assert_eq!(t.c_cc[libc::VMIN], 1);
    }

    fn records(path: &std::path::Path) -> Vec<libc::utmpx> {
        let b = std::fs::read(path).unwrap();
        let n = std::mem::size_of::<libc::utmpx>();
        b.chunks_exact(n)
            .map(|c| unsafe { std::ptr::read_unaligned(c.as_ptr() as *const libc::utmpx) })
            .collect()
    }

    /// In files of our own: the init record, a dead record for another pid
    /// leaves it alone, the main pid's turns it into DEAD_PROCESS with the
    /// exit status, and wtmp gets each record appended.
    #[test]
    fn utmp_init_then_dead() {
        let dir = std::env::temp_dir().join(format!("uic-utmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (u, w) = (dir.join("utmp"), dir.join("wtmp"));
        std::fs::write(&u, b"").unwrap();
        std::fs::write(&w, b"").unwrap();
        let files = (u.to_str().unwrap(), w.to_str().unwrap());
        utmp_init("tty3", 4242, 4242, Some("/dev/tty3"), "init", files);
        let r = records(&u);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ut_type, libc::INIT_PROCESS);
        assert_eq!(r[0].ut_pid, 4242);
        assert_eq!(
            &r[0].ut_line[..5],
            &[b't', b't', b'y', b'3', 0].map(|b| b as libc::c_char)
        );
        utmp_dead("tty3", 1111, libc::CLD_EXITED, 0, files);
        assert_eq!(records(&u)[0].ut_type, libc::INIT_PROCESS);
        utmp_dead("tty3", 4242, libc::CLD_EXITED, 1, files);
        let r = records(&u);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ut_type, libc::DEAD_PROCESS);
        assert_eq!(r[0].ut_exit.e_exit, 1);
        let wt = records(&w);
        assert_eq!(
            wt.iter().map(|x| x.ut_type).collect::<Vec<_>>(),
            [libc::INIT_PROCESS, libc::DEAD_PROCESS]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
