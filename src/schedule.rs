//! when commands run, shared by the server and `pcron list --simulate`

use chrono::{DateTime, TimeDelta, Utc};
use rand::Rng;

use crate::TabCmd;

/// the next run of `cmd` after one at `from`: its delay is sampled (and clamped), and with
/// a window it only counts while the window is open
pub fn next_run(cmd: &TabCmd, from: DateTime<Utc>, rng: &mut dyn Rng) -> (f64, DateTime<Utc>) {
    let secs = cmd.settings.bounds.clamp(cmd.delay.sample_with(rng));
    let at = match &cmd.window {
        None => from + delta(secs),
        Some(window) => {
            let zone = cmd.settings.tz;
            zone.utc(window.advance(zone.naive(from), secs))
        }
    };
    (secs, at)
}

/// the first moment at or after `t` when `cmd` may run (with a window, when it is open)
pub fn open_at(cmd: &TabCmd, t: DateTime<Utc>) -> DateTime<Utc> {
    match &cmd.window {
        None => t,
        Some(window) => {
            let zone = cmd.settings.tz;
            let naive = zone.naive(t);
            if window.contains(naive) {
                t
            } else {
                zone.utc(window.advance(naive, 0.0))
            }
        }
    }
}

fn delta(secs: f64) -> TimeDelta {
    // the delay is at most PCRON_MAX_DELAY, which fits
    TimeDelta::microseconds((secs * 1e6) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file;
    use chrono::NaiveDate;

    fn cmd(tab: &str) -> TabCmd {
        file::parse(tab).unwrap().cmds.remove(0)
    }

    fn utc(day: u32, h: u32, m: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2024, 1, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
            .and_utc()
    }

    #[test]
    fn runs() {
        let rng = &mut rand::rng();
        assert_eq!(
            next_run(&cmd("90 $ x"), utc(5, 12, 0), rng),
            (90.0, utc(5, 12, 0) + delta(90.0))
        );

        // friday 17:40 in Buenos Aires (20:40 UTC) + 45 minutes of 9-18 there
        let c = cmd("PCRON_TZ=America/Argentina/Buenos_Aires\n45*60 @ mon-fri 9-18 $ x");
        assert_eq!(next_run(&c, utc(5, 20, 40), rng).1, utc(8, 12, 25));
        assert_eq!(
            open_at(&c, utc(6, 12, 0)),
            utc(8, 12, 0),
            "saturday waits for monday"
        );
        assert_eq!(open_at(&c, utc(8, 13, 0)), utc(8, 13, 0), "already open");
    }
}
