//! uinitconverter-journal-relay: listens on journald's client sockets and
//! forwards what arrives to syslog, for systems that run without journald
//! (see relay.rs). Standalone: needs only a syslog daemon on /dev/log.

use std::path::PathBuf;
use std::process::exit;

mod relay;
mod scm;

const USAGE: &str = "\
usage: uinitconverter-journal-relay [--dir DIR] [--syslog PATH] [--ready-fd N]

  --dir DIR       where to create journald's sockets (default /run/systemd/journal)
  --syslog PATH   the syslog socket to forward to (default /dev/log)
  --ready-fd N    write a newline to fd N once listening (s6 readiness)
";

fn die(msg: &str) -> ! {
    eprintln!("uinitconverter-journal-relay: {msg}");
    exit(1)
}

fn main() {
    let mut dir = PathBuf::from("/run/systemd/journal");
    let mut syslog = PathBuf::from("/dev/log");
    let mut ready_fd = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| die(&format!("{a} needs a value")));
        match a.as_str() {
            "--dir" => dir = PathBuf::from(val()),
            "--syslog" => syslog = PathBuf::from(val()),
            "--ready-fd" => ready_fd = Some(val().parse().unwrap_or_else(|_| die("--ready-fd needs a number"))),
            "-h" | "--help" => {
                print!("{USAGE}");
                exit(0);
            }
            o => die(&format!("unknown option {o}")),
        }
    }
    exit(relay::serve_forever(&dir, &syslog, ready_fd))
}
