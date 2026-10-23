use std::{borrow::Cow, ops::Deref};

use nom::{
    IResult, Input, Needed, Parser,
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, digit0, digit1, line_ending, multispace0, one_of, space0},
    combinator::{eof, opt, recognize},
    error::{ErrorKind, ParseError},
    multi::many0,
    sequence::preceded,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Caret,
    Comma,
    Ident(String),
    Minus,
    Num(f64),
    ParenLeft,
    ParenRight,
    Plus,
    Script(String),
    Slash,
    Star,
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
        self.0.iter().position(predicate)
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

#[derive(Debug, PartialEq)]
pub struct LexError<'a> {
    pub input: &'a str,
    pub msg: Cow<'static, str>,
}

impl<'a> LexError<'a> {
    fn new(input: &'a str, msg: &'static str) -> Self {
        Self {
            input,
            msg: Cow::Borrowed(msg),
        }
    }
}

impl<'a> ParseError<&'a str> for LexError<'a> {
    fn from_error_kind(input: &'a str, kind: ErrorKind) -> Self {
        Self {
            input,
            msg: Cow::Owned(format!("unexpected input ({kind:?})")),
        }
    }

    fn append(_: &'a str, _: ErrorKind, other: Self) -> Self {
        other
    }
}

type LexResult<'a, T> = IResult<&'a str, T, LexError<'a>>;

pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let line_of = |rest: &str| input[..input.len() - rest.len()].matches('\n').count() + 1;

    match lex_tokens(input) {
        Ok(("", toks)) => Ok(toks),
        Ok((rest, _)) => Err(format!(
            "lex error: line {}: unexpected character {:?}",
            line_of(rest),
            rest.chars().next().unwrap_or_default()
        )),
        Err(nom::Err::Error(e) | nom::Err::Failure(e)) => {
            Err(format!("lex error: line {}: {}", line_of(e.input), e.msg))
        }
        Err(nom::Err::Incomplete(_)) => Err("lex error: incomplete input".to_string()),
    }
}

fn lex_tokens(i: &str) -> LexResult<'_, Vec<Token>> {
    let (i, toks) = many0(lex_token).parse(i)?;
    let (i, _) = lex_junk0(i)?;
    Ok((i, toks))
}

// any single token (skips junk first)
fn lex_token(i: &str) -> LexResult<'_, Token> {
    preceded(
        lex_junk0,
        alt((
            lex_comma,
            lex_ident,
            lex_num,
            lex_op,
            lex_paren_left,
            lex_paren_right,
            lex_script,
        )),
    )
    .parse(i)
}

// skip whitespace + comments repeatedly
fn lex_junk0(i: &str) -> LexResult<'_, ()> {
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
fn lex_line_comment(i: &str) -> LexResult<'_, ()> {
    preceded(char('#'), take_while(|c| c != '\n'))
        .map(|_| ())
        .parse(i)
}

// ','
fn lex_comma(i: &str) -> LexResult<'_, Token> {
    char(',').map(|_| Token::Comma).parse(i)
}

// unsigned: `1`, `1.5`, `1.`, `.5`, `1e-3` (signs are lexed as operators)
fn lex_num(i: &str) -> LexResult<'_, Token> {
    recognize((
        alt((
            recognize((digit1, opt((char('.'), digit0)))),
            recognize((char('.'), digit1)),
        )),
        opt((one_of("eE"), opt(one_of("+-")), digit1)),
    ))
    .map(|s: &str| Token::Num(s.parse().expect("recognized float syntax")))
    .parse(i)
}

// '+' | '-' | '*' | '/' | '^'
fn lex_op(i: &str) -> LexResult<'_, Token> {
    alt((
        char('+').map(|_| Token::Plus),
        char('-').map(|_| Token::Minus),
        char('*').map(|_| Token::Star),
        char('/').map(|_| Token::Slash),
        char('^').map(|_| Token::Caret),
    ))
    .parse(i)
}

fn lex_ident(i: &str) -> LexResult<'_, Token> {
    recognize((
        take_while1(|c: char| c == '_' || c.is_ascii_alphabetic()),
        take_while(|c: char| c == '_' || c.is_ascii_alphanumeric()),
    ))
    .map(|s: &str| Token::Ident(s.to_string()))
    .parse(i)
}

// '('
fn lex_paren_left(i: &str) -> LexResult<'_, Token> {
    char('(').map(|_| Token::ParenLeft).parse(i)
}

// ')'
fn lex_paren_right(i: &str) -> LexResult<'_, Token> {
    char(')').map(|_| Token::ParenRight).parse(i)
}

fn lex_script(i: &str) -> LexResult<'_, Token> {
    alt((lex_script_multiline, lex_script_single_line)).parse(i)
}

// $$ <newline> ... <newline> $$ (multiline, the closing `$$` alone on its line)
fn lex_script_multiline(i: &str) -> LexResult<'_, Token> {
    let start = i;
    let (i, _) = tag("$$").parse(i)?;
    let (i, _) = space0.parse(i)?;
    let (body_start, _) = alt((line_ending, eof))
        .parse(i)
        .map_err(|_: nom::Err<LexError>| {
            nom::Err::Failure(LexError::new(
                start,
                "expected a newline after `$$` (the script goes on the following lines)",
            ))
        })?;

    let mut rest = body_start;
    while !rest.is_empty() {
        let (line, next) = rest.split_once('\n').unwrap_or((rest, ""));
        if line.trim() == "$$" {
            let body = body_start[..body_start.len() - rest.len()].trim();
            if body.is_empty() {
                return Err(nom::Err::Failure(LexError::new(start, "empty script")));
            }
            return Ok((&rest[line.len()..], Token::Script(body.to_string())));
        }
        rest = next;
    }

    Err(nom::Err::Failure(LexError::new(
        start,
        "unterminated `$$` block (close it with a line containing only `$$`)",
    )))
}

// $ rest-of-line (single line)
fn lex_script_single_line(i: &str) -> LexResult<'_, Token> {
    let start = i;
    let (i, _) = char('$').parse(i)?;
    let (i, body) = take_while(|c| c != '\n').parse(i)?;

    let s = body.trim();
    if s.is_empty() {
        return Err(nom::Err::Failure(LexError::new(start, "empty script")));
    }
    Ok((i, Token::Script(s.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts(input: &str) -> Vec<String> {
        tokenize(input)
            .unwrap()
            .into_iter()
            .filter_map(|t| match t {
                Token::Script(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn numbers_and_operators() {
        use Token::*;
        assert_eq!(
            tokenize("2-1 +.5*1e-3/ 1.^-x").unwrap(),
            [
                Num(2.0),
                Minus,
                Num(1.0),
                Plus,
                Num(0.5),
                Star,
                Num(1e-3),
                Slash,
                Num(1.0),
                Caret,
                Minus,
                Ident("x".to_string()),
            ]
        );
    }

    #[test]
    fn single_line_script() {
        assert_eq!(scripts("exp(1) $ echo $$ # pid\n"), ["echo $$ # pid"]);
    }

    #[test]
    fn multiline_script_may_contain_dollar_dollar() {
        let tab = "exp(1) $$\n  tmp=/tmp/x.$$\n  echo $$ > $tmp\n$$\nexp(2) $ true\n";
        assert_eq!(scripts(tab), ["tmp=/tmp/x.$$\n  echo $$ > $tmp", "true"]);
    }

    #[test]
    fn multiline_closing_line_may_be_indented() {
        assert_eq!(scripts("exp(1) $$\n  echo hi\n  $$"), ["echo hi"]);
    }

    #[test]
    fn unterminated_multiline() {
        let err = tokenize("exp(1) $$\n echo hi\n").unwrap_err();
        assert!(
            err.contains("line 1") && err.contains("unterminated"),
            "{err}"
        );
    }

    #[test]
    fn text_after_multiline_opener() {
        let err = tokenize("exp(1) $$ echo hi\n$$\n").unwrap_err();
        assert!(err.contains("expected a newline"), "{err}");
    }

    #[test]
    fn empty_scripts() {
        for tab in ["\nexp(1) $\n", "\nexp(1) $$\n\n$$\n"] {
            let err = tokenize(tab).unwrap_err();
            assert!(
                err.contains("line 2") && err.contains("empty script"),
                "{err}"
            );
        }
    }

    #[test]
    fn unexpected_character() {
        let err = tokenize("# c\nexp(1) @ x\n").unwrap_err();
        assert!(err.contains("line 2") && err.contains("'@'"), "{err}");
    }
}
