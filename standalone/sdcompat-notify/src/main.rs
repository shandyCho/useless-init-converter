//! sdcompat-notify: a systemd-notify replacement. Sends one sd_notify
//! message to the socket in $NOTIFY_SOCKET (a path, or @name for the
//! abstract namespace). Works under any service manager that sets it.

use std::process::exit;

const USAGE: &str = "\
usage: sdcompat-notify [--ready] [--stopping] [--reloading] [--status=TEXT] [--pid=PID] [VAR=VALUE...]
";

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        exit(0);
    }
    exit(notify(&args))
}

/// Send the message the arguments describe; returns the exit code.
fn notify(args: &[String]) -> i32 {
    let Ok(sock) = std::env::var("NOTIFY_SOCKET") else {
        eprintln!("sdcompat-notify: NOTIFY_SOCKET not set");
        return 1;
    };
    let mut lines = Vec::new();
    for a in args {
        match a.as_str() {
            "--ready" => lines.push("READY=1".to_string()),
            "--stopping" => lines.push("STOPPING=1".to_string()),
            "--reloading" => lines.push("RELOADING=1".to_string()),
            s if s.starts_with("--status=") => lines.push(format!("STATUS={}", &s[9..])),
            s if s.starts_with("--pid=") => lines.push(format!("MAINPID={}", &s[6..])),
            s if s.contains('=') => lines.push(s.to_string()),
            s => {
                eprintln!("sdcompat-notify: unknown argument {s}");
                return 1;
            }
        }
    }
    let msg = lines.join("\n");
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as u16;
    let abstract_ns = sock.starts_with('@');
    for (i, b) in sock.bytes().enumerate().take(addr.sun_path.len() - 1) {
        addr.sun_path[i] = if abstract_ns && i == 0 { 0 } else { b as libc::c_char };
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + sock.len() + usize::from(!abstract_ns)) as u32;
    let r = unsafe {
        libc::sendto(
            fd,
            msg.as_ptr() as *const libc::c_void,
            msg.len(),
            0,
            &addr as *const _ as *const libc::sockaddr,
            len,
        )
    };
    if r < 0 {
        eprintln!("sdcompat-notify: {}", errno());
        return 1;
    }
    0
}
