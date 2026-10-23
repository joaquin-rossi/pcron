use anyhow::Result;
use clap::Args;
use rand::{SeedableRng, rngs::StdRng};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

use crate::{DelayArgs, DelayBounds};
use pcron::{DistFloat, TabCmd, file};

const SAMPLES: usize = 10_000;
const SCRIPT_WIDTH: usize = 30;

#[derive(Args, Debug)]
pub struct ListArgs {
    #[command(flatten)]
    delay: DelayArgs,

    /// Print as JSON (with the full scripts and unrounded numbers)
    #[arg(long)]
    json: bool,
}

pub fn main_list(tab_path: &Path, args: &ListArgs) -> Result<()> {
    let bounds = DelayBounds::new_from_args(&args.delay)?;
    let tab = file::read(tab_path)?;
    let stats: Vec<_> = tab
        .cmds
        .iter()
        .map(|cmd| Stats::sample(cmd.dist.as_ref(), bounds))
        .collect();

    if args.json {
        print_json(&tab.cmds, &stats, bounds)
    } else {
        print_table(&tab.cmds, &stats);
        Ok(())
    }
}

#[derive(Serialize)]
struct JsonList<'a> {
    min_delay: f64,
    max_delay: f64,
    commands: Vec<JsonCmd<'a>>,
}

#[derive(Serialize)]
struct JsonCmd<'a> {
    line: usize,
    script: &'a str,
    definition: &'a str,
    distribution: &'a str,
    args: &'a [f64],
    env: BTreeMap<&'a str, &'a str>,
    /// in seconds
    delay: &'a Stats,
}

fn print_json(cmds: &[TabCmd], stats: &[Stats], bounds: DelayBounds) -> Result<()> {
    let list = JsonList {
        min_delay: bounds.min,
        max_delay: bounds.max,
        commands: cmds
            .iter()
            .zip(stats)
            .map(|(cmd, delay)| JsonCmd {
                line: cmd.line,
                script: &cmd.script,
                definition: &cmd.source,
                distribution: &cmd.name,
                args: &cmd.args,
                env: cmd
                    .env
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
                delay,
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&list)?);
    Ok(())
}

fn print_table(cmds: &[TabCmd], stats: &[Stats]) {
    let header = [
        "LINE",
        "SCRIPT",
        "DEFINITION",
        "COMPUTED",
        "MEAN",
        "STDDEV",
        "P5",
        "P50",
        "P95",
        "RUNS/DAY",
        "CLAMPED",
    ];
    let rows = cmds.iter().zip(stats).map(|(cmd, s)| row(cmd, s));
    let table: Vec<Vec<String>> = std::iter::once(header.map(String::from).to_vec())
        .chain(rows)
        .collect();

    // text columns go left, numeric ones right
    let left = [
        false, true, true, true, false, false, false, false, false, false, false,
    ];
    print!("{}", render(&table, &left));
}

fn row(cmd: &TabCmd, s: &Stats) -> Vec<String> {
    let args: Vec<_> = cmd.args.iter().map(|&a| fmt_num(a)).collect();
    vec![
        cmd.line.to_string(),
        script_preview(&cmd.script),
        cmd.source.clone(),
        format!("{}({})", cmd.name, args.join(", ")),
        fmt_duration(s.mean),
        fmt_duration(s.std_dev),
        fmt_duration(s.p5),
        fmt_duration(s.p50),
        fmt_duration(s.p95),
        if s.runs_per_day.is_finite() {
            fmt_num(s.runs_per_day)
        } else {
            "∞".to_string()
        },
        format!("{}%", fmt_num(s.clamped * 100.0)),
    ]
}

/// statistics of the actual delay (after applying the bounds), estimated by sampling
#[derive(Debug, Serialize)]
struct Stats {
    mean: f64,
    std_dev: f64,
    p5: f64,
    p50: f64,
    p95: f64,
    /// infinite (null in JSON) if the mean delay is 0
    runs_per_day: f64,
    /// fraction of samples that fell outside the bounds
    clamped: f64,
}

impl Stats {
    fn sample(dist: &dyn DistFloat, bounds: DelayBounds) -> Self {
        // fixed seed so that `list` always prints the same numbers
        let mut rng = StdRng::seed_from_u64(0);
        let mut clamped = 0;
        let mut xs: Vec<f64> = (0..SAMPLES)
            .map(|_| {
                let raw = dist.sample_with(&mut rng);
                let x = bounds.clamp(raw);
                if x != raw {
                    clamped += 1;
                }
                x
            })
            .collect();
        xs.sort_by(f64::total_cmp);

        let n = xs.len() as f64;
        let mean = xs.iter().sum::<f64>() / n;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let pct = |q: f64| xs[((n - 1.0) * q).round() as usize];

        Self {
            mean,
            std_dev: var.sqrt(),
            p5: pct(0.05),
            p50: pct(0.50),
            p95: pct(0.95),
            runs_per_day: 86400.0 / mean,
            clamped: clamped as f64 / n,
        }
    }
}

// first line of the script, cut to SCRIPT_WIDTH chars; `…` marks anything left out
fn script_preview(script: &str) -> String {
    let mut lines = script.lines();
    let first = lines.next().unwrap_or_default().trim();
    if first.chars().count() > SCRIPT_WIDTH {
        let cut: String = first.chars().take(SCRIPT_WIDTH - 1).collect();
        format!("{}…", cut.trim_end())
    } else if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

// at most 2 decimals, or 2 significant digits below 1 (0.0167 -> 0.017),
// without trailing zeros
fn fmt_num(x: f64) -> String {
    let decimals = if x != 0.0 && x.abs() < 1.0 {
        (1 - x.abs().log10().floor() as i32) as usize
    } else {
        2
    };
    let s = format!("{x:.decimals$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0" } else { s }.to_string()
}

// `12.5s`, `3m05s`, `2h10m`, `4d06h`
fn fmt_duration(secs: f64) -> String {
    if secs < 60.0 {
        return format!("{}s", fmt_num(secs));
    }
    // round to the smallest unit shown
    let s = secs.round() as u64;
    if s < 3600 {
        return format!("{}m{:02}s", s / 60, s % 60);
    }
    let m = (secs / 60.0).round() as u64;
    if m < 24 * 60 {
        return format!("{}h{:02}m", m / 60, m % 60);
    }
    let h = (secs / 3600.0).round() as u64;
    format!("{}d{:02}h", h / 24, h % 24)
}

fn render(table: &[Vec<String>], left: &[bool]) -> String {
    let width = |col: usize| {
        table
            .iter()
            .map(|row| row[col].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<_> = (0..left.len()).map(width).collect();

    let mut out = String::new();
    for row in table {
        let cells: Vec<_> = row
            .iter()
            .zip(&widths)
            .zip(left)
            .map(|((cell, &w), &left)| {
                if left {
                    format!("{cell:<w$}")
                } else {
                    format!("{cell:>w$}")
                }
            })
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(fmt_num(60.0), "60");
        assert_eq!(fmt_num(1.0 / 60.0), "0.017");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(1.2345), "1.23");
        assert_eq!(fmt_num(0.5), "0.5");
        assert_eq!(fmt_num(0.001), "0.001");
        assert_eq!(fmt_num(0.00012345), "0.00012");
        assert_eq!(fmt_num(0.0999), "0.1");
        assert_eq!(fmt_num(0.0), "0");
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(0.5), "0.5s");
        assert_eq!(fmt_duration(59.0), "59s");
        assert_eq!(fmt_duration(65.0), "1m05s");
        assert_eq!(fmt_duration(3600.0 * 2.0 + 600.0), "2h10m");
        assert_eq!(fmt_duration(86400.0 * 4.0 + 3600.0 * 6.0), "4d06h");
        assert_eq!(fmt_duration(5399.6), "1h30m");
        assert_eq!(fmt_duration(3599.6), "1h00m");
    }

    #[test]
    fn scripts() {
        assert_eq!(script_preview("echo hi"), "echo hi");
        assert_eq!(script_preview("echo a\n  echo b"), "echo a …");
        let long = "x".repeat(40);
        assert_eq!(script_preview(&long).chars().count(), SCRIPT_WIDTH);
        assert!(script_preview(&long).ends_with('…'));
    }

    #[test]
    fn stats() {
        let bounds = DelayBounds::new(1.0, 100.0).unwrap();

        let s = Stats::sample(
            file::build_dyn_dist("const", &[42.0]).unwrap().as_ref(),
            bounds,
        );
        assert_eq!(
            (s.mean, s.std_dev, s.p50, s.clamped),
            (42.0, 0.0, 42.0, 0.0)
        );

        let s = Stats::sample(
            file::build_dyn_dist("uniform", &[0.0, 10.0])
                .unwrap()
                .as_ref(),
            bounds,
        );
        assert!((s.clamped - 0.1).abs() < 0.01, "{s:?}");
        assert!((s.p50 - 5.0).abs() < 0.1, "{s:?}");
        assert!((s.p95 - 9.5).abs() < 0.1, "{s:?}");
    }
}
