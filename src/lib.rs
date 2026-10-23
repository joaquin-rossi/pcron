use rand::RngCore;
use rand_distr::Distribution as RandDistribution;

pub mod file;

pub struct Tab {
    pub cmds: Vec<TabCmd>,
}

pub struct TabCmd {
    pub line: usize,
    /// the distribution as written in the tab, e.g. `exp(1/60)`
    pub source: String,
    /// the distribution with its arguments evaluated, e.g. `exp(0.016)`
    pub name: String,
    pub args: Vec<f64>,
    pub dist: Box<dyn DistFloat>,
    pub script: String,
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
