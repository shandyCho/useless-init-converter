//! Parser for systemd EnvironmentFile= / os-release style KEY=VALUE files.

use std::fs;
use std::io;
use std::path::Path;

pub fn load(path: &Path) -> io::Result<Vec<(String, String)>> {
    Ok(parse(&fs::read_to_string(path)?))
}

pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();

    loop {
        // Skip leading whitespace and blank lines.
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };
        if first == '#' || first == ';' {
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            continue;
        }

        let mut key = String::new();
        let mut ok = false;
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                ok = true;
                break;
            }
            if c == '\n' {
                break;
            }
            key.push(c);
            chars.next();
        }
        let mut key = key.trim().to_string();
        if let Some(k) = key.strip_prefix("export ") {
            key = k.trim().to_string();
        }
        if !ok || key.is_empty() {
            // Malformed line: drop it.
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            continue;
        }

        while matches!(chars.peek(), Some(' ') | Some('\t')) {
            chars.next();
        }

        let mut value = String::new();
        let mut trailing_ws = 0usize;
        while let Some(c) = chars.next() {
            match c {
                '\n' => break,
                '"' => {
                    trailing_ws = 0;
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => break,
                            '\\' => match chars.next() {
                                Some('\n') => {}
                                Some(e @ ('"' | '\\' | '`' | '$')) => value.push(e),
                                Some(e) => {
                                    value.push('\\');
                                    value.push(e);
                                }
                                None => {}
                            },
                            c => value.push(c),
                        }
                    }
                }
                '\'' => {
                    trailing_ws = 0;
                    for c in chars.by_ref() {
                        if c == '\'' {
                            break;
                        }
                        value.push(c);
                    }
                }
                '\\' => {
                    trailing_ws = 0;
                    match chars.next() {
                        Some('\n') | None => {}
                        Some(e) => value.push(e),
                    }
                }
                c if c == ' ' || c == '\t' => {
                    value.push(c);
                    trailing_ws += 1;
                }
                c => {
                    value.push(c);
                    trailing_ws = 0;
                }
            }
        }
        value.truncate(value.len() - trailing_ws);
        out.push((key, value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let v = parse("# c\nA=1\nexport B = \"two words\"\nC='x y'  \nD=a\\\nb\n");
        assert_eq!(
            v,
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "two words".into()),
                ("C".into(), "x y".into()),
                ("D".into(), "ab".into()),
            ]
        );
    }
}
