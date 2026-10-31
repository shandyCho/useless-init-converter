//! systemd calendar events (OnCalendar=, systemd.time(7)).
//!
//! Supported: weekday lists/ranges, `*`, lists, `a..b` ranges, `/step`
//! repetitions, `~` (days counted from the end of the month), optional date or
//! time part, optional seconds, the shorthands (daily, weekly, ...), and a
//! trailing `UTC` or IANA time zone.

use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
use jiff::{Timestamp, Zoned};

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
                Some((r, st)) => (
                    r,
                    st.parse::<u32>().map_err(|_| format!("bad step in {s}"))?,
                ),
                None => (part, 0),
            };
            let num = |x: &str| {
                x.parse::<u32>()
                    .map_err(|_| format!("bad number '{x}' in {s}"))
            };
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
    WEEKDAYS
        .iter()
        .position(|w| l.starts_with(w))
        .map(|p| p as u8)
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
    s == "UTC"
        || s.contains('/')
            && std::path::Path::new("/usr/share/zoneinfo")
                .join(s)
                .is_file()
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
        let expanded = if short.is_empty() {
            s.to_string()
        } else {
            format!("{short} {tail}")
        };
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
            hour: Field(Some(vec![Comp {
                start: 0,
                end: 0,
                step: 0,
            }])),
            minute: Field(Some(vec![Comp {
                start: 0,
                end: 0,
                step: 0,
            }])),
            second: Field(Some(vec![Comp {
                start: 0,
                end: 0,
                step: 0,
            }])),
            tz: None,
        };
        if let Some(last) = tokens.last() {
            if tokens.len() > 1 && is_timezone(last) {
                spec.tz = Some(last.to_string());
                tokens.pop();
            }
        }
        let first = tokens[0];
        if first
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        {
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
            let days: Vec<&str> = (0..7)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| names[i])
                .collect();
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

    /// The zone the spec is written in: the one it names, or the system's.
    fn time_zone(&self) -> Option<TimeZone> {
        match self.tz.as_deref() {
            Some("UTC") => Some(TimeZone::UTC),
            Some(name) => TimeZone::get(name).ok(),
            None => Some(TimeZone::system()),
        }
    }

    /// First elapse strictly after `after` (seconds since the epoch).
    pub fn next_after(&self, after: i64) -> Option<i64> {
        let tz = self.time_zone()?;
        let start = after + 1;
        let now = Timestamp::from_second(start).ok()?.to_zoned(tz.clone());
        let hours = self.hour.values(0, 23);
        let minutes = self.minute.values(0, 59);
        let seconds = self.second.values(0, 59);
        if hours.is_empty() || minutes.is_empty() || seconds.is_empty() {
            return None;
        }
        let (mut y, mut m, mut d) = (now.year() as u32, now.month() as u32, now.day() as u32);

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
            if !self.month.matches(m) || d > month_start(y, m).days_in_month() as u32 {
                m += 1;
                d = 1;
                if m > 12 {
                    y += 1;
                    m = 1;
                }
                continue;
            }
            let date = month_start(y, m);
            let dim = date.days_in_month() as u32;
            let day_ok = if self.end_of_month {
                self.day.matches_from_end(dim - d + 1)
            } else {
                self.day.matches(d)
            };
            let wd_ok = self
                .weekdays
                .map(|mask| mask & (1 << weekday(date, d)) != 0)
                .unwrap_or(true);
            if day_ok && wd_ok {
                for &h in &hours {
                    for &mi in &minutes {
                        for &s in &seconds {
                            let wall = DateTime::new(
                                y as i16, m as i8, d as i8, h as i8, mi as i8, s as i8, 0,
                            )
                            .ok()?;
                            if let Some(ts) = next_at_wall(&tz, wall, &now) {
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

/// The first of the month; only for what depends on the month as a whole.
/// Valid for every date next_after() reaches: its year is checked first.
fn month_start(y: u32, m: u32) -> Date {
    Date::new(y as i16, m as i8, 1).expect("year up to 9999, month 1 to 12")
}

/// Monday = 0 ... Sunday = 6, for day `d` of the month that starts at `first`.
fn weekday(first: Date, d: u32) -> u32 {
    (first.weekday().to_monday_zero_offset() as u32 + d - 1) % 7
}

/// Seconds since the epoch of local time `wall`, if that isn't before `from`.
///
/// As systemd: a time that a DST change skips never elapses. One that happens
/// twice elapses the first time, or the second time if `from` is already past
/// the change; so it doesn't elapse twice when `from` is inside the first pass.
fn next_at_wall(tz: &TimeZone, wall: DateTime, from: &Zoned) -> Option<i64> {
    let start = from.timestamp().as_second();
    let at = |offset: Offset| Some(offset.to_timestamp(wall).ok()?.as_second());
    let ts = match tz.to_ambiguous_timestamp(wall).offset() {
        AmbiguousOffset::Unambiguous { offset } => at(offset)?,
        AmbiguousOffset::Gap { .. } => return None,
        AmbiguousOffset::Fold { before, after } => {
            let first = at(before)?;
            if first >= start || from.offset() != after {
                first
            } else {
                at(after)?
            }
        }
    };
    (ts >= start).then_some(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> CalendarSpec {
        CalendarSpec::parse(&format!("{s} UTC")).unwrap()
    }

    // 2026-10-18 12:34:56 UTC (a Sunday)
    const NOW: i64 = 1792326896;

    fn fmt(t: i64) -> String {
        Timestamp::from_second(t)
            .unwrap()
            .to_zoned(TimeZone::UTC)
            .strftime("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    #[test]
    fn shorthands() {
        assert_eq!(
            fmt(utc("daily").next_after(NOW).unwrap()),
            "2026-10-19 00:00:00"
        );
        assert_eq!(
            fmt(utc("hourly").next_after(NOW).unwrap()),
            "2026-10-18 13:00:00"
        );
        assert_eq!(
            fmt(utc("weekly").next_after(NOW).unwrap()),
            "2026-10-19 00:00:00"
        );
        assert_eq!(
            fmt(utc("monthly").next_after(NOW).unwrap()),
            "2026-11-01 00:00:00"
        );
        assert_eq!(
            fmt(utc("yearly").next_after(NOW).unwrap()),
            "2027-01-01 00:00:00"
        );
        assert_eq!(
            fmt(utc("minutely").next_after(NOW).unwrap()),
            "2026-10-18 12:35:00"
        );
    }

    #[test]
    fn expressions() {
        assert_eq!(
            fmt(utc("*:0/15").next_after(NOW).unwrap()),
            "2026-10-18 12:45:00"
        );
        assert_eq!(
            fmt(utc("Mon..Fri 09:00").next_after(NOW).unwrap()),
            "2026-10-19 09:00:00"
        );
        assert_eq!(
            fmt(utc("Sat,Sun 20:00").next_after(NOW).unwrap()),
            "2026-10-18 20:00:00"
        );
        assert_eq!(
            fmt(utc("*-*-01,15 03:30:00").next_after(NOW).unwrap()),
            "2026-11-01 03:30:00"
        );
        assert_eq!(
            fmt(utc("*-02~01").next_after(NOW).unwrap()),
            "2027-02-28 00:00:00"
        );
        // The last Monday in May (systemd.time(7)).
        assert_eq!(
            fmt(utc("Mon *-05~07/1").next_after(NOW).unwrap()),
            "2027-05-31 00:00:00"
        );
        assert_eq!(
            fmt(utc("2027-03-04 05:06:07").next_after(NOW).unwrap()),
            "2027-03-04 05:06:07"
        );
        assert_eq!(
            fmt(utc("*-*-* 12:34:56").next_after(NOW).unwrap()),
            "2026-10-19 12:34:56"
        );
        assert!(utc("2020-01-01").next_after(NOW).is_none());
        assert_eq!(
            fmt(utc("2099-01-01 00:00:00").next_after(NOW).unwrap()),
            "2099-01-01 00:00:00"
        );
        assert_eq!(
            fmt(utc("Fri *-*-13").next_after(NOW).unwrap()),
            "2026-11-13 00:00:00"
        );
    }

    /// Expected values are what `systemd-analyze calendar --base-time` printed
    /// (systemd 262), so these follow systemd, not this code.
    #[test]
    fn dst_changes_as_systemd() {
        let next = |spec: &str, base: i64| CalendarSpec::parse(spec).unwrap().next_after(base);
        let ny = "America/New_York";

        // 2026-11-01, New York: 01:00 to 02:00 happens twice. 01:30 EDT is
        // 1793511000, 01:30 EST is 1793514600.
        // Before the first pass: the first.
        assert_eq!(
            next(&format!("*-*-* 01:30:00 {ny}"), 1793509800),
            Some(1793511000)
        );
        // Inside the first pass, after it: not again, the next day.
        assert_eq!(
            next(&format!("*-*-* 01:30:00 {ny}"), 1793511600),
            Some(1793601000)
        );
        // Inside the second pass: the second.
        assert_eq!(
            next(&format!("*-*-* 01:30:00 {ny}"), 1793512800),
            Some(1793514600)
        );
        // Inside the second pass, a minutely timer goes on minute by minute.
        assert_eq!(
            next(&format!("minutely {ny}"), 1793512800),
            Some(1793512860)
        );

        // 2026-03-08, New York: 02:30 doesn't exist, so the next day's.
        assert_eq!(
            next(&format!("*-*-* 02:30:00 {ny}"), 1772938800),
            Some(1773037800)
        );

        // Apia skipped 2011-12-30, a Friday: nothing runs on it, nor on the
        // Saturday mktime() would move it to.
        assert_eq!(
            next("Mon..Fri 09:00 Pacific/Apia", 1325203200),
            Some(1325444400)
        );
    }

    /// A named zone must not need TZ switched: the daemon calls this from its threads.
    #[test]
    fn named_zone_from_threads() {
        let spec = CalendarSpec::parse("daily Asia/Seoul").unwrap();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| assert_eq!(spec.next_after(NOW), Some(1792335600)));
            }
        });
    }

    #[test]
    fn render() {
        assert_eq!(
            CalendarSpec::parse("daily").unwrap().render(),
            "*-*-* 00:00:00"
        );
        assert_eq!(
            CalendarSpec::parse("Mon..Fri *:0/15").unwrap().render(),
            "Mon,Tue,Wed,Thu,Fri *-*-* *:00/15:00"
        );
    }

    #[test]
    fn errors() {
        assert!(CalendarSpec::parse("bogus").is_err());
        assert!(CalendarSpec::parse("25:00:00").is_ok()); // parses; never matches hour 25
    }
}
