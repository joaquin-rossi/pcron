use std::{borrow::Cow, fmt, ops::Range};

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, line_ending, multispace0, space0},
    combinator::{eof, recognize},
    error::{ErrorKind, ParseError},
    sequence::preceded,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Caret,
    Comma,
    /// `NAME=value`
    Env(String, String),
    Ident(String),
    Minus,
    /// `name:`, labels the command after it
    Name(String),
    Num(f64),
    ParenLeft,
    ParenRight,
    Plus,
    Script(String),
    Slash,
    Star,
    /// `@ ...`, up to the script
    Window(String),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Caret => f.write_str("`^`"),
            Token::Comma => f.write_str("`,`"),
            Token::Env(name, value) => write!(f, "assignment `{name}={value}`"),
            Token::Ident(s) => write!(f, "`{s}`"),
            Token::Minus => f.write_str("`-`"),
            Token::Name(name) => write!(f, "name `{name}:`"),
            Token::Num(n) => write!(f, "`{n}`"),
            Token::ParenLeft => f.write_str("`(`"),
            Token::ParenRight => f.write_str("`)`"),
            Token::Plus => f.write_str("`+`"),
            Token::Script(s) => match s.lines().next() {
                Some(first) if first.len() < s.len() => write!(f, "script `{first} ...`"),
                _ => write!(f, "script `{s}`"),
            },
            Token::Slash => f.write_str("`/`"),
            Token::Star => f.write_str("`*`"),
            Token::Window(text) => write!(f, "window `@ {text}`"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lexeme {
    pub token: Token,
    pub line: usize,
    /// byte range in the tab
    pub span: Range<usize>,
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

pub fn tokenize(input: &str) -> Result<Vec<Lexeme>, String> {
    let newlines = |from: &str, to: &str| from[..from.len() - to.len()].matches('\n').count();
    let offset = |rest: &str| input.len() - rest.len();

    let mut toks = Vec::new();
    let mut line = 1;
    let mut i = input;
    loop {
        let (rest, _) = lex_junk0(i).map_err(|_| "unexpected input".to_string())?;
        line += newlines(i, rest);
        let Some(c) = rest.chars().next() else {
            return Ok(toks);
        };

        match lex_token(rest) {
            Ok((next, token)) => {
                toks.push(Lexeme {
                    token,
                    line,
                    span: offset(rest)..offset(next),
                });
                line += newlines(rest, next);
                i = next;
            }
            Err(nom::Err::Failure(e)) => {
                return Err(format!(
                    "line {}: {}",
                    line + newlines(rest, e.input),
                    e.msg
                ));
            }
            Err(_) => return Err(format!("line {line}: unexpected character {c:?}")),
        }
    }
}

fn lex_token(i: &str) -> LexResult<'_, Token> {
    alt((
        lex_comma,
        lex_name,
        lex_env,
        lex_ident,
        lex_window,
        lex_num,
        lex_op,
        lex_paren_left,
        lex_paren_right,
        lex_script,
    ))
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

// unsigned: `1`, `1.5`, `1.`, `.5`, `1e-3` (signs are lexed as operators); scanned by
// hand because nom's recognize got it wrong at the end of the input ("1.5" -> "1")
fn lex_num(i: &str) -> LexResult<'_, Token> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();

    let mut n = digits(i);
    if i[n..].starts_with('.') {
        n += 1 + digits(&i[n + 1..]);
    }
    if n == 0 || &i[..n] == "." {
        return Err(nom::Err::Error(LexError::from_error_kind(
            i,
            ErrorKind::Digit,
        )));
    }

    let rest = &i[n..];
    if rest.starts_with(['e', 'E']) {
        let sign = usize::from(rest[1..].starts_with(['+', '-']));
        let exp = digits(&rest[1 + sign..]);
        if exp > 0 {
            n += 1 + sign + exp;
        }
    }

    let num = i[..n].parse().expect("scanned float syntax");
    Ok((&i[n..], Token::Num(num)))
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

// name: (letters, digits, `_`, `-` and `.`)
fn lex_name(i: &str) -> LexResult<'_, Token> {
    let (i, name) = recognize((
        take_while1(|c: char| c == '_' || c.is_ascii_alphabetic()),
        take_while(|c: char| matches!(c, '_' | '-' | '.') || c.is_ascii_alphanumeric()),
    ))
    .parse(i)?;
    let (i, _) = char(':').parse(i)?;
    Ok((i, Token::Name(name.to_string())))
}

// @ days hours, up to the script (or the end of the line)
fn lex_window(i: &str) -> LexResult<'_, Token> {
    let start = i;
    let (i, _) = char('@').parse(i)?;
    let (i, text) = take_while(|c| c != '$' && c != '\n').parse(i)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(nom::Err::Failure(LexError::new(
            start,
            "empty window after `@`",
        )));
    }
    Ok((i, Token::Window(text.to_string())))
}

// NAME = value, up to the end of the line; quotes around the value keep its outer blanks
fn lex_env(i: &str) -> LexResult<'_, Token> {
    let (i, name) = recognize((
        take_while1(|c: char| c == '_' || c.is_ascii_alphabetic()),
        take_while(|c: char| c == '_' || c.is_ascii_alphanumeric()),
    ))
    .parse(i)?;
    let (i, _) = (space0, char('=')).parse(i)?;
    let (i, value) = take_while(|c| c != '\n').parse(i)?;

    let value = value.trim();
    let value = ['"', '\'']
        .into_iter()
        .find_map(|q| value.strip_prefix(q)?.strip_suffix(q))
        .unwrap_or(value);
    Ok((i, Token::Env(name.to_string(), value.to_string())))
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
            .filter_map(|l| match l.token {
                Token::Script(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn numbers_and_operators() {
        use Token::*;
        for (text, num) in [("1.5", 1.5), ("10.25", 10.25), ("2e3", 2e3), ("7.", 7.0)] {
            let toks = tokenize(text).unwrap();
            assert_eq!(toks[0].token, Num(num), "{text}");
            assert_eq!(toks.len(), 1, "{text}");
        }

        let toks: Vec<_> = tokenize("2-1 +.5*1e-3/ 1.^-x")
            .unwrap()
            .into_iter()
            .map(|l| l.token)
            .collect();
        assert_eq!(
            toks,
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
    fn tokens_know_their_line() {
        let lines: Vec<_> = tokenize("# c\nexp(1) $$\n a\n b\n$$\n\nexp(2)\n  $ c")
            .unwrap()
            .into_iter()
            .map(|l| l.line)
            .collect();
        assert_eq!(lines, [2, 2, 2, 2, 2, 7, 7, 7, 7, 8]);
    }

    #[test]
    fn env_assignments() {
        let toks: Vec<_> = tokenize(
            "A=1\nB = two words # not a comment\nC=\"  quoted \"\nD=''\nE=\nexp(1) $ F=x env\n",
        )
        .unwrap()
        .into_iter()
        .map(|l| l.token)
        .collect();
        let env = |n: &str, v: &str| Token::Env(n.to_string(), v.to_string());
        assert_eq!(
            toks[..5],
            [
                env("A", "1"),
                env("B", "two words # not a comment"),
                env("C", "  quoted "),
                env("D", ""),
                env("E", ""),
            ]
        );
        assert_eq!(toks.last(), Some(&Token::Script("F=x env".to_string())));
    }

    #[test]
    fn names_and_windows() {
        use Token::*;
        let toks: Vec<_> =
            tokenize("daily-backup.v2: 60 @ mon-fri 9-18 $ x\na: exp(1)\n@ 9-18\n$ y")
                .unwrap()
                .into_iter()
                .map(|l| l.token)
                .collect();
        assert_eq!(
            toks,
            [
                Name("daily-backup.v2".to_string()),
                Num(60.0),
                Window("mon-fri 9-18".to_string()),
                Script("x".to_string()),
                Name("a".to_string()),
                Ident("exp".to_string()),
                ParenLeft,
                Num(1.0),
                ParenRight,
                Window("9-18".to_string()),
                Script("y".to_string()),
            ]
        );

        let err = tokenize("exp(1) @ $ x").unwrap_err();
        assert!(err.contains("empty window"), "{err}");
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
        let err = tokenize("# c\nexp(1) ! x\n").unwrap_err();
        assert!(err.contains("line 2") && err.contains("'!'"), "{err}");
    }
}
