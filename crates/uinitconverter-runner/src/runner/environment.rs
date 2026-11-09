//! The service's environment variables and its RuntimeDirectory= family.

use super::*;

/// RuntimeDirectory= and its family: (setting, base, variable).
pub(super) fn dirs() -> Vec<(&'static str, String, &'static str)> {
    vec![
        ("RuntimeDirectory", "/run".into(), "RUNTIME_DIRECTORY"),
        ("StateDirectory", "/var/lib".into(), "STATE_DIRECTORY"),
        ("CacheDirectory", "/var/cache".into(), "CACHE_DIRECTORY"),
        ("LogsDirectory", "/var/log".into(), "LOGS_DIRECTORY"),
        (
            "ConfigurationDirectory",
            "/etc".into(),
            "CONFIGURATION_DIRECTORY",
        ),
    ]
}

pub(super) fn env_set(env: &mut Vec<(String, String)>, k: &str, v: &str) {
    if let Some(e) = env.iter_mut().find(|(ek, _)| ek == k) {
        e.1 = v.to_string();
    } else {
        env.push((k.to_string(), v.to_string()));
    }
}

impl Runner {
    /// The manager's and the runner's own variables (systemd's our_env);
    /// the unit's settings go on top in unit_env().
    pub(super) fn build_env(&mut self) -> Result<(), String> {
        self.setenv("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin");
        if let Ok(vars) = crate::envfile::load(Path::new("/etc/locale.conf")) {
            for (k, v) in vars {
                self.setenv(&k, &v);
            }
        }
        if let Ok(vars) = crate::envfile::load(&self.cfg.run_dir.join("manager.env")) {
            for (k, v) in vars {
                self.setenv(&k, &v);
            }
        }
        if let Some(inv) = self.st.get(keys::INVOCATION) {
            self.setenv("INVOCATION_ID", &inv);
        }
        self.memory_pressure_env();
        if let Some(u) = &self.user {
            let (name, home, shell) = (u.name.clone(), u.home.clone(), u.shell.clone());
            self.setenv("USER", &name);
            self.setenv("LOGNAME", &name);
            self.setenv("HOME", &home);
            self.setenv("SHELL", &shell);
        }
        for (key, base, var) in dirs() {
            let dirs: Vec<String> = self
                .unit
                .get_words("Service", key)
                .iter()
                .map(|d| format!("{base}/{}", d.split(':').next().unwrap_or(d)))
                .collect();
            if !dirs.is_empty() {
                self.setenv(var, &dirs.join(":"));
            }
        }
        if self.uses_tty() && !self.env.iter().any(|(k, _)| k == "TERM") {
            let tty = self.tty_path();
            let term = if tty.starts_with("/dev/ttyS") || tty.starts_with("/dev/ttyAMA") {
                "vt220"
            } else {
                "linux"
            };
            self.setenv("TERM", term);
        }
        // A missing EnvironmentFile= fails the start here already.
        self.unit_env(&[])?;
        Ok(())
    }

    /// The environment of a command, in systemd's order (exec_invoke()):
    /// our_env, PassEnvironment=, Environment=, EnvironmentFile=, each
    /// overriding the ones before, then UnsetEnvironment=. The files are
    /// read for every command, as systemd does: an ExecStartPre= may write
    /// the file ExecStart= reads.
    pub(super) fn unit_env(
        &self,
        extra_env: &[(String, String)],
    ) -> Result<Vec<(String, String)>, String> {
        let mut env = self.env.clone();
        for (k, v) in extra_env {
            env_set(&mut env, k, v);
        }
        for k in self.unit.get_words("Service", "PassEnvironment") {
            if let Ok(v) = std::env::var(&k) {
                env_set(&mut env, &k, &v);
            }
        }
        for w in self.unit.get_words("Service", "Environment") {
            if let Some((k, v)) = w.split_once('=') {
                env_set(&mut env, k, v);
            }
        }
        for f in self.unit.get_all("Service", "EnvironmentFile") {
            let (optional, path) = match f.strip_prefix('-') {
                Some(p) => (true, p.trim().to_string()),
                None => (false, f.clone()),
            };
            match crate::envfile::load(Path::new(&path)) {
                Ok(vars) => {
                    for (k, v) in vars {
                        env_set(&mut env, &k, &v);
                    }
                }
                Err(_) if optional => {}
                Err(e) => return Err(format!("EnvironmentFile {path}: {e}")),
            }
        }
        for k in self.unit.get_words("Service", "UnsetEnvironment") {
            match k.split_once('=') {
                Some(_) => env.retain(|(ek, ev)| format!("{ek}={ev}") != k),
                None => env.retain(|(ek, _)| *ek != k),
            }
        }
        Ok(env)
    }

    pub(super) fn create_directories(&mut self) -> Result<(), String> {
        for (key, base, _) in dirs() {
            let mode = self
                .unit
                .get("Service", &format!("{key}Mode"))
                .and_then(|m| u32::from_str_radix(&m, 8).ok())
                .unwrap_or(0o755);
            for d in self.unit.get_words("Service", key) {
                let d = d.split(':').next().unwrap_or(&d).to_string();
                let path = PathBuf::from(&base).join(&d);
                fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                crate::selinux::restorecon(&path.display().to_string());
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
                if key != "ConfigurationDirectory" {
                    if let Some(u) = &self.user {
                        let c = cstr(&path.display().to_string());
                        unsafe { libc::chown(c.as_ptr(), u.uid, self.gid.unwrap_or(u.gid)) };
                    }
                }
                self.writable_dirs.push(path.display().to_string());
            }
        }
        self.sandbox.writable_dirs = self.writable_dirs.clone();
        Ok(())
    }

    pub(super) fn remove_runtime_dirs(&self) {
        let preserve = self
            .unit
            .get("Service", "RuntimeDirectoryPreserve")
            .unwrap_or_default();
        if preserve == "yes" || preserve == "restart" {
            return;
        }
        for d in self.unit.get_words("Service", "RuntimeDirectory") {
            let d = d.split(':').next().unwrap_or(&d).to_string();
            if !d.is_empty() && !d.contains("..") {
                let _ = fs::remove_dir_all(Path::new(&dirs()[0].1).join(d));
            }
        }
    }

    /// MEMORY_PRESSURE_WATCH=/WRITE= for the cgroup entered last. A User=
    /// service gets the file, so it can write its trigger, as under systemd.
    pub(super) fn memory_pressure_env(&mut self) {
        for (k, v) in cgroup::memory_pressure_env(&self.unit, self.cgroup_dir().as_deref()) {
            if k == "MEMORY_PRESSURE_WATCH" && v != "/dev/null" && self.is_root {
                if let Some(u) = self.user.as_ref().filter(|u| u.uid != 0) {
                    let p = cstr(&v);
                    unsafe { libc::chown(p.as_ptr(), u.uid, self.gid.unwrap_or(u.gid)) };
                }
            }
            self.setenv(&k, &v);
        }
    }
}
