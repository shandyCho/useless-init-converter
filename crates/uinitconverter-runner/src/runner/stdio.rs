//! StandardInput=/StandardOutput=/StandardError=: the journal stream, files and ttys.

use super::*;

impl Runner {
    pub(super) fn uses_tty(&self) -> bool {
        let input = self
            .unit
            .get("Service", "StandardInput")
            .unwrap_or_default();
        let output = self
            .unit
            .get("Service", "StandardOutput")
            .unwrap_or_default();
        input.starts_with("tty") || output == "tty"
    }

    pub(super) fn tty_path(&self) -> String {
        self.unit
            .get("Service", "TTYPath")
            .unwrap_or_else(|| "/dev/console".into())
    }

    /// Connect to journald's stdout stream (the protocol PID 1 uses). journald
    /// attributes the stream to the unit through our cgroup.
    pub(super) fn journal_stream(&self, cmd: &ExecCommand) -> Option<i32> {
        if !self.cfg.journal || self.unit.id == "systemd-journald.service" {
            return None;
        }
        let path_buf = self.cfg.journal_stdout.display().to_string();
        let path = path_buf.as_str();
        if !Path::new(path).exists() || path.len() >= 100 {
            return None;
        }
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return None;
        }
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as u16;
        for (i, b) in path.bytes().enumerate() {
            addr.sun_path[i] = b as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + path.len() + 1) as u32;
        if unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) } < 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        unsafe { libc::shutdown(fd, libc::SHUT_RD) };
        let ident = self
            .unit
            .get("Service", "SyslogIdentifier")
            .unwrap_or_else(|| {
                let a0 = cmd.argv.first().map(String::as_str).unwrap_or(&cmd.path);
                a0.rsplit('/').next().unwrap_or(a0).to_string()
            });
        let prio = self
            .unit
            .get("Service", "SyslogLevel")
            .map(|l| match l.as_str() {
                "emerg" => 0,
                "alert" => 1,
                "crit" => 2,
                "err" => 3,
                "warning" => 4,
                "notice" => 5,
                "debug" => 7,
                _ => 6,
            })
            .unwrap_or(6);
        let prefix = i32::from(self.unit.get_bool("Service", "SyslogLevelPrefix", true));
        let header = format!("{ident}\n{}\n{prio}\n{prefix}\n0\n0\n0\n", self.unit.id);
        let r = unsafe { libc::write(fd, header.as_ptr() as *const libc::c_void, header.len()) };
        if r != header.len() as isize {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(fd)
    }

    pub(super) fn setup_stdio(&self, cmd: &ExecCommand) -> Result<(), (i32, String)> {
        let input = self
            .unit
            .get("Service", "StandardInput")
            .unwrap_or_else(|| "null".into());
        let default_out = if input.starts_with("tty") || input == "socket" {
            "inherit"
        } else {
            "journal"
        };
        let output = self
            .unit
            .get("Service", "StandardOutput")
            .unwrap_or_else(|| default_out.into());
        let error = self
            .unit
            .get("Service", "StandardError")
            .unwrap_or_else(|| "inherit".into());

        // fd:NAME from systemd-run --pipe/--stdio (see daemon::transient).
        // Taken once: the store's copies would keep the caller's pipes open.
        let stdio = if [&input, &output, &error]
            .iter()
            .any(|s| s.starts_with("fd:"))
        {
            let store = self.cfg.run_dir.join("fdstore");
            let key = format!("{}#stdio", self.unit.id);
            let fds = crate::fdstore::get(&store, &key);
            crate::fdstore::del(&store, &key, None);
            fds
        } else {
            Vec::new()
        };
        let stdio_fd = |spec: &str| {
            use std::os::fd::AsRawFd;
            let name = spec.strip_prefix("fd:")?;
            stdio
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f.as_raw_fd())
        };

        let open = |path: &str, flags: i32| -> Result<i32, String> {
            let c = cstr(path);
            let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC, 0o644) };
            if fd < 0 {
                Err(format!("open {path}: {}", errno()))
            } else {
                Ok(fd)
            }
        };
        let place = |fd: i32, target: i32| unsafe {
            if fd != target {
                libc::dup2(fd, target);
                libc::close(fd);
            } else {
                let fl = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, fl & !libc::FD_CLOEXEC);
            }
        };

        let wants_journal =
            |x: &str| x.starts_with("journal") || x.starts_with("kmsg") || x == "syslog";
        let journal = if wants_journal(&output) || wants_journal(&error) {
            self.journal_stream(cmd)
        } else {
            None
        };
        let mut tty_fd: Option<i32> = None;
        if input.starts_with("tty") || output == "tty" || error == "tty" {
            tty_fd = Some(
                self.open_tty(input == "tty-force")
                    .map_err(|e| (EXIT_STDIN, e))?,
            );
        }

        match input.as_str() {
            "null" | "" => place(
                open("/dev/null", libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?,
                0,
            ),
            s if s.starts_with("tty") => unsafe {
                libc::dup2(tty_fd.unwrap(), 0);
            },
            s if s.starts_with("file:") => place(
                open(&s[5..], libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?,
                0,
            ),
            s if stdio_fd(s).is_some() => unsafe {
                libc::dup2(stdio_fd(s).unwrap(), 0);
            },
            "socket" if self.main_conn().is_some() => unsafe {
                libc::dup2(self.main_conn().unwrap(), 0);
            },
            _ => place(
                open("/dev/null", libc::O_RDONLY).map_err(|e| (EXIT_STDIN, e))?,
                0,
            ),
        }

        let redirect = |spec: &str, target: i32, code: i32| -> Result<(), (i32, String)> {
            match spec {
                "null" => place(
                    open("/dev/null", libc::O_WRONLY).map_err(|e| (code, e))?,
                    target,
                ),
                "tty" => unsafe {
                    libc::dup2(tty_fd.unwrap(), target);
                },
                "inherit" if input.starts_with("tty") && target == 1 => unsafe {
                    libc::dup2(0, 1);
                },
                "inherit" if input == "socket" && target == 1 && self.main_conn().is_some() => unsafe {
                    libc::dup2(0, 1);
                },
                "socket" => match self.main_conn() {
                    Some(fd) => unsafe {
                        libc::dup2(fd, target);
                    },
                    None => place(
                        open("/dev/null", libc::O_WRONLY).map_err(|e| (code, e))?,
                        target,
                    ),
                },
                s if s.starts_with("file:") => place(
                    open(&s[5..], libc::O_WRONLY | libc::O_CREAT).map_err(|e| (code, e))?,
                    target,
                ),
                s if s.starts_with("append:") => place(
                    open(&s[7..], libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND)
                        .map_err(|e| (code, e))?,
                    target,
                ),
                s if s.starts_with("truncate:") => place(
                    open(&s[9..], libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC)
                        .map_err(|e| (code, e))?,
                    target,
                ),
                s if stdio_fd(s).is_some() => unsafe {
                    libc::dup2(stdio_fd(s).unwrap(), target);
                },
                // journal, kmsg, syslog: journald's stream if available,
                // otherwise keep the s6 log pipe.
                s if s.starts_with("journal")
                    || s.starts_with("kmsg")
                    || s == "syslog"
                    || s == "inherit" =>
                {
                    if let Some(fd) = journal.filter(|_| s != "inherit") {
                        unsafe { libc::dup2(fd, target) };
                    }
                }
                _ => {}
            }
            Ok(())
        };
        redirect(&output, 1, EXIT_STDOUT)?;
        if error == "inherit" {
            unsafe { libc::dup2(1, 2) };
        } else {
            redirect(&error, 2, EXIT_STDOUT)?;
        }
        if let Some(fd) = tty_fd {
            if fd > 2 {
                unsafe { libc::close(fd) };
            }
        }
        if let Some(fd) = journal {
            unsafe { libc::close(fd) };
        }
        Ok(())
    }

    pub(super) fn open_tty(&self, force: bool) -> Result<i32, String> {
        let path = self.tty_path();
        let c = cstr(&path);
        unsafe {
            libc::setsid();
            if self.unit.get_bool("Service", "TTYVHangup", false) {
                let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
                if fd >= 0 {
                    let old = libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    libc::ioctl(fd, libc::TIOCSCTTY, 1);
                    libc::vhangup();
                    libc::signal(libc::SIGHUP, old);
                    libc::close(fd);
                }
            }
            let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if fd < 0 {
                return Err(format!("open {path}: {}", errno()));
            }
            if libc::ioctl(fd, libc::TIOCSCTTY, i32::from(force)) < 0 {
                libc::close(fd);
                return Err(format!("TIOCSCTTY {path}: {}", errno()));
            }
            Ok(fd)
        }
    }
}
