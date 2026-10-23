//! systemd calendar events (OnCalendar=, systemd.time(7)).
//!
//! Supported: weekday lists/ranges, `*`, lists, `a..b` ranges, `/step`
//! repetitions, `~` (days counted from the end of the month), optional date or
//! time part, optional seconds, the shorthands (daily, weekly, ...), and a
//! trailing `UTC` or IANA time zone.

use std::ffi::CString;

#[derive(Debug, Clone, PartialEq)]
struct Comp {
    start: u32,
    end: u32,
    /// 0 = every value in start..=end
    step: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Field(Option<Vec<Comp>>);

impl Field {
    fn any() -> Field {
        Field(None)
    }

    fn matches(&self, v: u32) -> bool {
        match &self.0 {
            None => true,
            Some(comps) => comps.iter().any(|c| {
                v >= c.start && v <= c.end && (c.step == 0 || (v - c.start) % c.step == 0)
            }),
        }
    }

    /// A day field after `~`, `v` counted from the end of the month (1 =
    /// the last day). An open repetition runs toward the end, as in
    /// systemd.time(7): `~07/1` is the last seven days, not the 7th-last day
    /// and every day before it.
    fn matches_from_end(&self, v: u32) -> bool {
        match &self.0 {
            None => true,
            Some(comps) => comps.iter().any(|c| {
                if c.step > 0 && c.end == u32::MAX {
                    v <= c.start && (c.start - v) % c.step == 0
                } else {
                    v >= c.start && v <= c.end && (c.step == 0 || (v - c.start) % c.step == 0)
                }
            }),
        }
    }

    fn values(&self, lo: u32, hi: u32) -> Vec<u32> {
        (lo..=hi).filter(|&v| self.matches(v)).collect()
    }

    fn parse(s: &str, max: u32) -> Result<Field, String> {
        if s == "*" {
            return Ok(Field::any());
        }
        let mut comps = Vec::new();
        for part in s.split(',') {
            let (range, step) = match part.split_once('/') {
                Some((r, st)) => (r, st.parse::<u32>().map_err(|_| format!("bad step in {s}"))?),
                None => (part, 0),
            };
            let num = |x: &str| x.parse::<u32>().map_err(|_| format!("bad number '{x}' in {s}"));
            // "a/step" and "*/step" are open-ended (rendered without an end).
            let (start, end) = if range == "*" {
                (0, if step > 0 { u32::MAX } else { max })
            } else if let Some((a, b)) = range.split_once("..") {
                (num(a)?, num(b)?)
            } else {
                let v = num(range)?;
                (v, if step > 0 { u32::MAX } else { v })
            };
            if start > end {
                return Err(format!("bad range in {s}"));
            }
            comps.push(Comp { start, end, step });
        }
        Ok(Field(Some(comps)))
    }

    fn render(&self, width: usize) -> String {
        match &self.0 {
            None => "*".into(),
            Some(comps) => comps
                .iter()
                .map(|c| {
                    let a = format!("{:0w$}", c.start, w = width);
                    if c.step > 0 {
                        if c.end == u32::MAX {
                            format!("{a}/{}", c.step)
                        } else {
                            format!("{a}..{:0w$}/{}", c.end, c.step, w = width)
                        }
                    } else if c.start != c.end {
                        format!("{a}..{:0w$}", c.end, w = width)
                    } else {
                        a
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarSpec {
    /// Bit 0 = Monday ... bit 6 = Sunday; None = any day.
    weekdays: Option<u8>,
    year: Field,
    month: Field,
    day: Field,
    /// Day field counts from the end of the month (`~`).
    end_of_month: bool,
    hour: Field,
    minute: Field,
    second: Field,
    /// None = local time; Some("UTC") or an IANA zone name.
    tz: Option<String>,
}

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn weekday_index(s: &str) -> Option<u8> {
    let l = s.to_ascii_lowercase();
    WEEKDAYS.iter().position(|w| l.starts_with(w)).map(|p| p as u8)
}

fn parse_weekdays(s: &str) -> Result<u8, String> {
    let mut mask = 0u8;
    for part in s.split(',') {
        if let Some((a, b)) = part.split_once("..") {
            let (a, b) = (
                weekday_index(a).ok_or_else(|| format!("bad weekday {a}"))?,
                weekday_index(b).ok_or_else(|| format!("bad weekday {b}"))?,
            );
            let mut i = a;
            loop {
                mask |= 1 << i;
                if i == b {
                    break;
                }
                i = (i + 1) % 7;
            }
        } else {
            mask |= 1 << weekday_index(part).ok_or_else(|| format!("bad weekday {part}"))?;
        }
    }
    Ok(mask)
}

fn is_timezone(s: &str) -> bool {
    s == "UTC" || s.contains('/') && std::path::Path::new("/usr/share/zoneinfo").join(s).is_file()
}

impl CalendarSpec {
    pub fn parse(input: &str) -> Result<CalendarSpec, String> {
        let s = input.trim();
        let (head, tail) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        let short = match head.to_ascii_lowercase().as_str() {
            "minutely" => "*-*-* *:*:00".to_string(),
            "hourly" => "*-*-* *:00:00".to_string(),
            "daily" => "*-*-* 00:00:00".to_string(),
            "monthly" => "*-*-01 00:00:00".to_string(),
            "weekly" => "Mon *-*-* 00:00:00".to_string(),
            "yearly" | "annually" => "*-01-01 00:00:00".to_string(),
            "quarterly" => "*-01,04,07,10-01 00:00:00".to_string(),
            "semiannually" => "*-01,07-01 00:00:00".to_string(),
            _ => String::new(),
        };
        let expanded = if short.is_empty() { s.to_string() } else { format!("{short} {tail}") };
        let mut tokens: Vec<&str> = expanded.split_whitespace().collect();
        if tokens.is_empty() {
            return Err("empty calendar specification".into());
        }
        let mut spec = CalendarSpec {
            weekdays: None,
            year: Field::any(),
            month: Field::any(),
            day: Field::any(),
            end_of_month: false,
            hour: Field(Some(vec![Comp { start: 0, end: 0, step: 0 }])),
            minute: Field(Some(vec![Comp { start: 0, end: 0, step: 0 }])),
            second: Field(Some(vec![Comp { start: 0, end: 0, step: 0 }])),
            tz: None,
        };
        if let Some(last) = tokens.last() {
            if tokens.len() > 1 && is_timezone(last) {
                spec.tz = Some(last.to_string());
                tokens.pop();
            }
        }
        let first = tokens[0];
        if first.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            spec.weekdays = Some(parse_weekdays(first)?);
            tokens.remove(0);
        }
        let mut have_date = false;
        let mut have_time = false;
        for tok in tokens {
            if tok.contains(':') {
                if have_time {
                    return Err(format!("duplicate time in {input}"));
                }
                have_time = true;
                let parts: Vec<&str> = tok.split(':').collect();
                let sec = |p: &str| p.split('.').next().unwrap_or(p).to_string();
                match parts.as_slice() {
                    [h, m] => {
                        spec.hour = Field::parse(h, 23)?;
                        spec.minute = Field::parse(m, 59)?;
                        spec.second = Field::parse("00", 59)?;
                    }
                    [h, m, s] => {
                        spec.hour = Field::parse(h, 23)?;
                        spec.minute = Field::parse(m, 59)?;
                        spec.second = Field::parse(&sec(s), 59)?;
                    }
                    _ => return Err(format!("bad time '{tok}'")),
                }
            } else if tok.contains('-') || tok.contains('~') || tok == "*" {
                if have_date {
                    return Err(format!("duplicate date in {input}"));
                }
                have_date = true;
                let (ym, day, eom) = match tok.rsplit_once('~') {
                    Some((l, d)) => (l, d, true),
                    None => match tok.rsplit_once('-') {
                        Some((l, d)) => (l, d, false),
                        None => ("*", tok, false),
                    },
                };
                spec.end_of_month = eom;
                spec.day = Field::parse(day, 31)?;
                match ym.split_once('-') {
                    Some((y, m)) => {
                        spec.year = Field::parse(y, 9999)?;
                        spec.month = Field::parse(m, 12)?;
                    }
                    None => spec.month = Field::parse(ym, 12)?,
                }
            } else {
                return Err(format!("cannot parse '{tok}' in {input}"));
            }
        }
        if !have_date && !have_time && spec.weekdays.is_none() {
            return Err(format!("cannot parse {input}"));
        }
        Ok(spec)
    }

    /// Normalized form, as `systemd-analyze calendar` prints it.
    pub fn render(&self) -> String {
        let mut s = String::new();
        if let Some(mask) = self.weekdays {
            let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            let days: Vec<&str> = (0..7).filter(|i| mask & (1 << i) != 0).map(|i| names[i]).collect();
            s.push_str(&days.join(","));
            s.push(' ');
        }
        s.push_str(&format!(
            "{}-{}{}{} {}:{}:{}",
            self.year.render(4),
            self.month.render(2),
            if self.end_of_month { "~" } else { "-" },
            self.day.render(2),
            self.hour.render(2),
            self.minute.render(2),
            self.second.render(2)
        ));
        if let Some(tz) = &self.tz {
            s.push(' ');
            s.push_str(tz);
        }
        s
    }

    /// True if next_after() switches TZ (a named zone other than UTC): it
    /// sets an environment variable, which only a single-threaded process
    /// may do (the timer runner, not the daemon).
    pub fn switches_tz(&self) -> bool {
        self.tz.as_deref().is_some_and(|z| z != "UTC")
    }

    /// First elapse strictly after `after` (seconds since the epoch). See
    /// switches_tz() before calling it from a multi-threaded process.
    pub fn next_after(&self, after: i64) -> Option<i64> {
        let _tz = TzGuard::set(self.tz.as_deref());
        let utc = self.tz.as_deref() == Some("UTC");
        let start = after + 1;
        let now = to_tm(start, utc);
        let hours = self.hour.values(0, 23);
        let minutes = self.minute.values(0, 59);
        let seconds = self.second.values(0, 59);
        if hours.is_empty() || minutes.is_empty() || seconds.is_empty() {
            return None;
        }
        let (mut y, mut m, mut d) = ((now.tm_year + 1900) as u32, (now.tm_mon + 1) as u32, now.tm_mday as u32);

        // Skip whole years and months that cannot match, so far-future dates are cheap.
        for _ in 0..200_000 {
            if y > 9999 {
                return None;
            }
            if !self.year.matches(y) {
                y += 1;
                (m, d) = (1, 1);
                continue;
            }
            if !self.month.matches(m) || d > days_in_month(y, m) {
                m += 1;
                d = 1;
                if m > 12 {
                    y += 1;
                    m = 1;
                }
                continue;
            }
            let dim = days_in_month(y, m);
            let day_ok = if self.end_of_month { self.day.matches_from_end(dim - d + 1) } else { self.day.matches(d) };
            let wd_ok = self
                .weekdays
                .map(|mask| mask & (1 << weekday(y, m, d)) != 0)
                .unwrap_or(true);
            if day_ok && wd_ok {
                for &h in &hours {
                    for &mi in &minutes {
                        for &s in &seconds {
                            let mut t: libc::tm = unsafe { std::mem::zeroed() };
                            t.tm_year = y as i32 - 1900;
                            t.tm_mon = m as i32 - 1;
                            t.tm_mday = d as i32;
                            t.tm_hour = h as i32;
                            t.tm_min = mi as i32;
                            t.tm_sec = s as i32;
                            t.tm_isdst = -1;
                            let ts = from_tm(&mut t, utc);
                            // Skip times that do not exist (DST gaps normalize away).
                            if t.tm_hour as u32 != h || t.tm_min as u32 != mi {
                                continue;
                            }
                            if ts >= start {
                                return Some(ts);
                            }
                        }
                    }
                }
            }
            d += 1;
        }
        None
    }
}

/// Monday = 0 ... Sunday = 6
fn weekday(y: u32, m: u32, d: u32) -> u32 {
    const T: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let sunday0 = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d) % 7;
    (sunday0 + 6) % 7
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn to_tm(t: i64, utc: bool) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    unsafe {
        if utc {
            libc::gmtime_r(&tt, &mut tm);
        } else {
            libc::localtime_r(&tt, &mut tm);
        }
    }
    tm
}

fn from_tm(tm: &mut libc::tm, utc: bool) -> i64 {
    unsafe {
        if utc {
            libc::timegm(tm) as i64
        } else {
            libc::mktime(tm) as i64
        }
    }
}

/// Temporarily switch TZ for a named zone (the timer process is single-threaded).
struct TzGuard {
    old: Option<Option<String>>,
}

impl TzGuard {
    fn set(tz: Option<&str>) -> TzGuard {
        match tz {
            Some(z) if z != "UTC" => {
                let old = std::env::var("TZ").ok();
                std::env::set_var("TZ", z);
                unsafe { tzset() };
                TzGuard { old: Some(old) }
            }
            _ => TzGuard { old: None },
        }
    }
}

impl Drop for TzGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            match old {
                Some(v) => std::env::set_var("TZ", v),
                None => std::env::remove_var("TZ"),
            }
            unsafe { tzset() };
        }
    }
}

extern "C" {
    fn tzset();
}

/// Make sure `CString` stays referenced for platforms that need it.
#[allow(dead_code)]
fn _unused(_: CString) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> CalendarSpec {
        CalendarSpec::parse(&format!("{s} UTC")).unwrap()
    }

    // 2026-10-18 12:34:56 UTC (a Sunday)
    const NOW: i64 = 1792326896;

    fn fmt(t: i64) -> String {
        let tm = to_tm(t, true);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }

    #[test]
    fn shorthands() {
        assert_eq!(fmt(utc("daily").next_after(NOW).unwrap()), "2026-10-19 00:00:00");
        assert_eq!(fmt(utc("hourly").next_after(NOW).unwrap()), "2026-10-18 13:00:00");
        assert_eq!(fmt(utc("weekly").next_after(NOW).unwrap()), "2026-10-19 00:00:00");
        assert_eq!(fmt(utc("monthly").next_after(NOW).unwrap()), "2026-11-01 00:00:00");
        assert_eq!(fmt(utc("yearly").next_after(NOW).unwrap()), "2027-01-01 00:00:00");
        assert_eq!(fmt(utc("minutely").next_after(NOW).unwrap()), "2026-10-18 12:35:00");
    }

    #[test]
    fn expressions() {
        assert_eq!(fmt(utc("*:0/15").next_after(NOW).unwrap()), "2026-10-18 12:45:00");
        assert_eq!(fmt(utc("Mon..Fri 09:00").next_after(NOW).unwrap()), "2026-10-19 09:00:00");
        assert_eq!(fmt(utc("Sat,Sun 20:00").next_after(NOW).unwrap()), "2026-10-18 20:00:00");
        assert_eq!(fmt(utc("*-*-01,15 03:30:00").next_after(NOW).unwrap()), "2026-11-01 03:30:00");
        assert_eq!(fmt(utc("*-02~01").next_after(NOW).unwrap()), "2027-02-28 00:00:00");
        // The last Monday in May (systemd.time(7)).
        assert_eq!(fmt(utc("Mon *-05~07/1").next_after(NOW).unwrap()), "2027-05-31 00:00:00");
        assert_eq!(fmt(utc("2027-03-04 05:06:07").next_after(NOW).unwrap()), "2027-03-04 05:06:07");
        assert_eq!(fmt(utc("*-*-* 12:34:56").next_after(NOW).unwrap()), "2026-10-19 12:34:56");
        assert!(utc("2020-01-01").next_after(NOW).is_none());
        assert_eq!(fmt(utc("2099-01-01 00:00:00").next_after(NOW).unwrap()), "2099-01-01 00:00:00");
        assert_eq!(fmt(utc("Fri *-*-13").next_after(NOW).unwrap()), "2026-11-13 00:00:00");
    }

    #[test]
    fn render() {
        assert_eq!(CalendarSpec::parse("daily").unwrap().render(), "*-*-* 00:00:00");
        assert_eq!(CalendarSpec::parse("Mon..Fri *:0/15").unwrap().render(), "Mon,Tue,Wed,Thu,Fri *-*-* *:00/15:00");
    }

    #[test]
    fn errors() {
        assert!(CalendarSpec::parse("bogus").is_err());
        assert!(CalendarSpec::parse("25:00:00").is_ok()); // parses; never matches hour 25
    }
}
