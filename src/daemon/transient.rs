//! Transient .service/.timer/.path units (systemd-run): D-Bus properties are
//! turned into a unit file under TRANSIENT_DIR and started on demand.

use std::collections::HashMap;
use std::fs;

use zbus::zvariant::{OwnedValue, Value};

use super::Daemon;
use crate::state::{self, keys};

const UNIT_KEYS: &[&str] = &[
    "Description", "Documentation", "Requires", "Requisite", "Wants", "BindsTo", "PartOf",
    "Upholds", "Conflicts", "Before", "After", "OnFailure", "OnSuccess", "DefaultDependencies",
    "RefuseManualStart", "RefuseManualStop", "StopWhenUnneeded", "SourcePath",
    "RequiresMountsFor", "JoinsNamespaceOf",
];
const IGNORED: &[&str] = &["AddRef", "CollectMode", "PIDs", "PIDFDs", "JobTimeoutUSec"];

/// Quote one word for a unit file (no specifier expansion of literal '%').
fn quote(w: &str) -> String {
    let e = w.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('\n', "\\n");
    format!("\"{e}\"")
}

fn exec_line(path: &str, argv: &[String], ignore: bool, flags: &[String]) -> String {
    let mut prefix = String::new();
    if ignore || flags.iter().any(|f| f == "ignore-failure") {
        prefix.push('-');
    }
    if flags.iter().any(|f| f == "privileged") {
        prefix.push('+');
    }
    if flags.iter().any(|f| f == "no-env-expand") {
        prefix.push(':');
    }
    let argv0 = argv.first().cloned().unwrap_or_else(|| path.to_string());
    let mut s = String::new();
    if argv0 != path {
        prefix.push('@');
        s.push_str(&format!("{prefix}{} {}", quote(path), quote(&argv0)));
    } else {
        s.push_str(&format!("{prefix}{}", quote(path)));
    }
    for a in argv.iter().skip(1) {
        s.push(' ');
        s.push_str(&quote(a));
    }
    s
}

fn usec(v: u64) -> String {
    if v == u64::MAX {
        "infinity".into()
    } else {
        format!("{v}us")
    }
}

/// Render one property as unit-file lines ([section], key, value).
fn render(name: &str, v: &Value<'_>, is_timer: bool) -> Result<Vec<(&'static str, String, String)>, String> {
    let main_sec: &'static str = if is_timer { "Timer" } else { "Service" };
    let sec_for = |k: &str| -> &'static str { if UNIT_KEYS.contains(&k) || k.starts_with("Condition") || k.starts_with("Assert") { "Unit" } else { main_sec } };
    let mut out = Vec::new();

    // Exec* lines: a(sasb) or the newer a(sasas) (ExecStartEx and friends).
    if name.starts_with("Exec") {
        let key = name.trim_end_matches("Ex").to_string();
        if let Value::Array(a) = v {
            for item in a.iter() {
                let Value::Structure(st) = item else { continue };
                let f = st.fields();
                let path = <&str>::try_from(&f[0]).map_err(|e| e.to_string())?.to_string();
                let argv: Vec<String> = match &f[1] {
                    Value::Array(av) => av.iter().filter_map(|x| <&str>::try_from(x).ok().map(String::from)).collect(),
                    _ => Vec::new(),
                };
                let (ignore, flags) = match f.get(2) {
                    Some(Value::Bool(b)) => (*b, Vec::new()),
                    Some(Value::Array(fl)) => (false, fl.iter().filter_map(|x| <&str>::try_from(x).ok().map(String::from)).collect()),
                    _ => (false, Vec::new()),
                };
                out.push(("Service", key.clone(), exec_line(&path, &argv, ignore, &flags)));
            }
        }
        return Ok(out);
    }
    match name {
        n if n.starts_with("Standard") && n.ends_with("FileDescriptor") => {
            return Err("passing file descriptors (systemd-run --pipe/--stdio) is not supported by sdcompat".into());
        }
        "Environment" => {
            if let Value::Array(a) = v {
                let words: Vec<String> = a.iter().filter_map(|x| <&str>::try_from(x).ok().map(quote)).collect();
                if !words.is_empty() {
                    out.push(("Service", "Environment".into(), words.join(" ")));
                }
            }
        }
        "EnvironmentFiles" => {
            if let Value::Array(a) = v {
                for item in a.iter() {
                    if let Value::Structure(st) = item {
                        let f = st.fields();
                        let p = <&str>::try_from(&f[0]).unwrap_or("");
                        let opt = matches!(f.get(1), Some(Value::Bool(true)));
                        out.push(("Service", "EnvironmentFile".into(), format!("{}{p}", if opt { "-" } else { "" })));
                    }
                }
            }
        }
        "TimersCalendar" => {
            if let Value::Array(a) = v {
                for item in a.iter() {
                    if let Value::Structure(st) = item {
                        let f = st.fields();
                        let spec = <&str>::try_from(&f[1]).unwrap_or("");
                        out.push(("Timer", "OnCalendar".into(), spec.to_string()));
                    }
                }
            }
        }
        "TimersMonotonic" => {
            if let Value::Array(a) = v {
                for item in a.iter() {
                    if let Value::Structure(st) = item {
                        let f = st.fields();
                        let base = <&str>::try_from(&f[0]).unwrap_or("");
                        let t = <u64>::try_from(&f[1]).unwrap_or(0);
                        out.push(("Timer", base.replace("USec", "Sec"), usec(t)));
                    }
                }
            }
        }
        "CPUQuotaPerSecUSec" => {
            if let Ok(t) = <u64>::try_from(v) {
                if t != u64::MAX {
                    out.push(("Service", "CPUQuota".into(), format!("{}%", t / 10_000)));
                }
            }
        }
        _ => {
            let key = if name.ends_with("USec") {
                format!("{}Sec", name.trim_end_matches("USec"))
            } else {
                name.to_string()
            };
            let sec = sec_for(&key);
            let val = match v {
                Value::Bool(b) => if *b { "yes".into() } else { "no".into() },
                Value::Str(s) => s.to_string(),
                Value::U64(n) if name.ends_with("USec") => usec(*n),
                Value::U64(n) => if *n == u64::MAX { "infinity".into() } else { n.to_string() },
                Value::I64(n) => n.to_string(),
                Value::U32(n) => n.to_string(),
                Value::I32(n) => n.to_string(),
                Value::U16(n) => n.to_string(),
                Value::I16(n) => n.to_string(),
                Value::U8(n) => n.to_string(),
                Value::Array(a) => a
                    .iter()
                    .filter_map(|x| <&str>::try_from(x).ok().map(String::from))
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => {
                    eprintln!("sdcompatd: transient property {name} with an unsupported type, ignored");
                    return Ok(out);
                }
            };
            out.push((sec, key, val));
        }
    }
    Ok(out)
}

pub fn unit_file(name: &str, props: &[(String, OwnedValue)]) -> Result<String, String> {
    let is_timer = name.ends_with(".timer");
    let mut sections: Vec<(&'static str, Vec<String>)> = vec![("Unit", Vec::new()), (if is_timer { "Timer" } else { "Service" }, Vec::new())];
    for (k, v) in props {
        if IGNORED.contains(&k.as_str()) {
            continue;
        }
        for (sec, key, val) in render(k, v, is_timer)? {
            if let Some(s) = sections.iter_mut().find(|(n, _)| *n == sec) {
                s.1.push(format!("{key}={val}"));
            } else {
                sections.push((sec, vec![format!("{key}={val}")]));
            }
        }
    }
    let mut text = format!("# Transient unit created by sdcompat (StartTransientUnit)\n");
    for (sec, lines) in sections {
        if lines.is_empty() && sec == "Unit" {
            continue;
        }
        text.push_str(&format!("[{sec}]\n"));
        for l in lines {
            text.push_str(&l);
            text.push('\n');
        }
    }
    Ok(text)
}

impl Daemon {
    pub(super) fn is_transient_file(&self, unit: &crate::loader::Unit) -> bool {
        unit.fragment
            .as_ref()
            .map(|f| f.starts_with(&self.cfg.transient_dir))
            .unwrap_or(false)
    }

    /// Write the main unit and auxiliary units.
    pub(super) fn write_transient(
        &self,
        name: &str,
        props: &[(String, OwnedValue)],
        aux: &[(String, Vec<(String, OwnedValue)>)],
    ) -> Result<(), String> {
        fs::create_dir_all(&self.cfg.transient_dir).map_err(|e| e.to_string())?;
        let mut files = vec![(name.to_string(), unit_file(name, props)?)];
        for (n, p) in aux {
            files.push((n.clone(), unit_file(n, p)?));
        }
        for (n, text) in files {
            let path = self.cfg.transient_dir.join(&n);
            if path.exists() {
                return Err(format!("Unit {n} already exists."));
            }
            fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }

    pub(super) fn remove_transient_file(&self, id: &str) {
        let _ = fs::remove_file(self.cfg.transient_dir.join(id));
        let _ = fs::remove_dir_all(self.cfg.unit_run_dir(id));
    }

    /// Unload finished transient units and watch on-demand units for state
    /// changes (systemd-run --wait waits for PropertiesChanged).
    pub(super) async fn watch_dynamic(&self, last: &mut HashMap<String, (String, String)>) {
        let names: Vec<String> = self.dynamic.lock().unwrap().keys().cloned().collect();
        let active = self.active_atoms().await;
        for n in names {
            let (id, unit) = self.load_unit(&n);
            let st = self.unit_status(&id, &active, &unit).await;
            let cur = (st.active.to_string(), st.sub.to_string());
            if last.get(&id) != Some(&cur) {
                last.insert(id.clone(), cur.clone());
                let mut changed: HashMap<String, Value<'static>> = HashMap::new();
                changed.insert("ActiveState".into(), Value::from(cur.0.clone()));
                changed.insert("SubState".into(), Value::from(cur.1.clone()));
                self.emit(&super::unit_path(&id), "org.freedesktop.DBus.Properties", "PropertiesChanged",
                    &("org.freedesktop.systemd1.Unit", changed, Vec::<String>::new())).await;
                let mut svc: HashMap<String, Value<'static>> = HashMap::new();
                svc.insert("Result".into(), Value::from(st.result.clone()));
                svc.insert("MainPID".into(), Value::from(st.main_pid));
                self.emit(&super::unit_path(&id), "org.freedesktop.DBus.Properties", "PropertiesChanged",
                    &("org.freedesktop.systemd1.Service", svc, Vec::<String>::new())).await;
            }
            if !self.is_transient_file(&unit) {
                continue;
            }
            // CollectMode=inactive: unload after it has been inactive for a while,
            // so clients like `systemd-run --wait` can still read the results.
            let rs = state::for_unit(&self.cfg.run_dir, &id);
            let since = rs.get_u64(keys::INACTIVE_ENTER);
            let elapsed_timer = unit.utype() == "timer"
                && rs.has(crate::timer::ELAPSED)
                && !unit.get_bool("Timer", "RemainAfterElapse", true);
            let idle = cur.0 == "inactive" && since > 0 && crate::util::now_realtime_usec().saturating_sub(since) > 10_000_000;
            if idle || elapsed_timer {
                self.dyn_stop(&id, &unit).await;
                self.remove_transient_file(&id);
                last.remove(&id);
                self.emit(super::MANAGER_PATH, super::props::MANAGER_IFACE, "UnitRemoved",
                    &(id.clone(), super::op(super::unit_path(&id)))).await;
            }
        }
    }
}
