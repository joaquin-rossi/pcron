use crate::{Distribution, Tab, TabCmd};

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

fn parser_dist<'a>()
-> impl Parser<&'a str, Output = Distribution, Error = nom::error::Error<&'a str>> {
    let args = delimited(
        char('('),
        delimited(
            multispace0,
            separated_list1(delimited(multispace0, char(','), multispace0), parser_num()),
            multispace0,
        ),
        char(')'),
    );

    fn build_distribution<'a, E: ParseError<&'a str>>(
        name: &'a str,
        args: Vec<f32>,
    ) -> Result<Distribution, nom::Err<E>> {
        let err = || nom::Err::Error(E::from_error_kind(name, ErrorKind::Verify));

        let d = match name {
            "exp" => {
                if args.len() != 1 {
                    return Err(err());
                }
                Distribution::exp(args[0])
            }
            "poisson" => {
                if args.len() != 1 {
                    return Err(err());
                }
                Distribution::poisson(args[0])
            }
            _ => return Err(err()),
        };
        Ok(d)
    }

    map_res(pair(alpha1, args), |(name, args)| {
        build_distribution::<nom::error::Error<&str>>(name, args).map_err(|_| ())
    })
}

fn parser_cmd<'a>() -> impl Parser<&'a str, Output = TabCmd, Error = nom::error::Error<&'a str>> {
    map(
        pair(parser_dist(), preceded(space1, take_till1(|_| false))),
        |(dist, shell)| TabCmd {
            dist,
            shell: shell.to_string(),
        },
    )
}
