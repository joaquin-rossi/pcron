use std::ops::Deref;

use nom::{
    IResult, Input, Needed, Parser,
    branch::alt,
    bytes::complete::{tag, take_until, take_while, take_while1},
    character::complete::{char, multispace0},
    combinator::{map, recognize},
    multi::many0,
    number::complete::double,
    sequence::preceded,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Comma,
    Ident(String),
    Num(f64),
    ParenLeft,
    ParenRight,
    Script(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens<'a>(pub &'a [Token]);

impl<'a> Deref for Tokens<'a> {
    type Target = &'a [Token];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> Tokens<'a> {
    #[inline]
    pub fn as_slice(self) -> &'a [Token] {
        self.0
    }
}

impl<'a> Input for Tokens<'a> {
    type Item = &'a Token;
    type Iter = std::slice::Iter<'a, Token>;
    type IterIndices = std::iter::Enumerate<std::slice::Iter<'a, Token>>;

    #[inline]
    fn input_len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn take(&self, index: usize) -> Self {
        Tokens(&self.0[..index])
    }

    #[inline]
    fn take_from(&self, index: usize) -> Self {
        Tokens(&self.0[index..])
    }

    #[inline]
    fn take_split(&self, index: usize) -> (Self, Self) {
        (self.take_from(index), self.take(index))
    }

    #[inline]
    fn position<P>(&self, predicate: P) -> Option<usize>
    where
        P: Fn(Self::Item) -> bool,
    {
        self.0.iter().position(|t| predicate(t))
    }

    #[inline]
    fn iter_elements(&self) -> Self::Iter {
        self.0.iter()
    }

    #[inline]
    fn iter_indices(&self) -> Self::IterIndices {
        self.0.iter().enumerate()
    }

    #[inline]
    fn slice_index(&self, count: usize) -> Result<usize, Needed> {
        if self.0.len() >= count {
            Ok(count)
        } else {
            Err(Needed::new(count - self.0.len()))
        }
    }
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let (rest, toks) = lex_tokens(input).map_err(|e| format!("lex error: {e:?}"))?;
    if rest.is_empty() {
        Ok(toks)
    } else {
        Err(format!("lex error: unconsumed input: {:?}", rest))
    }
}

fn lex_tokens(i: &str) -> IResult<&str, Vec<Token>> {
    let (i, toks) = many0(lex_token).parse(i)?;
    let (i, _) = lex_junk0(i)?;
    Ok((i, toks))
}

// any single token (skips junk first)
fn lex_token(i: &str) -> IResult<&str, Token> {
    preceded(
        lex_junk0,
        alt((
            lex_comma,
            lex_ident,
            lex_num,
            lex_paren_left,
            lex_paren_right,
            lex_script,
        )),
    )
    .parse(i)
}

// skip whitespace + comments repeatedly
fn lex_junk0(i: &str) -> IResult<&str, ()> {
    let (mut i, _) = multispace0.parse(i)?;

    loop {
        let before = i;

        if let Ok((i2, _)) = lex_line_comment(i) {
            i = i2;
        }

        let (i2, _) = multispace0.parse(i)?;
        i = i2;

        if i == before {
            break;
        }
    }

    Ok((i, ()))
}

// match `# ... until newline` (newline NOT consumed)
fn lex_line_comment(i: &str) -> IResult<&str, ()> {
    preceded(char('#'), take_while(|c| c != '\n'))
        .map(|_| ())
        .parse(i)
}

// ','
fn lex_comma(i: &str) -> IResult<&str, Token> {
    char(',').map(|_| Token::Comma).parse(i)
}

fn lex_num(i: &str) -> IResult<&str, Token> {
    map(double, Token::Num).parse(i)
}

fn lex_ident(i: &str) -> IResult<&str, Token> {
    recognize((
        take_while1(|c: char| c == '_' || c.is_ascii_alphabetic()),
        take_while(|c: char| c == '_' || c.is_ascii_alphanumeric()),
    ))
    .map(|s: &str| Token::Ident(s.to_string()))
    .parse(i)
}

// '('
fn lex_paren_left(i: &str) -> IResult<&str, Token> {
    char('(').map(|_| Token::ParenLeft).parse(i)
}

// ')'
fn lex_paren_right(i: &str) -> IResult<&str, Token> {
    char(')').map(|_| Token::ParenRight).parse(i)
}

fn lex_script(i: &str) -> IResult<&str, Token> {
    alt((lex_script_multiline, lex_script_single_line)).parse(i)
}

// $$ ... $$ (multiline)
fn lex_script_multiline(i: &str) -> IResult<&str, Token> {
    let (i, _) = tag("$$").parse(i)?;

    let (i, body) = take_until("$$").parse(i)?;
    let (i, _) = tag("$$").parse(i)?;

    let s = body.trim().to_string();
    Ok((i, Token::Script(s)))
}

// $ rest-of-line (single line)
fn lex_script_single_line(i: &str) -> IResult<&str, Token> {
    let (i, _) = char('$').parse(i)?;
    let (i, body) = take_while(|c| c != '\n').parse(i)?;

    let s = body.trim().to_string();
    Ok((i, Token::Script(s)))
}
