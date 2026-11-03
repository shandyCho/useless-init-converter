//! A user manager's report to systemd-oomd (work_history part 144), as
//! systemd --user makes it: the cgroups whose unit says
//! ManagedOOMMemoryPressure=kill or ManagedOOMSwap=kill, sent with
//! io.systemd.oom.ReportManagedOOMCGroups to oomd's socket. Fedora's
//! user slice.d/10-oomd-per-slice-defaults.conf sets it for every user
//! slice, so without this report oomd watches none of the session.
//!
//! The message is what systemd 262 sends (captured in the VM): one-way,
//! NUL-terminated JSON; `limit` is the percentage scaled to UINT32_MAX,
//! `duration` microseconds, both left out when unset. oomd keeps what it
//! was told until it hears `"mode":"auto"` for a path, so a cgroup that
//! goes away (or stops asking) is reported once more as auto. A restarted
//! oomd forgets everything: the whole list is sent again on reconnect.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::loader::Loader;

const OOMD_SOCKET: &str = "/run/systemd/oom/io.systemd.ManagedOOM";

/// One report entry (the mode is always kill; auto is only sent for removals).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    property: &'static str,
    limit: Option<u32>,
    duration: Option<u64>,
}

/// cgroup path from the root ("/user.slice/...") and property -> entry.
pub(super) type Report = BTreeMap<(String, &'static str), Entry>;

/// "80%", "80.5%", "805‰", "8050‱" as a fraction of UINT32_MAX, as
/// systemd's permyriad_to_uint32_scale.
fn scaled_limit(v: &str) -> Option<u32> {
    let v = v.trim();
    let (num, per) = if let Some(n) = v.strip_suffix('%') {
        (n, 100.0)
    } else if let Some(n) = v.strip_suffix('‰') {
        (n, 1000.0)
    } else if let Some(n) = v.strip_suffix('‱') {
        (n, 10000.0)
    } else {
        return None;
    };
    let f: f64 = num.trim().parse().ok()?;
    if !(0.0..=per).contains(&f) {
        return None;
    }
    let permyriad = (f * 10000.0 / per).round() as u64;
    Some((permyriad * u32::MAX as u64 / 10000) as u32)
}

/// The units' cgroups under `base` (the manager's delegated cgroup, `-.slice`
/// itself) and what they ask of oomd.
pub(super) fn scan(cfg: &Config, base: &Path) -> Report {
    scan_under(cfg, base, Path::new(crate::cgroup::CGROUP_ROOT))
}

fn scan_under(cfg: &Config, base: &Path, root: &Path) -> Report {
    let ld = Loader::new(cfg);
    let mut out = Report::new();
    let mut dirs: Vec<(String, PathBuf)> = vec![("-.slice".into(), base.to_path_buf())];
    let mut i = 0;
    while i < dirs.len() {
        let dir = dirs[i].1.clone();
        i += 1;
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_unit = [".slice", ".service", ".scope"]
                .iter()
                .any(|s| name.ends_with(s));
            if is_unit && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                // Only slices hold other units' cgroups.
                dirs.push((name, e.path()));
            }
        }
    }
    for (name, dir) in dirs {
        let unit = ld.load_implicit(&name);
        let section = match name.rsplit('.').next() {
            Some("slice") => "Slice",
            Some("service") => "Service",
            Some("scope") => "Scope",
            _ => continue,
        };
        let Ok(rel) = dir.strip_prefix(root) else {
            continue;
        };
        let path = format!("/{}", rel.display());
        if unit.file.get(section, "ManagedOOMMemoryPressure") == Some("kill") {
            let limit = unit
                .file
                .get(section, "ManagedOOMMemoryPressureLimit")
                .and_then(scaled_limit);
            let duration = unit
                .file
                .get(section, "ManagedOOMMemoryPressureDurationSec")
                .and_then(|v| crate::unit::parse_timespan_usec(v, 1_000_000));
            out.insert(
                (path.clone(), "ManagedOOMMemoryPressure"),
                Entry {
                    property: "ManagedOOMMemoryPressure",
                    limit,
                    duration,
                },
            );
        }
        if unit.file.get(section, "ManagedOOMSwap") == Some("kill") {
            out.insert(
                (path, "ManagedOOMSwap"),
                Entry {
                    property: "ManagedOOMSwap",
                    limit: None,
                    duration: None,
                },
            );
        }
    }
    out
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The call for `now`, with `"mode":"auto"` for what `before` had and
/// `now` lacks. None when there is nothing to say.
pub(super) fn message(now: &Report, before: &Report) -> Option<String> {
    let mut items = Vec::new();
    for ((path, _), e) in now {
        let mut o = format!(
            "{{\"mode\":\"kill\",\"path\":{},\"property\":\"{}\"",
            json_str(path),
            e.property
        );
        if let Some(l) = e.limit {
            o.push_str(&format!(",\"limit\":{l}"));
        }
        if let Some(d) = e.duration {
            o.push_str(&format!(",\"duration\":{d}"));
        }
        o.push('}');
        items.push(o);
    }
    for ((path, prop), _) in before {
        if !now.contains_key(&(path.clone(), prop)) {
            items.push(format!(
                "{{\"mode\":\"auto\",\"path\":{},\"property\":\"{prop}\"}}",
                json_str(path)
            ));
        }
    }
    if items.is_empty() {
        return None;
    }
    Some(format!(
        "{{\"method\":\"io.systemd.oom.ReportManagedOOMCGroups\",\"parameters\":{{\"cgroups\":[{}]}},\"oneway\":true}}\0",
        items.join(",")
    ))
}

/// The connection and what oomd was last told.
#[derive(Default)]
pub(super) struct Reporter {
    conn: Option<UnixStream>,
    sent: Report,
}

impl Reporter {
    fn socket() -> PathBuf {
        std::env::var_os("UINITCONVERTER_OOMD_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(OOMD_SOCKET))
    }

    /// oomd closed the connection (it restarted).
    fn closed(&self) -> bool {
        use std::os::fd::AsRawFd;
        let Some(c) = &self.conn else { return true };
        let mut b = [0u8; 1];
        let r = unsafe {
            libc::recv(
                c.as_raw_fd(),
                b.as_mut_ptr() as *mut libc::c_void,
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        r == 0
            || (r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock)
    }

    /// Report `now` if it changed or oomd is new. Quiet when oomd is not
    /// running: the next tick tries again.
    pub(super) fn update(&mut self, now: Report) {
        if self.closed() {
            self.conn = None;
            self.sent.clear();
            match UnixStream::connect(Self::socket()) {
                Ok(c) => {
                    let _ = c.set_nonblocking(true);
                    self.conn = Some(c);
                }
                Err(_) => return,
            }
        }
        if now == self.sent {
            return;
        }
        let Some(msg) = message(&now, &self.sent) else {
            self.sent = now;
            return;
        };
        let ok = self
            .conn
            .as_mut()
            .map(|c| c.write_all(msg.as_bytes()).is_ok())
            .unwrap_or(false);
        if ok {
            self.sent = now;
        } else {
            eprintln!("initconvertd: systemd-oomd: the report could not be sent; again later");
            self.conn = None;
            self.sent.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_scale_as_systemd() {
        // The value systemd 262 sent for 80%.
        assert_eq!(scaled_limit("80%"), Some(3435973836));
        assert_eq!(scaled_limit("800‰"), Some(3435973836));
        assert_eq!(scaled_limit("100%"), Some(u32::MAX));
        assert_eq!(scaled_limit("0%"), Some(0));
        assert_eq!(scaled_limit("120%"), None);
        assert_eq!(scaled_limit("80"), None);
    }

    /// A delegated tree with app.slice and a nested implicit slice: the
    /// slice.d drop-in reaches -.slice and the slice that has no unit
    /// file; a removed slice is reported as auto.
    #[test]
    fn scan_and_message() {
        let top = std::env::temp_dir().join(format!("uc-oom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let units = top.join("units");
        std::fs::create_dir_all(units.join("slice.d")).unwrap();
        std::fs::write(
            units.join("slice.d/10-oomd.conf"),
            "[Slice]\nManagedOOMMemoryPressure=kill\nManagedOOMMemoryPressureLimit=80%\n",
        )
        .unwrap();
        std::fs::write(units.join("app.slice"), "[Slice]\n").unwrap();
        std::fs::write(
            units.join("swappy.service"),
            "[Service]\nExecStart=/bin/true\nManagedOOMSwap=kill\nManagedOOMMemoryPressureDurationSec=30s\n",
        )
        .unwrap();
        // The delegated tree under a fake cgroup root (`top`).
        let base = top.join("user@1000.service");
        let nested = base.join("app.slice/app-gnome\\x2dsession\\x2dmanager.slice");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(base.join("app.slice/swappy.service")).unwrap();
        std::fs::create_dir_all(base.join("init.scope")).unwrap();
        let mut cfg = Config::defaults();
        cfg.unit_path = vec![units.clone()];
        let r = scan_under(&cfg, &base, &top);
        let keys: Vec<String> = r.keys().map(|(p, prop)| format!("{p} {prop}")).collect();
        assert_eq!(
            keys,
            vec![
                "/user@1000.service ManagedOOMMemoryPressure",
                "/user@1000.service/app.slice ManagedOOMMemoryPressure",
                "/user@1000.service/app.slice/app-gnome\\x2dsession\\x2dmanager.slice ManagedOOMMemoryPressure",
                "/user@1000.service/app.slice/swappy.service ManagedOOMSwap",
            ]
        );
        let m = message(&r, &Report::new()).unwrap();
        assert!(m.starts_with("{\"method\":\"io.systemd.oom.ReportManagedOOMCGroups\""));
        assert!(m.ends_with("\"oneway\":true}\0"));
        assert!(m.contains("\"path\":\"/user@1000.service/app.slice/app-gnome\\\\x2dsession\\\\x2dmanager.slice\",\"property\":\"ManagedOOMMemoryPressure\",\"limit\":3435973836}"));
        assert!(m.contains("{\"mode\":\"kill\",\"path\":\"/user@1000.service/app.slice/swappy.service\",\"property\":\"ManagedOOMSwap\"}"));
        // The nested slice goes away: reported as auto, the rest again.
        std::fs::remove_dir(&nested).unwrap();
        let r2 = scan_under(&cfg, &base, &top);
        let m2 = message(&r2, &r).unwrap();
        assert!(m2.contains("{\"mode\":\"auto\",\"path\":\"/user@1000.service/app.slice/app-gnome\\\\x2dsession\\\\x2dmanager.slice\",\"property\":\"ManagedOOMMemoryPressure\"}"));
        assert_eq!(message(&Report::new(), &Report::new()), None);
        let _ = std::fs::remove_dir_all(&top);
    }
}
