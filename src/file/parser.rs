use crate::file::lexer::{Token, Tokens};
use nom::{
    IResult, Parser,
    error::ErrorKind,
    multi::{many0, separated_list0},
};

#[derive(Debug)]
pub struct File {
    pub cmds: Vec<Command>,
}

#[derive(Debug)]
pub struct Command {
    pub expr: DistExpr,
    pub script: String,
}

#[derive(Debug)]
pub struct DistExpr {
    pub name: String,
    pub args: Vec<f32>,
}

pub fn parse(input: &[Token]) -> Result<File, String> {
    let input = Tokens(input);

    let (rest, file) = parse_file(input).map_err(|e| format!("parse error: {e:?}"))?;
    if rest.is_empty() {
        Ok(file)
    } else {
        Err(format!("parse error: unconsumed input: {:?}", rest))
    }
}

// File := Cmd*
fn parse_file(i: Tokens) -> IResult<Tokens, File> {
    let (i, cmds) = many0(parse_cmd).parse(i)?;
    Ok((i, File { cmds }))
}

// Cmd := DistExpr Script
fn parse_cmd(i: Tokens) -> IResult<Tokens, Command> {
    let (i, expr) = parse_dist_expr(i)?;
    let (i, script) = parse_t_script(i)?;
    Ok((
        i,
        Command {
            expr,
            script,
        },
    ))
}

// DistExpr := IDENT '(' params ')'
fn parse_dist_expr(i: Tokens) -> IResult<Tokens, DistExpr> {
    let (i, name) = parse_t_ident(i)?;
    let (i, _) = parse_t_paren_left(i)?;
    let (i, params) = parse_params_sep_with_comma(i)?;
    let (i, _) = parse_t_paren_right(i)?;

    Ok((i, DistExpr { name, args: params }))
}

// params := (NUM (',' NUM)*)?
fn parse_params_sep_with_comma(i: Tokens) -> IResult<Tokens, Vec<f32>> {
    separated_list0(parse_t_comma, parse_t_num).parse(i)
}

// ==============
// TOKEN MATCHERS
// ==============

fn parse_t_ident(i: Tokens) -> IResult<Tokens, String> {
    match i.split_first() {
        Some((Token::Ident(s), rest)) => Ok((Tokens(rest), s.clone())),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}

fn parse_t_num(i: Tokens) -> IResult<Tokens, f32> {
    match i.split_first() {
        Some((Token::Num(n), rest)) => Ok((Tokens(rest), *n as f32)),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}

fn parse_t_script(i: Tokens) -> IResult<Tokens, String> {
    match i.split_first() {
        Some((Token::Script(s), rest)) => Ok((Tokens(rest), s.clone())),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}

fn parse_t_paren_left(i: Tokens) -> IResult<Tokens, ()> {
    match i.split_first() {
        Some((Token::ParenLeft, rest)) => Ok((Tokens(rest), ())),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}

fn parse_t_paren_right(i: Tokens) -> IResult<Tokens, ()> {
    match i.split_first() {
        Some((Token::ParenRight, rest)) => Ok((Tokens(rest), ())),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}

fn parse_t_comma(i: Tokens) -> IResult<Tokens, ()> {
    match i.split_first() {
        Some((Token::Comma, rest)) => Ok((Tokens(rest), ())),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag))),
    }
}
