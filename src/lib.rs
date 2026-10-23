use rand_distr::Distribution as RandDistribution;

pub mod file;

pub struct Tab {
    pub cmds: Vec<TabCmd>,
}

pub struct TabCmd {
    pub dist: Box<dyn DistFloat>,
    pub script: String,
}

pub trait DistFloat: Send + Sync {
    fn sample(&self) -> f64;
}

impl<D> DistFloat for D
where
    D: RandDistribution<f64> + Send + Sync,
{
    #[inline]
    fn sample(&self) -> f64 {
        RandDistribution::sample(self, &mut rand::rng())
    }
}
