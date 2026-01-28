use rand_distr::Distribution as RandDistribution;

mod parser;
pub use parser::parse_file;

#[derive(Debug)]
pub struct Tab {
    pub cmds: Vec<TabCmd>,
}

#[derive(Debug)]
pub struct TabCmd {
    pub dist: Distribution,
    pub shell: String,
}

#[derive(Debug)]
pub enum Distribution {
    Exp(rand_distr::Exp<f32>),
    Poisson(rand_distr::Poisson<f32>),
}

impl Distribution {
    pub fn exp(lambda: f32) -> Distribution {
        let d = rand_distr::Exp::new(1.0 / lambda).unwrap();
        Distribution::Exp(d)
    }

    pub fn poisson(lambda: f32) -> Distribution {
        let d = rand_distr::Poisson::new(lambda).unwrap();
        Distribution::Poisson(d)
    }

    pub fn sample(&self) -> f32 {
        let r = &mut rand::rng();

        use Distribution::*;
        match self {
            Exp(dist) => dist.sample(r),
            Poisson(dist) => dist.sample(r),
        }
    }
}
