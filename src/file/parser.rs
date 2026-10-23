use crate::file::lexer::{Token, Tokens};
use std::fmt;

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
    pub args: Vec<Expr>,
}

#[derive(Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

impl fmt::Display for DistExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args: Vec<_> = self.args.iter().map(Expr::to_string).collect();
        write!(f, "{}({})", self.name, args.join(", "))
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // parenthesize nested binary operations so the output is unambiguous
        let operand = |e: &Expr| match e {
            Expr::Bin(..) => format!("({e})"),
            _ => e.to_string(),
        };
        match self {
            Expr::Num(n) => write!(f, "{n}"),
            Expr::Neg(e) => write!(f, "-{}", operand(e)),
            Expr::Bin(op, l, r) => write!(f, "{} {op} {}", operand(l), operand(r)),
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
        })
    }
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
    Ok((i, Command { expr, script }))
}

// DistExpr := IDENT '(' params ')'
fn parse_dist_expr(i: Tokens) -> IResult<Tokens, DistExpr> {
    let (i, name) = parse_t_ident(i)?;
    let (i, _) = parse_t_paren_left(i)?;
    let (i, params) = parse_params_sep_with_comma(i)?;
    let (i, _) = parse_t_paren_right(i)?;

    Ok((i, DistExpr { name, args: params }))
}

// params := (Expr (',' Expr)*)?
fn parse_params_sep_with_comma(i: Tokens) -> IResult<Tokens, Vec<Expr>> {
    separated_list0(parse_t_comma, parse_expr).parse(i)
}

// Expr := Term (('+' | '-') Term)*
fn parse_expr(i: Tokens) -> IResult<Tokens, Expr> {
    let (mut i, mut lhs) = parse_term(i)?;
    loop {
        let op = match i.first() {
            Some(Token::Plus) => BinOp::Add,
            Some(Token::Minus) => BinOp::Sub,
            _ => return Ok((i, lhs)),
        };
        let (rest, rhs) = parse_term(Tokens(&i[1..]))?;
        lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        i = rest;
    }
}

// Term := Unary (('*' | '/') Unary)*
fn parse_term(i: Tokens) -> IResult<Tokens, Expr> {
    let (mut i, mut lhs) = parse_unary(i)?;
    loop {
        let op = match i.first() {
            Some(Token::Star) => BinOp::Mul,
            Some(Token::Slash) => BinOp::Div,
            _ => return Ok((i, lhs)),
        };
        let (rest, rhs) = parse_unary(Tokens(&i[1..]))?;
        lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        i = rest;
    }
}

// Unary := ('+' | '-') Unary | Power
fn parse_unary(i: Tokens) -> IResult<Tokens, Expr> {
    match i.first() {
        Some(Token::Plus) => parse_unary(Tokens(&i[1..])),
        Some(Token::Minus) => {
            let (i, e) = parse_unary(Tokens(&i[1..]))?;
            Ok((i, Expr::Neg(Box::new(e))))
        }
        _ => parse_power(i),
    }
}

// Power := Atom ('^' Unary)?
// right associative and tighter than unary minus: 2^3^2 = 2^(3^2), -2^2 = -(2^2)
fn parse_power(i: Tokens) -> IResult<Tokens, Expr> {
    let (i, base) = parse_atom(i)?;
    match i.first() {
        Some(Token::Caret) => {
            let (i, exp) = parse_unary(Tokens(&i[1..]))?;
            Ok((i, Expr::Bin(BinOp::Pow, Box::new(base), Box::new(exp))))
        }
        _ => Ok((i, base)),
    }
}

// Atom := NUM | '(' Expr ')'
fn parse_atom(i: Tokens) -> IResult<Tokens, Expr> {
    if let Ok((i, n)) = parse_t_num(i) {
        return Ok((i, Expr::Num(n)));
    }
    let (i, _) = parse_t_paren_left(i)?;
    let (i, e) = parse_expr(i)?;
    let (i, _) = parse_t_paren_right(i)?;
    Ok((i, e))
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

fn parse_t_num(i: Tokens) -> IResult<Tokens, f64> {
    match i.split_first() {
        Some((Token::Num(n), rest)) => Ok((Tokens(rest), *n)),
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
