use rand::RngCore;
use rand_distr::Distribution as RandDistribution;
use std::fmt;

pub mod file;
pub mod window;

use file::parser::BinOp;
use window::Window;

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
    /// set for the script, on top of the server's environment
    pub env: Vec<(String, String)>,
    pub script: String,
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
