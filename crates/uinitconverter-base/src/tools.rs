//! The programs uinitconverter runs from other packages (s6, s6-rc,
//! util-linux, policycoreutils, ...), by name. TOOLS= in uinitconverter.conf
//! maps a name to another program, so a package can be replaced or installed
//! elsewhere without a rebuild: `TOOLS=s6-svc=/opt/s6/bin/s6-svc`. Names it
//! doesn't list are looked up in PATH as before.

use std::sync::RwLock;

static MAP: RwLock<Vec<(String, String)>> = RwLock::new(Vec::new());

/// Parse TOOLS= (NAME=PROGRAM pairs separated by blanks) and use it from
/// now on; a later call replaces the earlier table.
pub fn configure(spec: &str) -> Result<(), String> {
    let mut map = Vec::new();
    for pair in spec.split_whitespace() {
        match pair.split_once('=') {
            Some((name, prog)) if !name.is_empty() && !prog.is_empty() => {
                map.retain(|(n, _): &(String, String)| n != name);
                map.push((name.to_string(), prog.to_string()));
            }
            _ => return Err(format!("TOOLS: {pair}: not NAME=PROGRAM")),
        }
    }
    *MAP.write().unwrap() = map;
    Ok(())
}

/// The program to run for NAME.
pub fn path(name: &str) -> String {
    MAP.read()
        .unwrap()
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| name.to_string())
}

/// Whether NAME's program exists: TOOLS='s path, or found in PATH (and
/// /usr/bin, /usr/sbin, where an empty or odd PATH would miss it).
pub fn available(name: &str) -> bool {
    let prog = path(name);
    if prog.contains('/') {
        return std::path::Path::new(&prog).is_file();
    }
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .chain(["/usr/bin", "/usr/sbin"])
        .any(|d| std::path::Path::new(d).join(&prog).is_file())
}

/// The table is global: tests that change it take this lock.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_and_lookup() {
        let _l = TEST_LOCK.lock().unwrap();
        configure("s6-svc=/opt/s6/bin/s6-svc mount=/x/mount s6-svc=/y/s6-svc").unwrap();
        assert_eq!(path("s6-svc"), "/y/s6-svc");
        assert_eq!(path("mount"), "/x/mount");
        assert_eq!(path("umount"), "umount");
        assert!(configure("bad").is_err());
        assert!(configure("=x").is_err());
        configure("").unwrap();
        assert_eq!(path("mount"), "mount");
        assert!(available("sh"));
        configure("sh=/nonexistent/sh").unwrap();
        assert!(!available("sh"));
        configure("").unwrap();
    }
}
