use crate::file::lexer::{Lexeme, Token};
use std::{collections::BTreeMap, fmt, ops::Range};

#[derive(Debug)]
pub struct File {
    pub cmds: Vec<Command>,
}

#[derive(Debug)]
pub struct Command {
    pub line: usize,
    pub name: Option<String>,
    /// the `NAME=value` lines above the command (later ones win), sorted by name
    pub env: Vec<(String, String)>,
    /// byte range of the delay in the tab
    pub span: Range<usize>,
    pub expr: Expr,
    pub window: Option<String>,
    pub script: String,
}

#[derive(Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    /// a distribution
    Call(String, Vec<Expr>),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// whether it has a distribution somewhere
    pub fn is_random(&self) -> bool {
        match self {
            Expr::Num(_) => false,
            Expr::Call(..) => true,
            Expr::Neg(e) => e.is_random(),
            Expr::Bin(_, l, r) => l.is_random() || r.is_random(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
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
            Expr::Call(name, args) => {
                let args: Vec<_> = args.iter().map(Expr::to_string).collect();
                write!(f, "{name}({})", args.join(", "))
            }
            Expr::Neg(e) => write!(f, "-{}", operand(e)),
            Expr::Bin(op, l, r) => write!(f, "{} {op} {}", operand(l), operand(r)),
        }
    }
}

impl BinOp {
    pub fn apply(self, l: f64, r: f64) -> f64 {
        match self {
            BinOp::Add => l + r,
            BinOp::Sub => l - r,
            BinOp::Mul => l * r,
            BinOp::Div => l / r,
            BinOp::Pow => l.powf(r),
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

pub fn parse(input: &[Lexeme]) -> Result<File, String> {
    let mut p = Parser { input, pos: 0 };
    let mut cmds = Vec::new();
    let mut env = BTreeMap::new();
    while let Some(t) = p.peek() {
        if let Token::Env(name, value) = t {
            env.insert(name.clone(), value.clone());
            p.pos += 1;
            continue;
        }
        let mut cmd = p.cmd()?;
        cmd.env = env.clone().into_iter().collect();
        cmds.push(cmd);
    }
    Ok(File { cmds })
}

/// a lone expression (the value of a setting)
pub fn parse_expr(input: &[Lexeme]) -> Result<Expr, String> {
    let mut p = Parser { input, pos: 0 };
    let e = p.expr()?;
    if p.peek().is_some() {
        return p.error("the end of the value");
    }
    Ok(e)
}

struct Parser<'a> {
    input: &'a [Lexeme],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.input.get(self.pos).map(|l| &l.token)
    }

    fn eat(&mut self, t: &Token) -> bool {
        let found = self.peek() == Some(t);
        if found {
            self.pos += 1;
        }
        found
    }

    fn expect(&mut self, t: &Token, expected: &str) -> Result<(), String> {
        if self.eat(t) {
            Ok(())
        } else {
            self.error(expected)
        }
    }

    // line of the next token (or of the last one, at end of file)
    fn line(&self) -> usize {
        self.input
            .get(self.pos)
            .or(self.input.last())
            .map_or(1, |l| l.line)
    }

    fn error<T>(&self, expected: &str) -> Result<T, String> {
        let found = match self.peek() {
            Some(t) => t.to_string(),
            None => "end of file".to_string(),
        };
        Err(format!(
            "line {}: expected {expected}, found {found}",
            self.line()
        ))
    }

    // Cmd := NAME? Expr WINDOW? SCRIPT
    fn cmd(&mut self) -> Result<Command, String> {
        let line = self.line();
        let name = match self.peek() {
            Some(Token::Name(name)) => {
                self.pos += 1;
                Some(name.clone())
            }
            _ => None,
        };

        let Some(start) = self.input.get(self.pos).map(|l| l.span.start) else {
            return self.error("a delay");
        };
        let expr = self.expr()?;
        let span = start..self.input[self.pos - 1].span.end;

        let window = match self.peek() {
            Some(Token::Window(text)) => {
                self.pos += 1;
                Some(text.clone())
            }
            _ => None,
        };

        let Some(Token::Script(script)) = self.peek() else {
            return self.error(&format!(
                "a script (`$ ...` or a `$$` block) after `{expr}`"
            ));
        };
        self.pos += 1;

        Ok(Command {
            line,
            name,
            env: Vec::new(),
            span,
            expr,
            window,
            script: script.clone(),
        })
    }

    // Args := (Expr (',' Expr)*)?
    fn args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if self.peek() == Some(&Token::ParenRight) {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if !self.eat(&Token::Comma) {
                return Ok(args);
            }
        }
    }

    // Expr := Term (('+' | '-') Term)*
    fn expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(self.term()?));
        }
    }

    // Term := Unary (('*' | '/') Unary)*
    fn term(&mut self) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(self.unary()?));
        }
    }

    // Unary := ('+' | '-') Unary | Power
    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat(&Token::Plus) {
            self.unary()
        } else if self.eat(&Token::Minus) {
            Ok(Expr::Neg(Box::new(self.unary()?)))
        } else {
            self.power()
        }
    }

    // Power := Atom ('^' Unary)?
    // right associative and tighter than unary minus: 2^3^2 = 2^(3^2), -2^2 = -(2^2)
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if self.eat(&Token::Caret) {
            let exp = self.unary()?;
            Ok(Expr::Bin(BinOp::Pow, Box::new(base), Box::new(exp)))
        } else {
            Ok(base)
        }
    }

    // Atom := NUM | IDENT '(' Args ')' | '(' Expr ')'
    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Token::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(*n))
            }
            Some(Token::Ident(name)) => {
                self.pos += 1;
                self.expect(&Token::ParenLeft, &format!("`(` after `{name}`"))?;
                let args = self.args()?;
                self.expect(&Token::ParenRight, "`,` or `)`")?;
                Ok(Expr::Call(name.clone(), args))
            }
            Some(Token::ParenLeft) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(&Token::ParenRight, "`)`")?;
                Ok(e)
            }
            _ => self.error("a number, a distribution or `(`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::lexer::tokenize;

    fn parse_err(tab: &str) -> String {
        parse(&tokenize(tab).unwrap()).unwrap_err()
    }

    #[test]
    fn syntax_errors() {
        let cases = [
            (
                "exp(1/60 $ true",
                "line 1: expected `,` or `)`, found script `true`",
            ),
            (
                "\nexp 1 $ true",
                "line 2: expected `(` after `exp`, found `1`",
            ),
            ("exp(1)\n\n", "line 1: expected a script"),
            (
                "exp(1) $ a\n$ b",
                "line 2: expected a number, a distribution or `(`, found script `b`",
            ),
            (
                "exp(1 +) $ a",
                "line 1: expected a number, a distribution or `(`, found `)`",
            ),
            ("x:", "line 1: expected a delay, found end of file"),
            ("exp((1 $ a", "line 1: expected `)`, found script `a`"),
            (
                "exp(",
                "line 1: expected a number, a distribution or `(`, found end of file",
            ),
        ];
        for (tab, want) in cases {
            let err = parse_err(tab);
            assert!(err.starts_with(want), "{tab:?}: {err}");
        }
    }

    #[test]
    fn env_applies_to_the_commands_below() {
        let tab = "exp(1) $ a\nA=1\nB=2\nexp(1) $ b\nA=3\nexp(1) $ c";
        let file = parse(&tokenize(tab).unwrap()).unwrap();
        let env: Vec<Vec<_>> = file
            .cmds
            .iter()
            .map(|c| c.env.iter().map(|(k, v)| format!("{k}={v}")).collect())
            .collect();
        assert_eq!(env, [vec![], vec!["A=1", "B=2"], vec!["A=3", "B=2"]]);
    }

    #[test]
    fn full_commands() {
        let tab = "backup: 3600 + exp(1/600) @ mon-fri 9-18 $ a\n2 * (normal(1, 2) - 1) $ b";
        let file = parse(&tokenize(tab).unwrap()).unwrap();
        let [a, b] = &file.cmds[..] else { panic!() };
        assert_eq!(a.name.as_deref(), Some("backup"));
        assert_eq!(a.expr.to_string(), "3600 + exp(1 / 600)");
        assert_eq!(a.window.as_deref(), Some("mon-fri 9-18"));
        assert_eq!(&tab[a.span.clone()], "3600 + exp(1/600)");
        assert_eq!((b.name.as_deref(), b.window.as_deref()), (None, None));
        assert_eq!(b.expr.to_string(), "2 * (normal(1, 2) - 1)");
    }

    #[test]
    fn commands_know_their_line() {
        let file = parse(&tokenize("# c\nexp(1) $ a\n\nexp(2) $$\n b\n$$").unwrap()).unwrap();
        let lines: Vec<_> = file.cmds.iter().map(|c| c.line).collect();
        assert_eq!(lines, [2, 4]);
    }
}
