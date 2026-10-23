//! systemd unit file parser.
//!
//! Keeps every assignment in file order (fragment first, then drop-ins), so
//! list-valued settings can honour systemd's "empty assignment resets" rule.

use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct UnitFile {
    /// (section, key, value) in the order they were read.
    pub entries: Vec<(String, String, String)>,
}

impl UnitFile {
    pub fn parse_file(&mut self, path: &Path) -> io::Result<()> {
        let text = fs::read_to_string(path)?;
        self.parse_str(&text);
        Ok(())
    }

    pub fn parse_str(&mut self, text: &str) {
        let mut section = String::new();
        let mut pending: Option<String> = None;

        for raw in text.lines() {
            let line = raw.trim_start();
            if pending.is_some() && (line.starts_with('#') || line.starts_with(';')) {
                // Comments inside a continuation are skipped.
                continue;
            }
            let mut line = match pending.take() {
                Some(mut p) => {
                    p.push(' ');
                    p.push_str(line);
                    p
                }
                None => line.to_string(),
            };
            if line.ends_with('\\') && !line.ends_with("\\\\") {
                line.pop();
                pending = Some(line);
                continue;
            }
            self.parse_line(&mut section, &line);
        }
        if let Some(line) = pending {
            self.parse_line(&mut section, &line);
        }
    }

    fn parse_line(&mut self, section: &mut String, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            return;
        }
        if line.starts_with('[') && line.ends_with(']') {
            *section = line[1..line.len() - 1].trim().to_string();
            return;
        }
        if let Some((k, v)) = line.split_once('=') {
            self.entries
                .push((section.clone(), k.trim().to_string(), v.trim().to_string()));
        }
    }

    /// Last assignment of a scalar setting. An empty assignment resets to default.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(s, k, _)| s == section && k == key)
            .map(|(_, _, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// All assignments of a list setting, honouring empty-assignment resets.
    pub fn get_all(&self, section: &str, key: &str) -> Vec<&str> {
        let mut out = Vec::new();
        for (s, k, v) in &self.entries {
            if s == section && k == key {
                if v.is_empty() {
                    out.clear();
                } else {
                    out.push(v.as_str());
                }
            }
        }
        out
    }

    /// List setting split into whitespace-separated (optionally quoted) words.
    pub fn get_words(&self, section: &str, key: &str) -> Vec<String> {
        self.get_all(section, key)
            .into_iter()
            .flat_map(|v| split_words(v).unwrap_or_default())
            .collect()
    }

    pub fn get_bool(&self, section: &str, key: &str) -> Option<bool> {
        self.get(section, key).and_then(parse_bool)
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.entries.iter().any(|(s, _, _)| s == section)
    }

    pub fn keys_in(&self, section: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .entries
            .iter()
            .filter(|(s, _, _)| s == section)
            .map(|(_, k, _)| k.as_str())
            .collect();
        keys.dedup();
        keys
    }

    pub fn sections(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for (s, _, _) in &self.entries {
            if !out.contains(&s.as_str()) {
                out.push(s);
            }
        }
        out
    }
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" | "y" | "t" => Some(true),
        "0" | "no" | "false" | "off" | "n" | "f" => Some(false),
        _ => None,
    }
}

/// Split a value into words using systemd's quoting rules: whitespace separates
/// words, single and double quotes group, C-style escapes are processed outside
/// single quotes.
pub fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err(format!("unterminated double quote in: {s}")),
                        Some('"') => break,
                        Some('\\') => cunescape_one(&mut chars, &mut cur)?,
                        Some(c) => cur.push(c),
                    }
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err(format!("unterminated single quote in: {s}")),
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cunescape_one(&mut chars, &mut cur)?;
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

fn cunescape_one(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) -> Result<(), String> {
    let c = chars.next().ok_or("trailing backslash")?;
    match c {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        'a' => out.push('\x07'),
        'b' => out.push('\x08'),
        'f' => out.push('\x0c'),
        'v' => out.push('\x0b'),
        's' => out.push(' '),
        'x' => {
            let hex: String = (0..2).filter_map(|_| chars.next()).collect();
            let b = u8::from_str_radix(&hex, 16).map_err(|_| format!("bad \\x escape: {hex}"))?;
            out.push(b as char);
        }
        '0'..='7' => {
            let mut oct = String::from(c);
            for _ in 0..2 {
                if let Some(&d) = chars.peek() {
                    if ('0'..='7').contains(&d) {
                        oct.push(d);
                        chars.next();
                    }
                }
            }
            let b = u8::from_str_radix(&oct, 8).map_err(|_| "bad octal escape")?;
            out.push(b as char);
        }
        // Covers \\ \" \' \$ and a backslash before whitespace.
        c => out.push(c),
    }
    Ok(())
}

/// Parse a systemd time span ("90", "5s", "1min 30s", "infinity") into microseconds.
pub fn parse_timespan_usec(v: &str, default_unit_usec: u64) -> Option<u64> {
    let v = v.trim();
    if v == "infinity" {
        return Some(u64::MAX);
    }
    let mut total: u64 = 0;
    let mut rest = v;
    let mut any = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_end == 0 {
            return None;
        }
        let num: f64 = rest[..num_end].parse().ok()?;
        rest = rest[num_end..].trim_start();
        let unit_end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];
        let mult: f64 = match unit {
            "" => default_unit_usec as f64,
            "us" | "usec" | "µs" => 1.0,
            "ms" | "msec" => 1e3,
            "s" | "sec" | "second" | "seconds" => 1e6,
            "m" | "min" | "minute" | "minutes" => 60e6,
            "h" | "hr" | "hour" | "hours" => 3600e6,
            "d" | "day" | "days" => 86400e6,
            "w" | "week" | "weeks" => 604800e6,
            _ => return None,
        };
        total = total.saturating_add((num * mult) as u64);
        any = true;
    }
    any.then_some(total)
}

/// A Timeout*Sec= value. As in systemd, 0 disables the timeout like
/// "infinity" does (docker.service has TimeoutStartSec=0), so both give
/// u64::MAX.
pub fn parse_timeout_usec(v: &str) -> Option<u64> {
    parse_timespan_usec(v, 1_000_000).map(|u| if u == 0 { u64::MAX } else { u })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_reset() {
        let mut u = UnitFile::default();
        u.parse_str(
            "[Unit]\nDescription=Test \\\n  service\n# c\n[Service]\nEnvironment=A=1\nEnvironment=\nEnvironment=B=2 \"C=3 4\"\n",
        );
        assert_eq!(u.get("Unit", "Description"), Some("Test  service"));
        assert_eq!(u.get_words("Service", "Environment"), vec!["B=2", "C=3 4"]);
    }

    #[test]
    fn words() {
        assert_eq!(
            split_words(r#"/bin/echo "a b" 'c\d' e\x41"#).unwrap(),
            vec!["/bin/echo", "a b", "c\\d", "eA"]
        );
    }

    #[test]
    fn timespans() {
        assert_eq!(parse_timespan_usec("90", 1_000_000), Some(90_000_000));
        assert_eq!(parse_timespan_usec("1min 30s", 1_000_000), Some(90_000_000));
        assert_eq!(parse_timespan_usec("500ms", 1_000_000), Some(500_000));
        assert_eq!(parse_timeout_usec("0"), Some(u64::MAX));
        assert_eq!(parse_timeout_usec("infinity"), Some(u64::MAX));
        assert_eq!(parse_timeout_usec("90"), Some(90_000_000));
    }
}
