use rand_distr::Distribution as RandDistribution;

mod parser;
pub use parser::parse_file;

pub struct Tab {
    pub cmds: Vec<TabCmd>,
}

pub struct TabCmd {
    pub dist: Box<dyn DistFloat>,
    pub shell: String,
}

pub trait DistFloat: Send + Sync {
    fn sample(&self) -> f32;
}

impl<D> DistFloat for D
where
    D: RandDistribution<f32> + Send + Sync,
{
    #[inline]
    fn sample(&self) -> f32 {
        RandDistribution::sample(self, &mut rand::rng())
    }
}
