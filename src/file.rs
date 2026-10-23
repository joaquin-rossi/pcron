pub mod lexer;
pub mod parser;

use crate::{DistFloat, Tab, TabCmd};
use parser::{BinOp, Expr};

use anyhow::{Context, Result, anyhow, bail, ensure};
use rand::Rng;
use rand_distr::Distribution;
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
            let (args, dist) = cmd
                .expr
                .args
                .iter()
                .map(eval_arg)
                .collect::<Result<Vec<_>>>()
                .and_then(|args| {
                    let dist = build_dyn_dist(&cmd.expr.name, &args)?;
                    Ok((args, dist))
                })
                .with_context(|| format!("line {}: in `{}`", cmd.line, cmd.expr))?;
            let source = text[cmd.span]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Ok(TabCmd {
                line: cmd.line,
                source,
                name: cmd.expr.name,
                args,
                dist,
                script: cmd.script,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Tab { cmds })
}

// arguments must end up non-negative (eval already rejects non-finite values)
fn eval_arg(e: &Expr) -> Result<f64> {
    let v = eval(e)?;
    ensure!(v >= 0.0, "`{e}` is negative ({v})");
    Ok(v)
}

// every intermediate result must be finite (catches 1/0, 0^-1, (-8)^(1/3), ...)
fn eval(e: &Expr) -> Result<f64> {
    let v = match e {
        Expr::Num(n) => *n,
        Expr::Neg(x) => -eval(x)?,
        Expr::Bin(op, l, r) => {
            let (l, r) = (eval(l)?, eval(r)?);
            match op {
                BinOp::Add => l + r,
                BinOp::Sub => l - r,
                BinOp::Mul => l * r,
                BinOp::Div => l / r,
                BinOp::Pow => l.powf(r),
            }
        }
    };
    ensure!(v.is_finite(), "`{e}` is not a finite number ({v})");
    Ok(v)
}

// - beta(alpha, beta)
// - cauchy(median, scale)
// - const(value)
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
// - uniform(min, max)
// - weibull(scale, shape)
// always samples the same value
struct Const(f64);

impl Distribution<f64> for Const {
    fn sample<R: Rng + ?Sized>(&self, _: &mut R) -> f64 {
        self.0
    }
}

pub fn build_dyn_dist(name: &str, a: &[f64]) -> Result<Box<dyn DistFloat>> {
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
        "const" => {
            expect(1)?;
            Box::new(Const(a[0]))
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
            2 => Box::new(Uniform::new_inclusive(a[0], a[1])?),
            1 => bail!("`uniform` expects 2 arguments (min, max); for a fixed delay use `const`"),
            n => bail!("`uniform` expects 2 arguments, got {n}"),
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
        build_dyn_dist("pert", &[0.0, 10.0, 1.0, 20.0]).unwrap();
    }

    fn eval_str(arg: &str) -> Result<f64> {
        let tokens = lexer::tokenize(&format!("x({arg}) $ true")).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        eval(&ast.cmds[0].expr.args[0])
    }

    #[test]
    fn arithmetic() {
        let cases = [
            ("1/60", 1.0 / 60.0),
            ("1 + 2 * 3", 7.0),
            ("(1 + 2) * 3", 9.0),
            ("10 - 4 - 3", 3.0),
            ("8 / 4 / 2", 1.0),
            ("2^3^2", 512.0),
            ("-2^2", -4.0),
            ("2^-1", 0.5),
            ("+3 - -2", 5.0),
            ("1.5e2", 150.0),
        ];
        for (arg, want) in cases {
            assert_eq!(eval_str(arg).unwrap(), want, "{arg}");
        }
    }

    #[test]
    fn non_finite_is_an_error() {
        for arg in ["1/0", "0^-1", "(-8)^(1/3)", "0/0", "1e400"] {
            let err = format!("{:#}", eval_str(arg).unwrap_err());
            assert!(err.contains("not a finite number"), "{arg}: {err}");
        }
    }

    #[test]
    fn arg_must_be_non_negative() {
        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("exp(1 - 2) $ true").contains("`1 - 2` is negative"));
        parse("exp(1/60) $ true\nnormal(-1 + 2, 0) $ true").unwrap();
    }

    #[test]
    fn const_and_uniform() {
        let d = build_dyn_dist("const", &[42.5]).unwrap();
        assert!((0..10).all(|_| d.sample() == 42.5));

        let d = build_dyn_dist("uniform", &[1.0, 2.0]).unwrap();
        assert!(
            (0..100)
                .map(|_| d.sample())
                .all(|x| (1.0..=2.0).contains(&x))
        );

        let err = build_dyn_dist("uniform", &[10.0])
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("use `const`"), "{err}");
    }

    #[test]
    fn errors_name_the_expression() {
        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("foo(1) $ true").contains("unknown distribution `foo`"));
        assert!(err("normal(1) $ true").contains("in `normal(1)`"));
        assert!(err("beta(-1, 2) $ true").contains("in `beta(-1, 2)`"));
    }
}
