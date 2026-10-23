use crate::file::lexer::{Lexeme, Token};
use std::{collections::BTreeMap, fmt, ops::Range};

#[derive(Debug)]
pub struct File {
    pub cmds: Vec<Command>,
}

#[derive(Debug)]
pub struct Command {
    pub line: usize,
    /// the `NAME=value` lines above the command (later ones win), sorted by name
    pub env: Vec<(String, String)>,
    /// byte range of the distribution in the tab
    pub span: Range<usize>,
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

    // Cmd := IDENT '(' Args ')' SCRIPT
    fn cmd(&mut self) -> Result<Command, String> {
        let line = self.line();
        let start = self.input[self.pos].span.start;
        let Some(Token::Ident(name)) = self.peek() else {
            return self.error("a distribution name");
        };
        self.pos += 1;

        self.expect(&Token::ParenLeft, &format!("`(` after `{name}`"))?;
        let args = self.args()?;
        self.expect(&Token::ParenRight, "`,` or `)`")?;
        let span = start..self.input[self.pos - 1].span.end;
        let expr = DistExpr {
            name: name.clone(),
            args,
        };

        let Some(Token::Script(script)) = self.peek() else {
            return self.error(&format!(
                "a script (`$ ...` or a `$$` block) after `{expr}`"
            ));
        };
        self.pos += 1;

        Ok(Command {
            line,
            env: Vec::new(),
            span,
            expr,
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

    // Atom := NUM | '(' Expr ')'
    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Token::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(*n))
            }
            Some(Token::ParenLeft) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(&Token::ParenRight, "`)`")?;
                Ok(e)
            }
            _ => self.error("a number or `(`"),
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
                "exp(1) $ a\n(1) $ b",
                "line 2: expected a distribution name, found `(`",
            ),
            (
                "exp(1 +) $ a",
                "line 1: expected a number or `(`, found `)`",
            ),
            ("exp((1 $ a", "line 1: expected `)`, found script `a`"),
            (
                "exp(",
                "line 1: expected a number or `(`, found end of file",
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
    fn commands_know_their_line() {
        let file = parse(&tokenize("# c\nexp(1) $ a\n\nexp(2) $$\n b\n$$").unwrap()).unwrap();
        let lines: Vec<_> = file.cmds.iter().map(|c| c.line).collect();
        assert_eq!(lines, [2, 4]);
    }
}
