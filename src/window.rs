use chrono::{DateTime, Datelike, LocalResult, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use std::fmt;

/// the time zone of windows (PCRON_TZ)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Zone {
    #[default]
    Local,
    Tz(chrono_tz::Tz),
}

impl Zone {
    pub fn parse(name: &str) -> Result<Self, String> {
        name.parse().map(Zone::Tz).map_err(|_| {
            format!("unknown time zone `{name}` (use names like America/Argentina/Buenos_Aires)")
        })
    }

    /// the wall-clock time of `t` in this zone
    pub fn naive(self, t: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Zone::Local => t.with_timezone(&chrono::Local).naive_local(),
            Zone::Tz(tz) => t.with_timezone(&tz).naive_local(),
        }
    }

    /// when the wall clock of this zone shows `t`
    pub fn utc(self, t: NaiveDateTime) -> DateTime<Utc> {
        match self {
            Zone::Local => to_utc(&chrono::Local, t),
            Zone::Tz(tz) => to_utc(&tz, t),
        }
    }
}

// when the clocks go back, the first of the two moments; when they go forward (and `t`
// never happens), an hour later
fn to_utc<Z: TimeZone>(zone: &Z, t: NaiveDateTime) -> DateTime<Utc> {
    match zone.from_local_datetime(&t) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.to_utc(),
        LocalResult::None => zone
            .from_local_datetime(&(t + TimeDelta::hours(1)))
            .earliest()
            .map_or_else(|| t.and_utc(), |t| t.to_utc()),
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Zone::Local => f.write_str("local"),
            Zone::Tz(tz) => f.write_str(tz.name()),
        }
    }
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const DAY_MINUTES: u32 = 24 * 60;

/// when a command's delay counts: some days of the week, at some times of the day (local
/// time); days and times are checked separately, like cron fields
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// indexed from monday
    days: [bool; 7],
    /// [start, end) in minutes of the day, sorted and without overlaps
    minutes: Vec<(u32, u32)>,
    text: String,
}

impl Window {
    /// `mon-fri 9-18`, `9:30-12,14-18`, `sat,sun`, `fri-mon 22-6`, ...
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut days = None;
        let mut minutes = None;
        for part in text.split_whitespace() {
            if part.starts_with(|c: char| c.is_ascii_alphabetic()) {
                if days.is_some() {
                    return Err(format!("more than one list of days (`{part}`)"));
                }
                days = Some(parse_days(part)?);
            } else if part.starts_with(|c: char| c.is_ascii_digit()) {
                if minutes.is_some() {
                    return Err(format!("more than one list of hours (`{part}`)"));
                }
                minutes = Some(parse_hours(part)?);
            } else {
                return Err(format!(
                    "expected days (like mon-fri) or hours (like 9-18), found `{part}`"
                ));
            }
        }
        if days.is_none() && minutes.is_none() {
            return Err("the window is empty".to_string());
        }

        Ok(Self {
            days: days.unwrap_or([true; 7]),
            minutes: minutes.unwrap_or_else(|| vec![(0, DAY_MINUTES)]),
            text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        })
    }

    pub fn contains(&self, t: NaiveDateTime) -> bool {
        let secs = t.num_seconds_from_midnight();
        self.days[t.weekday().num_days_from_monday() as usize]
            && self
                .minutes
                .iter()
                .any(|&(s, e)| s * 60 <= secs && secs < e * 60)
    }

    pub fn active_secs_per_week(&self) -> f64 {
        let day: u32 = self.minutes.iter().map(|(s, e)| e - s).sum();
        let days = self.days.iter().filter(|&&d| d).count() as u32;
        f64::from(day * days * 60)
    }

    /// the moment after `start` when the window has been open for `active` seconds; with
    /// 0, the next moment it is open (`start` itself if it is open then)
    pub fn advance(&self, start: NaiveDateTime, active: f64) -> NaiveDateTime {
        let week = self.active_secs_per_week();
        let mut remaining = active.max(0.0);
        let mut t = start;

        // it repeats every week, so skip whole weeks, leaving 0 < remaining <= week
        let weeks = ((remaining / week).ceil() - 1.0).max(0.0);
        if weeks > 0.0 {
            remaining -= weeks * week;
            match TimeDelta::try_days((weeks * 7.0) as i64).and_then(|d| t.checked_add_signed(d)) {
                Some(later) => t = later,
                None => return NaiveDateTime::MAX,
            }
        }

        let mut date = t.date();
        loop {
            if self.days[date.weekday().num_days_from_monday() as usize] {
                let midnight = date.and_time(chrono::NaiveTime::MIN);
                for &(s, e) in &self.minutes {
                    let s = midnight + TimeDelta::minutes(s.into());
                    let e = midnight + TimeDelta::minutes(e.into());
                    if e <= t {
                        continue;
                    }
                    let from = s.max(t);
                    let open = (e - from).as_seconds_f64();
                    // strictly less, so that it never lands right on the (excluded) end
                    if remaining < open {
                        return from + TimeDelta::microseconds((remaining * 1e6) as i64);
                    }
                    remaining -= open;
                }
            }
            match date.succ_opt() {
                Some(next) => date = next,
                None => return NaiveDateTime::MAX,
            }
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn parse_days(text: &str) -> Result<[bool; 7], String> {
    let day = |s: &str| {
        DAYS.iter()
            .position(|d| d.eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("unknown day `{s}` (use {})", DAYS.join(", ")))
    };

    let mut days = [false; 7];
    for item in text.split(',') {
        let (a, b) = item.split_once('-').unwrap_or((item, item));
        let (a, b) = (day(a)?, day(b)?);
        // fri-mon wraps around the week
        let mut d = a;
        loop {
            days[d] = true;
            if d == b {
                break;
            }
            d = (d + 1) % 7;
        }
    }
    Ok(days)
}

fn parse_hours(text: &str) -> Result<Vec<(u32, u32)>, String> {
    let mut ranges = Vec::new();
    for item in text.split(',') {
        let (a, b) = item
            .split_once('-')
            .ok_or_else(|| format!("expected a range of hours like 9-18, found `{item}`"))?;
        let (a, b) = (parse_time(a)?, parse_time(b)?);
        if a == b {
            return Err(format!("`{item}` is an empty range"));
        }
        if a < b {
            ranges.push((a, b));
        } else {
            // 22-6 wraps around midnight
            ranges.push((a, DAY_MINUTES));
            if b > 0 {
                ranges.push((0, b));
            }
        }
    }

    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    Ok(merged)
}

// H or H:MM, from 0 to 24, in minutes of the day
fn parse_time(text: &str) -> Result<u32, String> {
    let err = || format!("`{text}` is not a time (use H or H:MM, from 0 to 24)");
    let number = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };

    let (h, m) = text.split_once(':').unwrap_or((text, "0"));
    let (h, m) = (number(h).ok_or_else(err)?, number(m).ok_or_else(err)?);
    if m >= 60 || h > 24 || (h == 24 && m > 0) {
        return Err(err());
    }
    Ok(h * 60 + m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    // 2024-01-05 is a friday
    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2024, 1, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn parsing() {
        let w = Window::parse("mon-fri  9:30-12,11-13,22-6").unwrap();
        assert_eq!(w.days, [true, true, true, true, true, false, false]);
        assert_eq!(w.minutes, [(0, 360), (570, 780), (1320, 1440)]);
        assert_eq!(w.to_string(), "mon-fri 9:30-12,11-13,22-6");

        let w = Window::parse("fri-mon").unwrap();
        assert_eq!(w.days, [true, false, false, false, true, true, true]);
        assert_eq!(w.minutes, [(0, 1440)]);

        for (text, err) in [
            ("funday", "unknown day `funday`"),
            ("9", "expected a range of hours"),
            ("9-9", "empty range"),
            ("9-25", "`25` is not a time"),
            ("9:60-10", "`9:60` is not a time"),
            ("mon tue", "more than one list of days"),
            ("*", "expected days"),
        ] {
            let e = Window::parse(text).unwrap_err();
            assert!(e.contains(err), "{text}: {e}");
        }
    }

    #[test]
    fn contains() {
        let w = Window::parse("mon-fri 9-18").unwrap();
        assert!(w.contains(at(5, 9, 0)));
        assert!(w.contains(at(5, 17, 59)));
        assert!(!w.contains(at(5, 18, 0)));
        assert!(!w.contains(at(6, 12, 0)), "saturday");

        // like cron, days and hours are separate: saturday 1:00 is not friday's night
        let w = Window::parse("fri 22-2").unwrap();
        assert!(w.contains(at(5, 23, 0)));
        assert!(w.contains(at(5, 1, 0)));
        assert!(!w.contains(at(6, 1, 0)));
    }

    #[test]
    fn zones() {
        let tz = Zone::parse("America/Argentina/Buenos_Aires").unwrap();
        let t = at(5, 12, 0);
        // UTC-3 all year
        assert_eq!(tz.utc(t), t.and_utc() + TimeDelta::hours(3));
        assert_eq!(tz.naive(tz.utc(t)), t);
        assert!(
            Zone::parse("Mars/Olympus_Mons")
                .unwrap_err()
                .contains("unknown time zone")
        );

        // 2024-03-10 02:30 does not happen in New York (clocks go from 2:00 to 3:00)
        let ny = Zone::parse("America/New_York").unwrap();
        let gap = NaiveDate::from_ymd_opt(2024, 3, 10)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        assert_eq!(ny.naive(ny.utc(gap)), gap + TimeDelta::hours(1));
    }

    #[test]
    fn advance_pauses_outside_the_window() {
        let w = Window::parse("mon-fri 9-18").unwrap();
        let min = 60.0;
        // friday 17:40 + 45 minutes: 20 today, the other 25 on monday
        assert_eq!(w.advance(at(5, 17, 40), 45.0 * min), at(8, 9, 25));
        // outside, the clock starts when it opens
        assert_eq!(w.advance(at(6, 3, 0), 10.0 * min), at(8, 9, 10));
        // with 0, the next open moment
        assert_eq!(w.advance(at(5, 12, 0), 0.0), at(5, 12, 0));
        assert_eq!(w.advance(at(5, 20, 0), 0.0), at(8, 9, 0));
        // exactly the rest of the day ends at the next opening, not at 18:00
        assert_eq!(w.advance(at(5, 17, 0), 60.0 * min), at(8, 9, 0));
        // whole weeks are skipped: 2 weeks and 1 hour of open time
        let week = w.active_secs_per_week();
        assert_eq!(week, 5.0 * 9.0 * 3600.0);
        assert_eq!(
            w.advance(at(8, 9, 0), 2.0 * week + 60.0 * min),
            at(22, 10, 0)
        );
    }
}
