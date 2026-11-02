//! What an Accept=yes .socket unit says, and what uinitconverter supports of
//! it: one ListenStream=, on a unix path (served by s6-ipcserverd) or an IP
//! port (s6-tcpserverd, from s6-networking), with one instance of the
//! socket's template service per connection (uinitconverter-runner `accept`
//! and `connection`). The generator, the runner and the daemon share these
//! rules and state names.

use std::net::{IpAddr, SocketAddr};

use crate::loader::Unit;

/// The socket's state directory holds these (RUN_DIR/units/<socket>/...).
/// NAccepted: the next connection's number, as systemd's n_accepted.
pub const ACCEPTED: &str = "accepted";
/// One file per running connection, named after its instance.
pub const CONNECTIONS_DIR: &str = "connections";
/// Lock for ACCEPTED (connections start concurrently).
pub const LOCK: &str = "accepted.lock";

/// systemd's default MaxConnections=.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 64;
/// s6-ipcserverd and s6-tcpserverd refuse -c above these.
pub const IPCSERVERD_MAX: u32 = 1000;
pub const TCPSERVERD_MAX: u32 = 16384;

/// The one thing an Accept=yes socket listens on.
#[derive(Debug, Clone, PartialEq)]
pub enum Listener {
    /// A path, or @name for the abstract namespace.
    Unix(String),
    /// A port or address:port, as socket::bind_inet takes it.
    Inet(String),
}

impl Listener {
    /// The s6 program that accepts on it (a tools:: name).
    pub fn server(&self) -> &'static str {
        match self {
            Listener::Unix(_) => "s6-ipcserverd",
            Listener::Inet(_) => "s6-tcpserverd",
        }
    }
    fn max(&self) -> u32 {
        match self {
            Listener::Unix(_) => IPCSERVERD_MAX,
            Listener::Inet(_) => TCPSERVERD_MAX,
        }
    }
}

pub fn is_accept(sock: &Unit) -> bool {
    sock.utype() == "socket" && sock.get_bool("Socket", "Accept", false)
}

/// The template the socket starts per connection: Service= or `<stem>@.service`.
pub fn template(sock: &Unit) -> String {
    sock.get("Socket", "Service")
        .unwrap_or_else(|| format!("{}@.service", sock.id.trim_end_matches(".socket")))
}

/// The socket's one ListenStream=, if that is all it listens on.
pub fn listener(sock: &Unit) -> Option<Listener> {
    let streams = sock.get_all("Socket", "ListenStream");
    let others = [
        "ListenDatagram",
        "ListenSequentialPacket",
        "ListenFIFO",
        "ListenSpecial",
        "ListenNetlink",
        "ListenMessageQueue",
        "ListenUSBFunction",
    ]
    .iter()
    .any(|k| !sock.get_all("Socket", k).is_empty());
    match streams.as_slice() {
        [p] if !others && (p.starts_with('/') || p.starts_with('@')) => {
            Some(Listener::Unix(p.clone()))
        }
        // AF_VSOCK isn't something s6-tcpserverd serves.
        [p] if !others && !p.starts_with("vsock:") => Some(Listener::Inet(p.clone())),
        _ => None,
    }
}

/// Why this socket can't be served, or None. `tmpl` is the loaded template.
pub fn unsupported(sock: &Unit, tmpl: &Unit) -> Option<String> {
    if listener(sock).is_none() {
        return Some(
            "Accept=yes: only one ListenStream= (a unix path or an IP port) is supported".into(),
        );
    }
    if !tmpl.is_loaded() {
        return Some(format!("Accept=yes: template {} not found", tmpl.id));
    }
    let ty = tmpl
        .get("Service", "Type")
        .unwrap_or_else(|| "simple".into());
    if !matches!(ty.as_str(), "simple" | "exec" | "oneshot") {
        return Some(format!(
            "Accept=yes: template {} has Type={ty} (only simple, exec and oneshot)",
            tmpl.id
        ));
    }
    if tmpl.get_all("Service", "ExecStart").is_empty() {
        return Some(format!(
            "Accept=yes: template {} has no ExecStart=",
            tmpl.id
        ));
    }
    None
}

/// The server's -c and -C from MaxConnections= and MaxConnectionsPerSource=
/// (which defaults to MaxConnections=). Clamped to what s6-ipcserverd or
/// s6-tcpserverd takes; the second value says whether it was clamped.
pub fn limits(sock: &Unit) -> ((u32, u32), bool) {
    let num = |k: &str| {
        sock.get("Socket", k)
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&n| n > 0)
    };
    let max = num("MaxConnections").unwrap_or(DEFAULT_MAX_CONNECTIONS);
    let per = num("MaxConnectionsPerSource").unwrap_or(max);
    let limit = listener(sock).map(|l| l.max()).unwrap_or(IPCSERVERD_MAX);
    let c = max.min(limit);
    let lc = per.min(c);
    ((c, lc), c != max || lc != per)
}

/// systemd's instance name for a unix connection: `<prefix>@<n>-<pid>-<uid>.service`
/// (instance_from_socket(); "unknown" without peer credentials).
pub fn instance_name(template: &str, n: u64, peer: Option<(i32, u32)>) -> String {
    let prefix = template.split('@').next().unwrap_or(template);
    match peer {
        Some((pid, uid)) => format!("{prefix}@{n}-{pid}-{uid}.service"),
        None => format!("{prefix}@{n}-unknown.service"),
    }
}

/// An IPv4-mapped IPv6 address as IPv4, as systemd prints them.
pub fn plain_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// systemd's instance name for a TCP connection:
/// `<prefix>@<n>-<local ip>:<port>-<remote ip>:<port>.service`, IPv6
/// without brackets, IPv4-mapped addresses as IPv4 when both are.
pub fn instance_name_inet(template: &str, n: u64, local: SocketAddr, remote: SocketAddr) -> String {
    let prefix = template.split('@').next().unwrap_or(template);
    let both_mapped = [local, remote].iter().all(|a| plain_ip(a.ip()).is_ipv4());
    let ip = |a: SocketAddr| {
        if both_mapped {
            plain_ip(a.ip())
        } else {
            a.ip()
        }
    };
    format!(
        "{prefix}@{n}-{}:{}-{}:{}.service",
        ip(local),
        local.port(),
        ip(remote),
        remote.port()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::LoadState;
    use crate::unit::UnitFile;

    fn unit(id: &str, text: &str) -> Unit {
        let mut file = UnitFile::default();
        file.parse_str(text);
        Unit {
            id: id.into(),
            names: vec![id.into()],
            fragment: None,
            dropins: Vec::new(),
            load_state: LoadState::Loaded,
            load_error: None,
            file,
        }
    }

    #[test]
    fn template_and_instance_names() {
        let s = unit("polkit-agent-helper.socket", "[Socket]\nAccept=yes\n");
        assert_eq!(template(&s), "polkit-agent-helper@.service");
        let s = unit("x.socket", "[Socket]\nAccept=yes\nService=y@.service\n");
        assert_eq!(template(&s), "y@.service");
        assert_eq!(
            instance_name("polkit-agent-helper@.service", 0, Some((1234, 1000))),
            "polkit-agent-helper@0-1234-1000.service"
        );
        assert_eq!(instance_name("e@.service", 7, None), "e@7-unknown.service");
    }

    #[test]
    fn supported_listeners() {
        let svc = unit("e@.service", "[Service]\nExecStart=/bin/cat\n");
        let ok = unit(
            "e.socket",
            "[Socket]\nAccept=yes\nListenStream=/run/e.sock\n",
        );
        assert!(is_accept(&ok));
        assert_eq!(unsupported(&ok, &svc), None);
        assert_eq!(listener(&ok), Some(Listener::Unix("/run/e.sock".into())));
        let tcp = unit("e.socket", "[Socket]\nAccept=yes\nListenStream=22\n");
        assert_eq!(unsupported(&tcp, &svc), None);
        assert_eq!(listener(&tcp).unwrap().server(), "s6-tcpserverd");
        let vsock = unit("e.socket", "[Socket]\nAccept=yes\nListenStream=vsock::22\n");
        assert!(unsupported(&vsock, &svc).is_some());
        let two = unit(
            "e.socket",
            "[Socket]\nAccept=yes\nListenStream=/run/a\nListenStream=/run/b\n",
        );
        assert!(unsupported(&two, &svc).is_some());
        let mixed = unit(
            "e.socket",
            "[Socket]\nAccept=yes\nListenStream=/run/a\nListenFIFO=/run/f\n",
        );
        assert!(unsupported(&mixed, &svc).is_some());
        let notify = unit("e@.service", "[Service]\nType=notify\nExecStart=/bin/cat\n");
        assert!(unsupported(&ok, &notify).unwrap().contains("Type=notify"));
        let oneshot = unit(
            "e@.service",
            "[Service]\nType=oneshot\nExecStart=/bin/cat\n",
        );
        assert_eq!(unsupported(&ok, &oneshot), None);
    }

    #[test]
    fn connection_limits() {
        let s = unit("e.socket", "[Socket]\nAccept=yes\n");
        assert_eq!(limits(&s), ((64, 64), false));
        let s = unit("e.socket", "[Socket]\nMaxConnections=5\n");
        assert_eq!(limits(&s), ((5, 5), false));
        let s = unit(
            "e.socket",
            "[Socket]\nMaxConnections=10\nMaxConnectionsPerSource=2\n",
        );
        assert_eq!(limits(&s), ((10, 2), false));
        let s = unit("e.socket", "[Socket]\nMaxConnections=5000\n");
        assert_eq!(limits(&s), ((1000, 1000), true));
        let s = unit(
            "e.socket",
            "[Socket]\nMaxConnections=4\nMaxConnectionsPerSource=9\n",
        );
        assert_eq!(limits(&s), ((4, 4), true));
        let s = unit(
            "e.socket",
            "[Socket]\nListenStream=22\nMaxConnections=5000\n",
        );
        assert_eq!(limits(&s), ((5000, 5000), false));
        let s = unit(
            "e.socket",
            "[Socket]\nListenStream=22\nMaxConnections=20000\n",
        );
        assert_eq!(limits(&s), ((16384, 16384), true));
    }

    #[test]
    fn inet_instance_names() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert_eq!(
            instance_name_inet("sshd@.service", 0, a("127.0.0.1:22"), a("127.0.0.1:5000")),
            "sshd@0-127.0.0.1:22-127.0.0.1:5000.service"
        );
        assert_eq!(
            instance_name_inet(
                "sshd@.service",
                1,
                a("[::ffff:10.0.0.1]:22"),
                a("[::ffff:10.0.0.2]:6000")
            ),
            "sshd@1-10.0.0.1:22-10.0.0.2:6000.service"
        );
        assert_eq!(
            instance_name_inet("sshd@.service", 2, a("[::1]:22"), a("[::1]:7000")),
            "sshd@2-::1:22-::1:7000.service"
        );
        assert_eq!(
            plain_ip("::ffff:1.2.3.4".parse().unwrap()).to_string(),
            "1.2.3.4"
        );
    }
}
