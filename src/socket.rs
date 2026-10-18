//! Socket units: bind the listening sockets and hand them over with
//! LISTEN_FDS / LISTEN_FDNAMES (sd_listen_fds protocol, Accept=no).

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::loader::Unit;
use crate::util;

pub struct BoundSocket {
    pub fd: i32,
    pub name: String,
    /// Filesystem path to remove on stop (RemoveOnStop=).
    pub path: Option<String>,
}

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn set_opt(fd: i32, level: i32, opt: i32, val: i32) {
    unsafe {
        libc::setsockopt(
            fd,
            level,
            opt,
            &val as *const i32 as *const libc::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

pub fn bind_unix(path: &str, ty: i32, unit: &Unit) -> Result<i32, String> {
    let abstract_ns = path.starts_with('@');
    if !abstract_ns {
        if let Some(parent) = Path::new(path).parent() {
            let mode = unit
                .get("Socket", "DirectoryMode")
                .and_then(|m| u32::from_str_radix(&m, 8).ok())
                .unwrap_or(0o755);
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(mode));
            }
        }
        // Remove a stale socket from a previous run.
        if fs::symlink_metadata(path)
            .map(|m| std::os::unix::fs::FileTypeExt::is_socket(&m.file_type()))
            .unwrap_or(false)
        {
            let _ = fs::remove_file(path);
        }
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, ty | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", errno()));
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as u16;
    let bytes = path.as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(format!("{path}: socket path too long"));
    }
    for (i, b) in bytes.iter().enumerate() {
        addr.sun_path[i] = if abstract_ns && i == 0 { 0 } else { *b as libc::c_char };
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + usize::from(!abstract_ns)) as u32;
    if unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) } < 0 {
        return Err(format!("bind {path}: {}", errno()));
    }
    if !abstract_ns {
        let mode = unit
            .get("Socket", "SocketMode")
            .and_then(|m| u32::from_str_radix(&m, 8).ok())
            .unwrap_or(0o666);
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
        let uid = unit
            .get("Socket", "SocketUser")
            .and_then(|u| util::lookup_user(&u))
            .map(|u| u.uid);
        let gid = unit.get("Socket", "SocketGroup").and_then(|g| util::lookup_group(&g));
        if uid.is_some() || gid.is_some() {
            let c = CString::new(path).unwrap();
            unsafe {
                libc::chown(c.as_ptr(), uid.unwrap_or(u32::MAX), gid.unwrap_or(u32::MAX));
            }
        }
    }
    Ok(fd)
}

fn bind_inet(spec: &str, ty: i32, unit: &Unit) -> Result<i32, String> {
    // Accepted forms: "80", "0.0.0.0:80", "[::1]:80", "[::]:80"
    let (host, port) = if let Ok(p) = spec.parse::<u16>() {
        (None, p)
    } else if let Some(rest) = spec.strip_prefix('[') {
        let (h, p) = rest.split_once("]:").ok_or_else(|| format!("bad address {spec}"))?;
        (Some(h.to_string()), p.parse().map_err(|_| format!("bad port in {spec}"))?)
    } else {
        let (h, p) = spec.rsplit_once(':').ok_or_else(|| format!("bad address {spec}"))?;
        (Some(h.to_string()), p.parse().map_err(|_| format!("bad port in {spec}"))?)
    };

    let v4: Option<std::net::Ipv4Addr> = host.as_deref().and_then(|h| h.parse().ok());
    let family = if v4.is_some() { libc::AF_INET } else { libc::AF_INET6 };
    let fd = unsafe { libc::socket(family, ty | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", errno()));
    }
    set_opt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);
    if unit.get_bool("Socket", "ReusePort", false) {
        set_opt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1);
    }
    if unit.get_bool("Socket", "FreeBind", false) {
        set_opt(fd, libc::IPPROTO_IP, libc::IP_FREEBIND, 1);
    }
    if unit.get_bool("Socket", "KeepAlive", false) {
        set_opt(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1);
    }
    if ty == libc::SOCK_STREAM && unit.get_bool("Socket", "NoDelay", false) {
        set_opt(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, 1);
    }

    let r = if let Some(ip) = v4 {
        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr { s_addr: u32::from(ip).to_be() },
            sin_zero: [0; 8],
        };
        unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as u32,
            )
        }
    } else {
        let ip: std::net::Ipv6Addr = match host.as_deref() {
            None => std::net::Ipv6Addr::UNSPECIFIED,
            Some(h) => h.parse().map_err(|_| format!("bad address {spec}"))?,
        };
        let only = unit.get("Socket", "BindIPv6Only").unwrap_or_default() == "ipv6-only";
        set_opt(fd, libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, i32::from(only));
        let mut addr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        addr.sin6_family = libc::AF_INET6 as u16;
        addr.sin6_port = port.to_be();
        addr.sin6_addr.s6_addr = ip.octets();
        unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in6>() as u32,
            )
        }
    };
    if r < 0 {
        return Err(format!("bind {spec}: {}", errno()));
    }
    Ok(fd)
}

/// Bind all Listen*= entries of a socket unit.
pub fn bind_all(sock: &Unit) -> Result<Vec<BoundSocket>, String> {
    let name = sock
        .get("Socket", "FileDescriptorName")
        .unwrap_or_else(|| sock.id.clone());
    let backlog = sock
        .get("Socket", "Backlog")
        .and_then(|b| b.parse().ok())
        .unwrap_or(libc::SOMAXCONN);
    let pass_cred = sock.get_bool("Socket", "PassCredentials", false);
    let pass_sec = sock.get_bool("Socket", "PassSecurity", false);
    let size = |k: &str| -> Option<i32> {
        let v = sock.get("Socket", k)?;
        let (n, m) = match v.chars().last()? {
            'K' => (&v[..v.len() - 1], 1i64 << 10),
            'M' => (&v[..v.len() - 1], 1 << 20),
            'G' => (&v[..v.len() - 1], 1 << 30),
            _ => (v.as_str(), 1),
        };
        n.parse::<i64>().ok().map(|n| (n * m).min(i32::MAX as i64) as i32)
    };
    let (rcvbuf, sndbuf) = (size("ReceiveBuffer"), size("SendBuffer"));
    let mut out = Vec::new();

    for (key, ty) in [
        ("ListenStream", libc::SOCK_STREAM),
        ("ListenDatagram", libc::SOCK_DGRAM),
        ("ListenSequentialPacket", libc::SOCK_SEQPACKET),
    ] {
        for spec in sock.get_all("Socket", key) {
            let is_path = spec.starts_with('/') || spec.starts_with('@');
            let fd = if is_path {
                bind_unix(&spec, ty, sock)?
            } else {
                bind_inet(&spec, ty, sock)?
            };
            if pass_cred {
                set_opt(fd, libc::SOL_SOCKET, libc::SO_PASSCRED, 1);
            }
            if pass_sec {
                set_opt(fd, libc::SOL_SOCKET, libc::SO_PASSSEC, 1);
            }
            if let Some(b) = rcvbuf {
                // FORCE needs CAP_NET_ADMIN; fall back to the capped variant.
                set_opt(fd, libc::SOL_SOCKET, libc::SO_RCVBUFFORCE, b);
                set_opt(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, b);
            }
            if let Some(b) = sndbuf {
                set_opt(fd, libc::SOL_SOCKET, libc::SO_SNDBUFFORCE, b);
                set_opt(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, b);
            }
            if ty != libc::SOCK_DGRAM && unsafe { libc::listen(fd, backlog) } < 0 {
                return Err(format!("listen {spec}: {}", errno()));
            }
            out.push(BoundSocket {
                fd,
                name: name.clone(),
                path: (spec.starts_with('/')).then(|| spec.clone()),
            });
        }
    }
    for spec in sock.get_all("Socket", "ListenFIFO") {
        let c = CString::new(spec.as_str()).map_err(|e| e.to_string())?;
        let mode = sock
            .get("Socket", "SocketMode")
            .and_then(|m| u32::from_str_radix(&m, 8).ok())
            .unwrap_or(0o666);
        unsafe { libc::mkfifo(c.as_ptr(), mode) };
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(format!("open fifo {spec}: {}", errno()));
        }
        out.push(BoundSocket { fd, name: name.clone(), path: Some(spec.clone()) });
    }
    // Symlinks= (e.g. /dev/log -> /run/systemd/journal/dev-log)
    let first_path = out.iter().find_map(|b| b.path.clone());
    if let Some(target) = first_path {
        for link in sock.get_words("Socket", "Symlinks") {
            let _ = fs::remove_file(&link);
            if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                eprintln!("sdcompat: {}: symlink {link}: {e}", sock.id);
            }
        }
    }
    for key in ["ListenNetlink", "ListenSpecial", "ListenMessageQueue", "ListenUSBFunction"] {
        if sock.get("Socket", key).is_some() {
            eprintln!("sdcompat: {}: {key}= is not supported, skipped", sock.id);
        }
    }
    Ok(out)
}

pub fn remove_paths(sock: &Unit) {
    if !sock.get_bool("Socket", "RemoveOnStop", false) {
        return;
    }
    for key in ["ListenStream", "ListenDatagram", "ListenSequentialPacket", "ListenFIFO"] {
        for spec in sock.get_all("Socket", key) {
            if spec.starts_with('/') {
                let _ = fs::remove_file(&spec);
            }
        }
    }
}
