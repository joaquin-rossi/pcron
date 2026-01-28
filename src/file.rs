pub mod lexer;
pub mod parser;

use crate::{DistFloat, Tab, TabCmd};

use std::{fs, io, path::Path};

pub fn read(path: impl AsRef<Path>) -> io::Result<Tab> {
    let file = fs::read_to_string(path)?;
    let tokens = lexer::tokenize(&file).unwrap();
    let ast = parser::parse(&tokens).unwrap();

    let cmds = ast
        .cmds
        .into_iter()
        .map(|cmd| {
            let dist = build_dyn_distf32(&cmd.expr.name, &cmd.expr.args)?;
            Ok(TabCmd {
                dist,
                script: cmd.script,
            })
        })
        .collect::<Result<Vec<_>, ()>>()
        .unwrap();

    Ok(Tab { cmds })
}

// - beta(alpha, beta)
// - cauchy(median, scale)
// - chi_squared(k)
// - exp(lambda)
// - exp1()
// - fisher_f(m, n)
// - frechet(location, scale, shape)
// - gamma(shape, scale)
// - gumbel(location, scale)
// - inverse_gaussian(mean, shape)
// - log_normal(mu, sigma)
// - normal(mean, std_dev)
// - normal_inverse_gaussian(alpha, beta)
// - pareto(scale, shape)
// - pert(min, max, mode, shape)
// - poisson(lambda)
// - skew_normal(location, scale, shape)
// - standard_normal()
// - standard_uniform()
// - student_t(nu)
// - triangular(min, max, mode)
// - uniform(a [, b])
// - weibull(scale, shape)
pub fn build_dyn_distf32(name: &str, a: &[f32]) -> Result<Box<dyn DistFloat>, ()> {
    let expect = |n: usize| if a.len() == n { Ok(()) } else { Err(()) };

    use rand_distr::{
        Beta, Cauchy, ChiSquared, Exp, Exp1, FisherF, Frechet, Gamma, Gumbel, InverseGaussian,
        LogNormal, Normal, NormalInverseGaussian, Pareto, Pert, Poisson, SkewNormal,
        StandardNormal, StandardUniform, StudentT, Triangular, Uniform, Weibull,
    };
    let d: Box<dyn DistFloat> = match name {
        "beta" => {
            expect(2)?;
            Box::new(Beta::new(a[0], a[1]).unwrap())
        }
        "cauchy" => {
            expect(2)?;
            Box::new(Cauchy::new(a[0], a[1]).unwrap())
        }
        "chi_squared" | "chisquared" | "chi2" => {
            expect(1)?;
            Box::new(ChiSquared::new(a[0]).unwrap())
        }
        "exp" | "exponential" => {
            expect(1)?;
            Box::new(Exp::new(a[0]).unwrap())
        }
        "exp1" => {
            expect(0)?;
            Box::new(Exp1)
        }
        "fisher_f" | "fisherf" | "f" => {
            expect(2)?;
            Box::new(FisherF::new(a[0], a[1]).unwrap())
        }
        "frechet" => {
            expect(3)?;
            Box::new(Frechet::new(a[0], a[1], a[2]).unwrap())
        }
        "gamma" => {
            expect(2)?;
            Box::new(Gamma::new(a[0], a[1]).unwrap())
        }
        "gumbel" => {
            expect(2)?;
            Box::new(Gumbel::new(a[0], a[1]).unwrap())
        }
        "inverse_gaussian" | "inversegaussian" | "wald" => {
            expect(2)?;
            Box::new(InverseGaussian::new(a[0], a[1]).unwrap())
        }
        "log_normal" | "lognormal" => {
            expect(2)?;
            Box::new(LogNormal::new(a[0], a[1]).unwrap())
        }
        "normal" => {
            expect(2)?;
            Box::new(Normal::new(a[0], a[1]).unwrap())
        }
        "normal_inverse_gaussian" | "normalinversegaussian" | "nig" => {
            expect(2)?;
            Box::new(NormalInverseGaussian::new(a[0], a[1]).unwrap())
        }
        "pareto" => {
            expect(2)?;
            Box::new(Pareto::new(a[0], a[1]).unwrap())
        }
        "pert" => {
            expect(4)?;
            Box::new(
                Pert::new(a[0], a[1])
                    .with_shape(a[2])
                    .with_mode(a[3])
                    .unwrap(),
            )
        }
        "poisson" => {
            expect(1)?;
            Box::new(Poisson::new(a[0]).unwrap())
        }
        "skew_normal" | "skewnormal" => {
            expect(3)?;
            Box::new(SkewNormal::new(a[0], a[1], a[2]).unwrap())
        }
        "standard_normal" | "std_normal" => {
            expect(0)?;
            Box::new(StandardNormal)
        }
        "standard_uniform" | "std_uniform" => {
            expect(0)?;
            Box::new(StandardUniform)
        }
        "student_t" | "studentt" | "t" => {
            expect(1)?;
            Box::new(StudentT::new(a[0]).unwrap())
        }
        "triangular" => {
            expect(3)?;
            Box::new(Triangular::new(a[0], a[1], a[2]).unwrap())
        }
        "uniform" => match a.len() {
            1 => Box::new(Uniform::new_inclusive(a[0], a[0]).unwrap()),
            2 => Box::new(Uniform::new_inclusive(a[0], a[1]).unwrap()),
            _ => return Err(()),
        },
        "weibull" => {
            expect(2)?;
            Box::new(Weibull::new(a[0], a[1]).unwrap())
        }
        _ => return Err(()),
    };

    Ok(d)
}
