use rand::RngCore;
use rand_distr::Distribution as RandDistribution;
use std::{fmt, time::Duration};

pub mod file;
pub mod schedule;
pub mod window;

use file::parser::BinOp;
use window::{Window, Zone};

pub struct Tab {
    pub cmds: Vec<TabCmd>,
}

pub struct TabCmd {
    pub line: usize,
    pub name: Option<String>,
    /// the delay as written in the tab, e.g. `3600 + exp(1/600)`
    pub source: String,
    /// the delay with its constant parts evaluated, e.g. `3600 + exp(0.0017)`
    pub delay: Delay,
    /// the delay only counts while the window is open
    pub window: Option<Window>,
    /// the `NAME=value` lines above it; the ones that are not PCRON_ settings are set for
    /// the script, on top of the server's environment
    pub env: Vec<(String, String)>,
    pub settings: Settings,
    pub script: String,
}

/// the PCRON_ variables of the tab, which can change from one command to the next
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub bounds: DelayBounds,
    /// how long a script may run before it is asked to stop
    pub timeout: Option<Duration>,
    /// how long a script gets to stop (after SIGTERM) before it is killed
    pub shutdown_timeout: Duration,
    /// of the window
    pub tz: Zone,
    /// a script to run when the command fails
    pub on_failure: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bounds: DelayBounds {
                min: 1.0,
                max: 365.0 * 24.0 * 3600.0,
            },
            timeout: None,
            shutdown_timeout: Duration::from_secs(60),
            tz: Zone::Local,
            on_failure: None,
        }
    }
}

/// every sampled delay is clamped to these, in seconds
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayBounds {
    pub min: f64,
    pub max: f64,
}

impl DelayBounds {
    pub fn new(min: f64, max: f64) -> Result<Self, String> {
        if !(min.is_finite() && min >= 0.0) {
            return Err(format!(
                "PCRON_MIN_DELAY must be a non-negative number, got {min}"
            ));
        }
        if !(max >= min && Duration::try_from_secs_f64(max).is_ok()) {
            return Err(format!(
                "PCRON_MAX_DELAY must be a finite number >= PCRON_MIN_DELAY ({min}), got {max}"
            ));
        }
        Ok(Self { min, max })
    }

    pub fn clamp(self, secs: f64) -> f64 {
        // clamp lets NaN through
        if secs.is_nan() {
            self.max
        } else {
            secs.clamp(self.min, self.max)
        }
    }
}

/// a random delay: arithmetic over constants and distributions
pub enum Delay {
    Const(f64),
    Dist {
        name: String,
        args: Vec<f64>,
        dist: Box<dyn DistFloat>,
    },
    Neg(Box<Delay>),
    Bin(BinOp, Box<Delay>, Box<Delay>),
}

impl Delay {
    pub fn sample_with(&self, rng: &mut dyn RngCore) -> f64 {
        match self {
            Delay::Const(v) => *v,
            Delay::Dist { dist, .. } => dist.sample_with(rng),
            Delay::Neg(d) => -d.sample_with(rng),
            Delay::Bin(op, l, r) => op.apply(l.sample_with(rng), r.sample_with(rng)),
        }
    }

    pub fn sample(&self) -> f64 {
        self.sample_with(&mut rand::rng())
    }

    /// with `num` formatting every number
    pub fn display_with(&self, num: &dyn Fn(f64) -> String) -> String {
        // parenthesize nested binary operations so the output is unambiguous
        let operand = |d: &Delay| match d {
            Delay::Bin(..) => format!("({})", d.display_with(num)),
            _ => d.display_with(num),
        };
        match self {
            Delay::Const(v) => num(*v),
            Delay::Dist { name, args, .. } => {
                let args: Vec<_> = args.iter().map(|&a| num(a)).collect();
                format!("{name}({})", args.join(", "))
            }
            Delay::Neg(d) => format!("-{}", operand(d)),
            Delay::Bin(op, l, r) => format!("{} {op} {}", operand(l), operand(r)),
        }
    }
}

impl fmt::Display for Delay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `+ 0.0` turns -0 into 0
        f.write_str(&self.display_with(&|n| (n + 0.0).to_string()))
    }
}

pub trait DistFloat: Send + Sync {
    fn sample_with(&self, rng: &mut dyn RngCore) -> f64;

    fn sample(&self) -> f64 {
        self.sample_with(&mut rand::rng())
    }
}

impl<D> DistFloat for D
where
    D: RandDistribution<f64> + Send + Sync,
{
    #[inline]
    fn sample_with(&self, rng: &mut dyn RngCore) -> f64 {
        RandDistribution::sample(self, rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_bounded() {
        let b = DelayBounds::new(1.0, 60.0).unwrap();
        assert_eq!(b.clamp(-5.0), 1.0);
        assert_eq!(b.clamp(0.0), 1.0);
        assert_eq!(b.clamp(30.0), 30.0);
        assert_eq!(b.clamp(f64::INFINITY), 60.0);
        assert_eq!(b.clamp(f64::NAN), 60.0);
    }

    #[test]
    fn invalid_bounds() {
        assert!(DelayBounds::new(-1.0, 10.0).is_err());
        assert!(DelayBounds::new(10.0, 1.0).is_err());
        assert!(DelayBounds::new(0.0, f64::INFINITY).is_err());
        assert!(DelayBounds::new(0.0, 1e30).is_err());
        assert!(DelayBounds::new(f64::NAN, 1.0).is_err());
    }
}
