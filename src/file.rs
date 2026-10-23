pub mod lexer;
pub mod parser;

use crate::{DistFloat, Tab, TabCmd};

use anyhow::{Context, Result, anyhow, bail};
use std::{fs, path::Path};

pub fn read(path: impl AsRef<Path>) -> Result<Tab> {
    let path = path.as_ref();
    let file =
        fs::read_to_string(path).with_context(|| format!("cannot read tab {}", path.display()))?;
    parse(&file).with_context(|| format!("invalid tab {}", path.display()))
}

pub fn parse(text: &str) -> Result<Tab> {
    let tokens = lexer::tokenize(text).map_err(|e| anyhow!(e))?;
    let ast = parser::parse(&tokens).map_err(|e| anyhow!(e))?;

    let cmds = ast
        .cmds
        .into_iter()
        .map(|cmd| {
            let dist = build_dyn_distf32(&cmd.expr.name, &cmd.expr.args)
                .with_context(|| format!("in `{}`", cmd.expr))?;
            Ok(TabCmd {
                dist,
                script: cmd.script,
            })
        })
        .collect::<Result<Vec<_>>>()?;

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
pub fn build_dyn_distf32(name: &str, a: &[f32]) -> Result<Box<dyn DistFloat>> {
    let expect = |n: usize| {
        if a.len() == n {
            Ok(())
        } else {
            Err(anyhow!("`{name}` expects {n} argument(s), got {}", a.len()))
        }
    };

    use rand_distr::{
        Beta, Cauchy, ChiSquared, Exp, Exp1, FisherF, Frechet, Gamma, Gumbel, InverseGaussian,
        LogNormal, Normal, NormalInverseGaussian, Pareto, Pert, Poisson, SkewNormal,
        StandardNormal, StandardUniform, StudentT, Triangular, Uniform, Weibull,
    };
    let d: Box<dyn DistFloat> = match name {
        "beta" => {
            expect(2)?;
            Box::new(Beta::new(a[0], a[1])?)
        }
        "cauchy" => {
            expect(2)?;
            Box::new(Cauchy::new(a[0], a[1])?)
        }
        "chi_squared" | "chisquared" | "chi2" => {
            expect(1)?;
            Box::new(ChiSquared::new(a[0])?)
        }
        "exp" | "exponential" => {
            expect(1)?;
            Box::new(Exp::new(a[0])?)
        }
        "exp1" => {
            expect(0)?;
            Box::new(Exp1)
        }
        "fisher_f" | "fisherf" | "f" => {
            expect(2)?;
            Box::new(FisherF::new(a[0], a[1])?)
        }
        "frechet" => {
            expect(3)?;
            Box::new(Frechet::new(a[0], a[1], a[2])?)
        }
        "gamma" => {
            expect(2)?;
            Box::new(Gamma::new(a[0], a[1])?)
        }
        "gumbel" => {
            expect(2)?;
            Box::new(Gumbel::new(a[0], a[1])?)
        }
        "inverse_gaussian" | "inversegaussian" | "wald" => {
            expect(2)?;
            Box::new(InverseGaussian::new(a[0], a[1])?)
        }
        "log_normal" | "lognormal" => {
            expect(2)?;
            Box::new(LogNormal::new(a[0], a[1])?)
        }
        "normal" => {
            expect(2)?;
            Box::new(Normal::new(a[0], a[1])?)
        }
        "normal_inverse_gaussian" | "normalinversegaussian" | "nig" => {
            expect(2)?;
            Box::new(NormalInverseGaussian::new(a[0], a[1])?)
        }
        "pareto" => {
            expect(2)?;
            Box::new(Pareto::new(a[0], a[1])?)
        }
        "pert" => {
            expect(4)?;
            Box::new(Pert::new(a[0], a[1]).with_shape(a[3]).with_mode(a[2])?)
        }
        "poisson" => {
            expect(1)?;
            Box::new(Poisson::new(a[0])?)
        }
        "skew_normal" | "skewnormal" => {
            expect(3)?;
            Box::new(SkewNormal::new(a[0], a[1], a[2])?)
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
            Box::new(StudentT::new(a[0])?)
        }
        "triangular" => {
            expect(3)?;
            Box::new(Triangular::new(a[0], a[1], a[2])?)
        }
        "uniform" => match a.len() {
            1 => Box::new(Uniform::new_inclusive(a[0], a[0])?),
            2 => Box::new(Uniform::new_inclusive(a[0], a[1])?),
            n => bail!("`uniform` expects 1 or 2 arguments, got {n}"),
        },
        "weibull" => {
            expect(2)?;
            Box::new(Weibull::new(a[0], a[1])?)
        }
        _ => bail!("unknown distribution `{name}`"),
    };

    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pert_args_are_min_max_mode_shape() {
        // mode=1 is inside [0, 10]; with mode and shape swapped it would be out of range
        build_dyn_distf32("pert", &[0.0, 10.0, 1.0, 20.0]).unwrap();
    }

    #[test]
    fn errors_name_the_expression() {
        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("foo(1) $ true").contains("unknown distribution `foo`"));
        assert!(err("normal(1) $ true").contains("in `normal(1)`"));
        assert!(err("beta(-1, 2) $ true").contains("in `beta(-1, 2)`"));
    }
}
