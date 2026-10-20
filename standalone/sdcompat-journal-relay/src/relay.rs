//! sdcompat-journal-relay: journald's two client sockets for JOURNAL=no,
//! forwarding what arrives to syslog (/dev/log, rsyslog under s6).
//!
//! Without journald nothing listens on /run/systemd/journal, and some
//! programs refuse to run then: dbus-broker-launch exits when it cannot
//! connect to the native socket, so `systemd --user` has no session bus and
//! GNOME's greeter fails. systemd --user also connects its units' stdout and
//! stderr to the stream socket and falls back to /dev/null without it.
//!
//! - `socket` (SOCK_DGRAM): the native protocol, `KEY=VALUE\n` fields or
//!   `KEY\n<le64 size><data>\n` for binary ones, or an empty datagram with a
//!   memfd holding the fields (for large entries).
//! - `stdout` (SOCK_STREAM): a header of seven lines (identifier, unit,
//!   priority, level prefix, forward to syslog, kmsg, console), then one
//!   log line per line, optionally with a `<N>` priority prefix.
//!
//! Only MESSAGE, PRIORITY, SYSLOG_FACILITY, SYSLOG_IDENTIFIER and SYSLOG_PID
//! survive; other fields are dropped. Messages are dropped too when syslog
//! is not up or does not keep up: the relay must never block its clients.

use std::io::{BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Longest log line read from a stream (journald's LINE_MAX); longer lines
/// are split.
const LINE_MAX: u64 = 48 * 1024;
/// Largest native datagram read; journald clients switch to a memfd above
/// the socket's send buffer size, so this is rarely reached.
const DGRAM_MAX: usize = 256 * 1024;
/// Largest MESSAGE forwarded, and largest memfd read.
const MSG_MAX: usize = 16 * 1024;
const MEMFD_MAX: u64 = 1024 * 1024;

const LOG_USER: u8 = 1 << 3;
const LOG_INFO: u8 = 6;

pub struct Relay {
    dir: PathBuf,
    syslog: PathBuf,
    out: UnixDatagram,
}

/// One message on its way to syslog.
#[derive(Debug, PartialEq)]
pub struct Entry {
    /// Facility and level, as in `<PRI>`.
    pub pri: u8,
    pub ident: Option<String>,
    pub pid: Option<u32>,
    pub msg: Vec<u8>,
}

/// The fields of one native protocol entry.
pub fn parse_native(mut data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let Some(nl) = data.iter().position(|&b| b == b'\n') else { break };
        let line = &data[..nl];
        if let Some(eq) = line.iter().position(|&b| b == b'=') {
            out.push((String::from_utf8_lossy(&line[..eq]).into_owned(), line[eq + 1..].to_vec()));
            data = &data[nl + 1..];
        } else {
            // Binary field: name, newline, little-endian 64-bit size, data, newline.
            let rest = &data[nl + 1..];
            if rest.len() < 8 {
                break;
            }
            let size = u64::from_le_bytes(rest[..8].try_into().unwrap());
            let Some(end) = usize::try_from(size).ok().and_then(|s| s.checked_add(8)).filter(|&e| e < rest.len()) else {
                break;
            };
            out.push((String::from_utf8_lossy(line).into_owned(), rest[8..end].to_vec()));
            data = &rest[end + 1..];
        }
    }
    out
}

/// The entry to forward from native fields, or None without a MESSAGE.
pub fn native_entry(fields: &[(String, Vec<u8>)]) -> Option<Entry> {
    let get = |k: &str| fields.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_slice());
    let num = |k: &str| get(k).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.trim().parse::<u32>().ok());
    let msg = get("MESSAGE")?.to_vec();
    let level = num("PRIORITY").filter(|&p| p <= 7).unwrap_or(LOG_INFO as u32) as u8;
    let facility = num("SYSLOG_FACILITY").filter(|&f| f > 0 && f <= 23).map(|f| (f as u8) << 3).unwrap_or(LOG_USER);
    Some(Entry {
        pri: facility | level,
        ident: get("SYSLOG_IDENTIFIER").map(|v| String::from_utf8_lossy(v).into_owned()),
        pid: num("SYSLOG_PID"),
        msg,
    })
}

/// A stream connection's header.
#[derive(Debug, Default, PartialEq)]
pub struct StreamHeader {
    pub ident: String,
    pub unit: String,
    pub level: u8,
    pub level_prefix: bool,
}

impl StreamHeader {
    pub fn from_lines(lines: &[String]) -> StreamHeader {
        let line = |i: usize| lines.get(i).map(|s| s.trim_end_matches('\n').to_string()).unwrap_or_default();
        StreamHeader {
            ident: line(0),
            unit: line(1),
            level: line(2).parse().ok().filter(|&l| l <= 7).unwrap_or(LOG_INFO),
            level_prefix: line(3) == "1",
        }
    }

    /// The entry for one line (newline already removed), or None if empty.
    pub fn entry(&self, mut line: &[u8], pid: Option<u32>) -> Option<Entry> {
        let mut pri = LOG_USER | self.level;
        if self.level_prefix {
            if let Some((p, rest)) = level_prefix(line) {
                pri = if p >> 3 == 0 { LOG_USER | p } else { p };
                line = rest;
            }
        }
        if line.is_empty() {
            return None;
        }
        let ident = [&self.ident, &self.unit].into_iter().find(|s| !s.is_empty()).cloned();
        Some(Entry { pri, ident, pid, msg: line.to_vec() })
    }
}

/// A leading `<N>` (N up to 191, facility and level) and the rest.
fn level_prefix(line: &[u8]) -> Option<(u8, &[u8])> {
    let rest = line.strip_prefix(b"<")?;
    let end = rest.iter().position(|&b| b == b'>').filter(|&e| (1..=3).contains(&e))?;
    let p: u8 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok().filter(|&p| p <= 191)?;
    Some((p, &rest[end + 1..]))
}

/// The datagram for /dev/log: `<PRI>IDENT[PID]: MESSAGE`. syslog adds the
/// time when it receives it.
pub fn format_syslog(e: &Entry) -> Vec<u8> {
    let ident = e.ident.clone().or_else(|| e.pid.and_then(comm)).unwrap_or_else(|| "unknown".into());
    let ident: String = ident.chars().filter(|c| !c.is_whitespace() && !c.is_control() && *c != ':').collect();
    let mut out = format!("<{}>{ident}", e.pri).into_bytes();
    if let Some(pid) = e.pid {
        out.extend_from_slice(format!("[{pid}]").as_bytes());
    }
    out.extend_from_slice(b": ");
    let msg = e.msg.strip_suffix(b"\n").unwrap_or(&e.msg);
    out.extend_from_slice(&msg[..msg.len().min(MSG_MAX)]);
    out
}

fn comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim_end().to_string()).filter(|s| !s.is_empty())
}

fn set_opt(fd: RawFd, opt: libc::c_int, val: libc::c_int) {
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            opt,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as u32,
        );
    }
}

fn peer_pid(s: &UnixStream) -> Option<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
    };
    (r == 0 && cred.pid > 0).then_some(cred.pid as u32)
}

/// Remove a stale socket file before binding; refuse anything else, and a
/// socket someone still listens on (journald, if it runs after all).
fn unlink_socket(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            let live = UnixStream::connect(path).is_ok()
                || UnixDatagram::unbound().is_ok_and(|d| d.connect(path).is_ok());
            if live {
                return Err(format!("{}: in use (is journald running?)", path.display()));
            }
            std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))
        }
        Ok(_) => Err(format!("{}: exists and is not a socket", path.display())),
        Err(_) => Ok(()),
    }
}

impl Relay {
    pub fn new(dir: &Path, syslog: &Path) -> Result<Relay, String> {
        let out = UnixDatagram::unbound().map_err(|e| e.to_string())?;
        out.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Relay { dir: dir.to_path_buf(), syslog: syslog.to_path_buf(), out })
    }

    fn send(&self, e: &Entry) {
        // Not up yet, gone, or full: drop.
        let _ = self.out.send_to(&format_syslog(e), &self.syslog);
    }

    /// Bind both sockets (mode 0666, like journald's), then serve them in
    /// threads.
    pub fn start(self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let sock_path = self.dir.join("socket");
        let stdout_path = self.dir.join("stdout");
        unlink_socket(&sock_path)?;
        unlink_socket(&stdout_path)?;
        let dgram = UnixDatagram::bind(&sock_path).map_err(|e| format!("{}: {e}", sock_path.display()))?;
        let listener = UnixListener::bind(&stdout_path).map_err(|e| format!("{}: {e}", stdout_path.display()))?;
        for p in [&sock_path, &stdout_path] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o666)).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        // The sender's pid for SYSLOG_PID-less entries.
        set_opt(dgram.as_raw_fd(), libc::SO_PASSCRED, 1);
        set_opt(dgram.as_raw_fd(), libc::SO_RCVBUF, 8 << 20);

        let relay = Arc::new(self);
        let r = relay.clone();
        std::thread::spawn(move || loop {
            let Ok((data, fds, cred)) = crate::scm::recv_max(dgram.as_raw_fd(), DGRAM_MAX) else { continue };
            let data = match fds.into_iter().next() {
                Some(fd) if data.is_empty() => {
                    let mut buf = Vec::new();
                    let _ = std::fs::File::from(fd).take(MEMFD_MAX).read_to_end(&mut buf);
                    buf
                }
                _ => data,
            };
            if let Some(mut e) = native_entry(&parse_native(&data)) {
                e.pid = e.pid.or(cred.filter(|c| c.pid > 0).map(|c| c.pid as u32));
                r.send(&e);
            }
        });
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let r = relay.clone();
                std::thread::spawn(move || r.serve_stream(conn));
            }
        });
        Ok(())
    }

    fn serve_stream(&self, conn: UnixStream) {
        let pid = peer_pid(&conn);
        let mut reader = BufReader::new(conn);
        let mut header = Vec::new();
        let mut line = Vec::new();
        while header.len() < 7 {
            line.clear();
            match (&mut reader).take(LINE_MAX).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => header.push(String::from_utf8_lossy(&line).into_owned()),
            }
        }
        let header = StreamHeader::from_lines(&header);
        loop {
            line.clear();
            match (&mut reader).take(LINE_MAX).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let l = line.strip_suffix(b"\n").unwrap_or(&line);
                    if let Some(e) = header.entry(l, pid) {
                        self.send(&e);
                    }
                }
            }
        }
    }
}

/// Serve until killed.
pub fn serve_forever(dir: &Path, syslog: &Path, ready_fd: Option<RawFd>) -> i32 {
    if let Err(e) = Relay::new(dir, syslog).and_then(Relay::start) {
        eprintln!("sdcompat-journal-relay: {e}");
        return 1;
    }
    if let Some(fd) = ready_fd {
        // s6 readiness notification (notification-fd).
        unsafe {
            libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
            libc::close(fd);
        }
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_fields() {
        let mut d = b"PRIORITY=3\nSYSLOG_IDENTIFIER=dbus-broker-launch\nMESSAGE".to_vec();
        d.extend_from_slice(b"\n");
        d.extend_from_slice(&5u64.to_le_bytes());
        d.extend_from_slice(b"a\nb c\nCODE_LINE=12\n");
        let f = parse_native(&d);
        assert_eq!(f.len(), 4);
        assert_eq!(f[2], ("MESSAGE".into(), b"a\nb c".to_vec()));
        let e = native_entry(&f).unwrap();
        assert_eq!(e.pri, LOG_USER | 3);
        assert_eq!(e.ident.as_deref(), Some("dbus-broker-launch"));
        assert_eq!(format_syslog(&Entry { pid: Some(7), ..e }), b"<11>dbus-broker-launch[7]: a\nb c");
        // A size running past the end stops the parse.
        let mut bad = b"MESSAGE\n".to_vec();
        bad.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse_native(&bad).is_empty());
        assert_eq!(native_entry(&parse_native(b"PRIORITY=3\n")), None);
        let e = native_entry(&parse_native(b"MESSAGE=x\nSYSLOG_FACILITY=4\nPRIORITY=9\n")).unwrap();
        assert_eq!(e.pri, (4 << 3) | LOG_INFO);
    }

    #[test]
    fn stream_lines() {
        let lines: Vec<String> = ["gnome-shell\n", "\n", "6\n", "1\n", "0\n", "0\n", "0\n"].map(String::from).to_vec();
        let h = StreamHeader::from_lines(&lines);
        assert_eq!(h, StreamHeader { ident: "gnome-shell".into(), unit: "".into(), level: 6, level_prefix: true });
        assert_eq!(h.entry(b"<3>oops", Some(9)).unwrap().pri, LOG_USER | 3);
        assert_eq!(h.entry(b"<30>daemon", None).unwrap().pri, 30);
        assert_eq!(h.entry(b"<x>plain", None).unwrap().msg, b"<x>plain");
        assert_eq!(h.entry(b"", None), None);
        let e = h.entry(b"hello", Some(9)).unwrap();
        assert_eq!(format_syslog(&e), b"<14>gnome-shell[9]: hello");
    }
}
