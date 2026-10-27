//! Units wanted by devices. udev rules set SYSTEMD_WANTS on devices tagged
//! "systemd" (99-systemd.rules: bluetooth.target for an hci device,
//! printer.target, sound.target, ...), and systemd starts those units when the
//! device appears. We do the same from udevd's netlink events, and at startup
//! from udev's database for devices that appeared before the daemon.
//!
//! The devices already seen are kept in RUN_DIR/devices, so a restarted
//! daemon does not start their units again (one the admin stopped stays
//! stopped). Like systemd, a "change" event starts only units newly added to
//! SYSTEMD_WANTS.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;

use super::{op, job_path, Daemon, MANAGER_IFACE, MANAGER_PATH};

/// Netlink multicast group udevd sends processed events to.
const UDEV_GROUP: u32 = 2;
const UDEV_MAGIC: u32 = 0xfeed_cafe;
/// struct udev_monitor_netlink_header in libudev.
const HEADER_LEN: usize = 40;

/// A device's state as far as wanted units are concerned.
#[derive(Debug, PartialEq)]
pub struct Event {
    /// udev's device id: b8:0, c189:3, n2 or +subsystem:sysname.
    pub id: String,
    /// Tagged "systemd", not SYSTEMD_READY=0 and not removed.
    pub present: bool,
    pub wants: Vec<String>,
}

enum Msg {
    Event(Event),
    /// Events were lost (receive buffer overflow): resync from the database.
    Rescan,
}

/// device id -> the wants already started for it
type Plugged = BTreeMap<String, Vec<String>>;

impl Daemon {
    pub(super) async fn watch_devices(self: Arc<Self>) {
        let Some(udev_dir) = self.cfg.udev_dir.clone() else { return };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Listen before scanning, so nothing between the two is missed.
        if self.cfg.udev_monitor {
            match open_monitor() {
                Ok(fd) => {
                    std::thread::spawn(move || monitor_loop(fd, tx));
                }
                Err(e) => eprintln!("uinitconverterd: cannot listen to udev events: {e}"),
            }
        } else {
            drop(tx);
        }
        let state = self.cfg.run_dir.join("devices");
        let mut plugged = load_plugged(&state);
        self.sync_devices(&mut plugged, scan_db(&udev_dir));
        save_plugged(&state, &plugged);
        while let Some(m) = rx.recv().await {
            match m {
                Msg::Event(e) => self.device_event(&mut plugged, e),
                Msg::Rescan => self.sync_devices(&mut plugged, scan_db(&udev_dir)),
            }
            save_plugged(&state, &plugged);
        }
    }

    /// Make `plugged` match the devices udev's database lists.
    fn sync_devices(self: &Arc<Self>, plugged: &mut Plugged, current: Vec<Event>) {
        plugged.retain(|id, _| current.iter().any(|e| e.present && &e.id == id));
        for e in current {
            self.device_event(plugged, e);
        }
    }

    fn device_event(self: &Arc<Self>, plugged: &mut Plugged, e: Event) {
        if !e.present {
            plugged.remove(&e.id);
            return;
        }
        let old = plugged.get(&e.id);
        let new: Vec<String> = e.wants.iter().filter(|w| !old.is_some_and(|o| o.contains(w))).cloned().collect();
        for name in new {
            let d = self.clone();
            let dev = e.id.clone();
            tokio::spawn(async move { d.start_wanted(name, dev).await });
        }
        plugged.insert(e.id, e.wants);
    }

    async fn start_wanted(&self, name: String, dev: String) {
        // systemd-backlight@, systemd-rfkill and the like: not ours to run.
        if self.cfg.is_excluded(&name) {
            return;
        }
        match self.check_job(&name, "start", false, false).await {
            Ok((id, unit)) => {
                let job = self.new_job(&id, "start");
                self.emit(MANAGER_PATH, MANAGER_IFACE, "JobNew", &(job, op(job_path(job)), id.clone())).await;
                let r = self.execute_job(job, &id, &unit, "start", "replace").await;
                eprintln!("uinitconverterd: {id} wanted by device {dev}: {r}");
            }
            Err(e) => eprintln!("uinitconverterd: device {dev} wants {name}: {}", e.msg),
        }
    }
}

fn load_plugged(path: &Path) -> Plugged {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let id = words.next()?.to_string();
            Some((id, words.map(String::from).collect()))
        })
        .collect()
}

fn save_plugged(path: &Path, plugged: &Plugged) {
    let mut text = String::new();
    for (id, wants) in plugged {
        text.push_str(id);
        for w in wants {
            text.push(' ');
            text.push_str(w);
        }
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)) {
        eprintln!("uinitconverterd: cannot write {}: {e}", path.display());
    }
}

/// SYSTEMD_WANTS value -> unit names (a name without a unit suffix is a
/// service, as in systemd). A template gets the device's sysfs path,
/// escaped, as its instance (systemd's device_add_udev_wants()); without
/// the path it is left out.
pub fn parse_wants(v: &str, syspath: Option<&str>) -> Vec<String> {
    const SUFFIXES: &[&str] = &[
        ".service", ".socket", ".target", ".device", ".mount", ".automount", ".swap", ".timer", ".path", ".slice",
        ".scope",
    ];
    v.split_whitespace()
        .map(|w| if SUFFIXES.iter().any(|s| w.ends_with(s)) { w.to_string() } else { format!("{w}.service") })
        .filter_map(|w| match w.split_once("@.") {
            Some((prefix, suffix)) => syspath.map(|p| format!("{prefix}@{}.{suffix}", crate::util::unit_name_path_escape(p))),
            None => Some(w),
        })
        .collect()
}

/// The sysfs path of a udev database id (b8:0, c189:3, n3, +subsystem:name).
fn syspath_of_id(id: &str) -> Option<String> {
    let link = if let Some(dev) = id.strip_prefix('b') {
        format!("/sys/dev/block/{dev}")
    } else if let Some(dev) = id.strip_prefix('c') {
        format!("/sys/dev/char/{dev}")
    } else if let Some(idx) = id.strip_prefix('n') {
        let rd = std::fs::read_dir("/sys/class/net").ok()?;
        rd.flatten()
            .map(|e| e.path())
            .find(|p| std::fs::read_to_string(p.join("ifindex")).is_ok_and(|i| i.trim() == idx))?
            .display()
            .to_string()
    } else {
        let (subsystem, name) = id.strip_prefix('+')?.split_once(':')?;
        [format!("/sys/bus/{subsystem}/devices/{name}"), format!("/sys/class/{subsystem}/{name}")]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())?
    };
    std::fs::canonicalize(link).ok().map(|p| p.display().to_string())
}

fn has_tag(tags: &str, tag: &str) -> bool {
    tags.split(':').any(|t| t == tag)
}

/// Event from the properties of a udev netlink message.
pub fn event_from_props(p: &BTreeMap<String, String>) -> Option<Event> {
    let get = |k: &str| p.get(k).map(String::as_str);
    let devpath = get("DEVPATH")?;
    let subsystem = get("SUBSYSTEM")?;
    let num = |k: &str| get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let id = if num("MAJOR") > 0 {
        let kind = if subsystem == "block" { 'b' } else { 'c' };
        format!("{kind}{}:{}", num("MAJOR"), num("MINOR"))
    } else if num("IFINDEX") > 0 {
        format!("n{}", num("IFINDEX"))
    } else {
        let sysname = devpath.rsplit('/').next()?;
        format!("+{subsystem}:{sysname}")
    };
    // Since systemd 247 the tags of the current event count (CURRENT_TAGS).
    let tags = get("CURRENT_TAGS").or(get("TAGS")).unwrap_or("");
    let present = get("ACTION") != Some("remove")
        && has_tag(tags, "systemd")
        && get("SYSTEMD_READY").and_then(crate::unit::parse_bool) != Some(false);
    let syspath = format!("/sys{devpath}");
    Some(Event { id, present, wants: get("SYSTEMD_WANTS").map(|v| parse_wants(v, Some(&syspath))).unwrap_or_default() })
}

/// Event from a udev database file (data/<id>: "E:KEY=value", "G:tag",
/// "Q:current tag" lines).
pub fn event_from_db(id: &str, text: &str, syspath: Option<&str>) -> Event {
    let mut tags = Vec::new();
    let mut current = Vec::new();
    let mut wants = Vec::new();
    let mut ready = true;
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("G:") {
            tags.push(t);
        } else if let Some(t) = line.strip_prefix("Q:") {
            current.push(t);
        } else if let Some(v) = line.strip_prefix("E:SYSTEMD_WANTS=") {
            wants = parse_wants(v, syspath);
        } else if let Some(v) = line.strip_prefix("E:SYSTEMD_READY=") {
            ready = crate::unit::parse_bool(v) != Some(false);
        }
    }
    let tags = if current.is_empty() { tags } else { current };
    Event { id: id.to_string(), present: ready && tags.contains(&"systemd"), wants }
}

/// Every device tagged "systemd" in udev's database.
fn scan_db(udev_dir: &Path) -> Vec<Event> {
    let Ok(dir) = std::fs::read_dir(udev_dir.join("tags/systemd")) else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            let text = std::fs::read_to_string(udev_dir.join("data").join(&id)).ok()?;
            Some(event_from_db(&id, &text, syspath_of_id(&id).as_deref()))
        })
        .collect()
}

/// Properties of a message udevd sent to its netlink group; None for anything else.
pub fn parse_message(buf: &[u8]) -> Option<BTreeMap<String, String>> {
    if buf.len() < HEADER_LEN || &buf[..8] != b"libudev\0" {
        return None;
    }
    let word = |i: usize| -> [u8; 4] { buf[i..i + 4].try_into().unwrap() };
    if u32::from_be_bytes(word(8)) != UDEV_MAGIC {
        return None;
    }
    let off = u32::from_ne_bytes(word(16)) as usize;
    let len = u32::from_ne_bytes(word(20)) as usize;
    let props = buf.get(off..off.checked_add(len)?)?;
    Some(
        props
            .split(|&b| b == 0)
            .filter_map(|kv| {
                let kv = std::str::from_utf8(kv).ok()?;
                let (k, v) = kv.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect(),
    )
}

fn setsockopt(fd: &OwnedFd, opt: libc::c_int, val: libc::c_int) -> io::Result<()> {
    let r = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            opt,
            &val as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn open_monitor() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    setsockopt(&fd, libc::SO_PASSCRED, 1)?;
    // A coldplug floods the socket; the default buffer drops events.
    if setsockopt(&fd, libc::SO_RCVBUFFORCE, 8 << 20).is_err() {
        let _ = setsockopt(&fd, libc::SO_RCVBUF, 8 << 20);
    }
    let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    sa.nl_groups = UDEV_GROUP;
    let r = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Receive one message. Ok(None): not from udevd (only root may send to the
/// udev group, which the credentials show), truncated, or not a udev message.
pub fn recv_message(fd: &OwnedFd) -> io::Result<Option<BTreeMap<String, String>>> {
    let mut buf = vec![0u8; 16384];
    let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    let mut cbuf = [0u64; 8];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = &mut sa as *mut libc::sockaddr_nl as *mut libc::c_void;
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = std::mem::size_of_val(&cbuf) as _;
    let n = unsafe { libc::recvmsg(fd.as_raw_fd(), &mut msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if msg.msg_flags & libc::MSG_TRUNC != 0 || sa.nl_groups != UDEV_GROUP || sa.nl_pid == 0 {
        return Ok(None);
    }
    let mut uid = None;
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_CREDENTIALS {
                let cred = std::ptr::read_unaligned(libc::CMSG_DATA(c) as *const libc::ucred);
                uid = Some(cred.uid);
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    if uid != Some(0) {
        return Ok(None);
    }
    Ok(parse_message(&buf[..n as usize]))
}

fn monitor_loop(fd: OwnedFd, tx: tokio::sync::mpsc::UnboundedSender<Msg>) {
    loop {
        let msg = match recv_message(&fd) {
            Ok(Some(props)) => match event_from_props(&props) {
                Some(e) => Msg::Event(e),
                None => continue,
            },
            Ok(None) => continue,
            Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => Msg::Rescan,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("uinitconverterd: udev monitor: {e}");
                return;
            }
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A message as udevd sends it.
    fn udev_message(kv: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (k, v) in kv {
            body.extend_from_slice(format!("{k}={v}").as_bytes());
            body.push(0);
        }
        let mut m = b"libudev\0".to_vec();
        m.extend_from_slice(&UDEV_MAGIC.to_be_bytes());
        for v in [HEADER_LEN as u32, HEADER_LEN as u32, body.len() as u32, 0, 0, 0, 0] {
            m.extend_from_slice(&v.to_ne_bytes());
        }
        m.extend_from_slice(&body);
        m
    }

    const HCI: &[(&str, &str)] = &[
        ("ACTION", "add"),
        ("DEVPATH", "/devices/pci0000:00/0000:00:08.1/0000:65:00.4/usb3/3-1/3-1:1.0/bluetooth/hci0"),
        ("SUBSYSTEM", "bluetooth"),
        ("TAGS", ":systemd:"),
        ("CURRENT_TAGS", ":systemd:"),
        ("SYSTEMD_WANTS", "bluetooth.target"),
        ("SYSTEMD_USER_WANTS", "bluetooth.target"),
    ];

    #[test]
    fn wants() {
        assert_eq!(parse_wants(" a  b.target c@x:y.service ", None), ["a.service", "b.target", "c@x:y.service"]);
        // Templates take the escaped sysfs path; without it they are left out.
        assert_eq!(parse_wants("t@.service u@", Some("/sys/devices/a-b")), ["t@sys-devices-a\\x2db.service", "u@sys-devices-a\\x2db.service"]);
        assert!(parse_wants("t@.service", None).is_empty());
    }

    #[test]
    fn message() {
        let p = parse_message(&udev_message(HCI)).unwrap();
        assert_eq!(p.get("SYSTEMD_WANTS").unwrap(), "bluetooth.target");
        assert_eq!(
            event_from_props(&p),
            Some(Event { id: "+bluetooth:hci0".into(), present: true, wants: vec!["bluetooth.target".into()] })
        );
        // Kernel uevents ("add@/devices/...") and garbage are ignored.
        assert_eq!(parse_message(b"add@/devices/x\0ACTION=add\0"), None);
        let mut m = udev_message(HCI);
        m[20..24].copy_from_slice(&9999u32.to_ne_bytes());
        assert_eq!(parse_message(&m), None);
    }

    #[test]
    fn device_ids_and_presence() {
        let e = event_from_props(&props(&[
            ("ACTION", "add"),
            ("DEVPATH", "/devices/x/block/sda"),
            ("SUBSYSTEM", "block"),
            ("MAJOR", "8"),
            ("MINOR", "0"),
            ("TAGS", ":systemd:"),
        ]))
        .unwrap();
        assert_eq!((e.id.as_str(), e.present), ("b8:0", true));
        let e = event_from_props(&props(&[
            ("ACTION", "add"),
            ("DEVPATH", "/devices/x/net/wlp2s0"),
            ("SUBSYSTEM", "net"),
            ("IFINDEX", "3"),
            ("TAGS", ":systemd:"),
            ("SYSTEMD_READY", "0"),
        ]))
        .unwrap();
        assert_eq!((e.id.as_str(), e.present), ("n3", false));
        let mut p = props(HCI);
        p.insert("ACTION".into(), "remove".into());
        assert!(!event_from_props(&p).unwrap().present);
        // The tag was dropped by the current event.
        let mut p = props(HCI);
        p.insert("CURRENT_TAGS".into(), ":seat:".into());
        assert!(!event_from_props(&p).unwrap().present);
    }

    #[test]
    fn database() {
        let e = event_from_db("+bluetooth:hci0", "I:123\nE:SYSTEMD_WANTS=bluetooth.target\nG:systemd\nQ:systemd\nV:1\n", None);
        assert_eq!(e, Event { id: "+bluetooth:hci0".into(), present: true, wants: vec!["bluetooth.target".into()] });
        assert!(!event_from_db("x", "G:systemd\nQ:seat\n", None).present);
        assert!(!event_from_db("x", "G:systemd\nE:SYSTEMD_READY=0\n", None).present);
        assert!(event_from_db("x", "G:systemd\n", None).present);
    }

    #[test]
    fn plugged_round_trip() {
        let dir = std::env::temp_dir().join(format!("uinitconverter-devices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devices");
        let mut p = Plugged::new();
        p.insert("+bluetooth:hci0".into(), vec!["bluetooth.target".into()]);
        p.insert("c189:3".into(), Vec::new());
        save_plugged(&path, &p);
        assert_eq!(load_plugged(&path), p);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Real netlink socket. Sending to the udev group needs CAP_NET_ADMIN, so
    /// run it in a user and network namespace:
    ///   unshare -rn cargo test -- --ignored netlink_monitor
    #[test]
    #[ignore]
    fn netlink_monitor() {
        let rx = open_monitor().unwrap();
        let tx = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
        assert!(tx >= 0);
        let tx = unsafe { OwnedFd::from_raw_fd(tx) };
        let send = |m: &[u8]| {
            let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
            sa.nl_groups = UDEV_GROUP;
            let r = unsafe {
                libc::sendto(
                    tx.as_raw_fd(),
                    m.as_ptr() as *const libc::c_void,
                    m.len(),
                    0,
                    &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
                )
            };
            assert_eq!(r, m.len() as isize, "{}", io::Error::last_os_error());
        };
        send(&udev_message(HCI));
        let p = recv_message(&rx).unwrap().expect("udev message accepted");
        assert_eq!(event_from_props(&p).unwrap().wants, ["bluetooth.target"]);
    }
}
