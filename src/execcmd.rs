//! ExecStart=-style command lines: prefixes, word splitting, $VAR expansion.

use crate::unit::split_words;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecCommand {
    pub path: String,
    /// Unexpanded argv (argv[0] included).
    pub argv: Vec<String>,
    /// '-' prefix: failure is ignored.
    pub ignore_failure: bool,
    /// '+', '!' or '!!': run with full privileges (no User=, no sandboxing).
    pub privileged: bool,
    /// ':' prefix: no environment variable substitution.
    pub no_env_expand: bool,
}

pub fn parse(value: &str) -> Result<Vec<ExecCommand>, String> {
    let words = split_words(value)?;
    let mut cmds = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for w in words {
        if w == ";" {
            if !cur.is_empty() {
                cmds.push(build(std::mem::take(&mut cur))?);
            }
        } else {
            cur.push(w);
        }
    }
    if !cur.is_empty() {
        cmds.push(build(cur)?);
    }
    Ok(cmds)
}

fn build(mut words: Vec<String>) -> Result<ExecCommand, String> {
    let mut cmd = ExecCommand::default();
    let first = words.remove(0);
    let mut argv0_from_next = false;
    let mut rest = first.as_str();
    loop {
        if let Some(r) = rest.strip_prefix("!!") {
            cmd.privileged = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('-') {
            cmd.ignore_failure = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('@') {
            argv0_from_next = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('+') {
            cmd.privileged = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('!') {
            cmd.privileged = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix(':') {
            cmd.no_env_expand = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('|') {
            rest = r;
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return Err("empty executable path".into());
    }
    cmd.path = rest.to_string();
    if argv0_from_next {
        if words.is_empty() {
            return Err("'@' prefix requires argv[0]".into());
        }
        cmd.argv = words;
    } else {
        cmd.argv = std::iter::once(cmd.path.clone()).chain(words).collect();
    }
    Ok(cmd)
}

/// Resolve a bare command name the way systemd does (fixed search path).
pub fn resolve_path(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    for dir in ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        let p = format!("{dir}/{path}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    path.to_string()
}

/// Expand $VAR and ${VAR} in argv. A word that is exactly "$VAR" is split on
/// whitespace; other references substitute as a single word.
pub fn expand_argv(argv: &[String], lookup: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    for w in argv {
        if let Some(name) = w.strip_prefix('$') {
            if !name.is_empty()
                && !name.starts_with('{')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                if let Some(v) = lookup(name) {
                    out.extend(v.split_whitespace().map(String::from));
                }
                continue;
            }
        }
        out.push(expand_str(w, lookup));
    }
    out
}

pub fn expand_str(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            if b[i + 1] == b'$' {
                out.push('$');
                i += 2;
                continue;
            }
            if b[i + 1] == b'{' {
                if let Some(end) = s[i + 2..].find('}') {
                    let name = &s[i + 2..i + 2 + end];
                    out.push_str(&lookup(name).unwrap_or_default());
                    i += 3 + end;
                    continue;
                }
            }
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > start {
                out.push_str(&lookup(&s[start..j]).unwrap_or_default());
                i = j;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        let c = &parse("-@/usr/bin/foo foo-argv0 --x").unwrap()[0];
        assert!(c.ignore_failure);
        assert_eq!(c.path, "/usr/bin/foo");
        assert_eq!(c.argv, vec!["foo-argv0", "--x"]);
        let c = &parse("+/bin/true").unwrap()[0];
        assert!(c.privileged);
    }

    #[test]
    fn expansion() {
        let env = |k: &str| match k {
            "OPTS" => Some("-a -b".to_string()),
            "ONE" => Some("x y".to_string()),
            _ => None,
        };
        let argv: Vec<String> = ["/bin/d", "$OPTS", "${ONE}", "pre${ONE}", "$MISSING", "$$"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            expand_argv(&argv, &env),
            vec!["/bin/d", "-a", "-b", "x y", "prex y", "$"]
        );
    }
}
