use crate::{DistFloat, Tab, TabCmd};

use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

pub fn parse_file(path: impl AsRef<Path>) -> io::Result<Tab> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut cmds = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.splitn(2, '#').next().unwrap_or("").trim();

        if line.is_empty() {
            continue;
        }

        let (_, cmd) = parser_cmd().parse_complete(line).unwrap();
        cmds.push(cmd);
    }

    Ok(Tab { cmds })
}

use nom::{
    Parser,
    bytes::complete::take_till1,
    character::complete::{alpha1, char, digit1, multispace0, one_of, space1},
    combinator::{map, map_res, opt, recognize},
    error::{ErrorKind, ParseError},
    multi::separated_list1,
    sequence::{delimited, pair, preceded},
};

fn parser_cmd<'a>() -> impl Parser<&'a str, Output = TabCmd, Error = nom::error::Error<&'a str>> {
    map(
        pair(parser_dist(), preceded(space1, take_till1(|_| false))),
        |(dist, shell)| TabCmd {
            dist,
            shell: shell.to_string(),
        },
    )
}

fn parser_dist<'a>()
-> impl Parser<&'a str, Output = Box<dyn DistFloat>, Error = nom::error::Error<&'a str>> {
    let args = delimited(
        char('('),
        delimited(
            multispace0,
            separated_list1(delimited(multispace0, char(','), multispace0), parser_num()),
            multispace0,
        ),
        char(')'),
    );

    map_res(pair(alpha1, args), |(name, args)| {
        build_dyn_distf32::<nom::error::Error<&str>>(name, &args).map_err(|_| ())
    })
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
pub fn build_dyn_distf32<'a, E: ParseError<&'a str>>(
    name: &'a str,
    a: &[f32],
) -> Result<Box<dyn DistFloat>, nom::Err<E>> {
    let err = || nom::Err::Error(E::from_error_kind(name, ErrorKind::Verify));
    let expect = |n: usize| if a.len() == n { Ok(()) } else { Err(err()) };

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
        "uniform" => {
            match a.len() {
                1 => Box::new(Uniform::new_inclusive(a[0], a[0]).unwrap()),
                2 => Box::new(Uniform::new_inclusive(a[0], a[1]).unwrap()),
                _ => return Err(err()),
            }
        }
        "weibull" => {
            expect(2)?;
            Box::new(Weibull::new(a[0], a[1]).unwrap())
        }
        _ => return Err(err()),
    };

    Ok(d)
}

fn parser_num<'a>() -> impl Parser<&'a str, Output = f32, Error = nom::error::Error<&'a str>> {
    map_res(
        recognize((
            opt(char('-')),
            digit1,
            opt((char('.'), digit1)),
            opt((one_of("eE"), opt(one_of("+-")), digit1)),
        )),
        str::parse::<f32>,
    )
}
