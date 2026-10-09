//! The rule language: a boolean expression over fields, as data (`Expr`, serialisable) and as text (a small
//! grammar the file, the CLI and the UI all share).
//!
//! ```text
//! protocol == aave_v3 && selector in [0xa415bcad, 0x573ade81] && liquidation.amount >= 50000e6
//! kind == swap && swap.amount_in > 1e18 && !(via contains multicall3)
//! kind == undecoded_known || (kind == unknown && len > 4)
//! ```
//! `&&`/`and`, `||`/`or`, `!`/`not`, parentheses; comparisons `== != < <= > >=`; `in [a, b]`; `contains`; `set(field)`
//! (the field has a value for this call). Numbers: decimal, `0x` hex, `_` separators, `e` exponent (`5e6`), `-` for
//! signed fields. Words and quoted strings are text.

use serde::{Deserialize, Serialize};

use crate::value::Op;

/// A literal as written; it is given a type when the rule is compiled against the field it is compared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Literal {
    /// A number or hex value, as written (`5e6`, `0xa415bcad`).
    Raw(String),
    Str(String),
    Bool(bool),
}

/// Words that can be written without quotes: letters, digits and `_`, not starting with a digit, and not a keyword.
fn is_bare_word(s: &str) -> bool {
    const KEYWORDS: [&str; 8] = ["true", "false", "and", "or", "not", "in", "contains", "set"];
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&s)
}

impl std::fmt::Display for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Literal::Raw(r) => f.write_str(r),
            Literal::Str(s) if is_bare_word(s) => f.write_str(s),
            Literal::Str(s) => write!(f, "\"{}\"", s.replace('"', "\\\"")),
            Literal::Bool(b) => write!(f, "{b}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Expr {
    True,
    And {
        args: Vec<Expr>,
    },
    Or {
        args: Vec<Expr>,
    },
    Not {
        arg: Box<Expr>,
    },
    Cmp {
        field: String,
        cmp: Op,
        value: Literal,
    },
    In {
        field: String,
        values: Vec<Literal>,
    },
    Contains {
        field: String,
        value: Literal,
    },
    Set {
        field: String,
    },
}

impl std::fmt::Display for Expr {
    /// The canonical text; parsing it gives the same expression back.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn join(
            f: &mut std::fmt::Formatter<'_>,
            args: &[Expr],
            sep: &str,
            wrap: fn(&Expr) -> bool,
        ) -> std::fmt::Result {
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    f.write_str(sep)?;
                }
                if wrap(a) {
                    write!(f, "({a})")?;
                } else {
                    write!(f, "{a}")?;
                }
            }
            Ok(())
        }
        match self {
            Expr::True => f.write_str("true"),
            Expr::And { args } => join(f, args, " && ", |e| matches!(e, Expr::Or { .. })),
            Expr::Or { args } => join(f, args, " || ", |_| false),
            Expr::Not { arg } => match **arg {
                Expr::And { .. } | Expr::Or { .. } => write!(f, "!({arg})"),
                _ => write!(f, "!{arg}"),
            },
            Expr::Cmp { field, cmp, value } => write!(f, "{field} {} {value}", cmp.symbol()),
            Expr::In { field, values } => {
                write!(f, "{field} in [")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Expr::Contains { field, value } => write!(f, "{field} contains {value}"),
            Expr::Set { field } => write!(f, "set({field})"),
        }
    }
}

/// Where and why a rule's text did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at column {pos}: {message}")]
pub struct ParseError {
    /// 1-based column in the text.
    pub pos: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(String),
    Str(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    And,
    Or,
    Not,
    Cmp(Op),
}

fn lex(text: &str) -> Result<Vec<(usize, Tok)>, ParseError> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let err = |pos: usize, m: &str| ParseError {
        pos: pos + 1,
        message: m.to_string(),
    };
    while i < cs.len() {
        let c = cs[i];
        let start = i;
        match c {
            c if c.is_whitespace() => {
                i += 1;
            }
            '(' => {
                out.push((start, Tok::LParen));
                i += 1;
            }
            ')' => {
                out.push((start, Tok::RParen));
                i += 1;
            }
            '[' => {
                out.push((start, Tok::LBracket));
                i += 1;
            }
            ']' => {
                out.push((start, Tok::RBracket));
                i += 1;
            }
            ',' => {
                out.push((start, Tok::Comma));
                i += 1;
            }
            '&' if cs.get(i + 1) == Some(&'&') => {
                out.push((start, Tok::And));
                i += 2;
            }
            '|' if cs.get(i + 1) == Some(&'|') => {
                out.push((start, Tok::Or));
                i += 2;
            }
            '&' | '|' => return Err(err(i, "use && and || (doubled)")),
            '!' if cs.get(i + 1) == Some(&'=') => {
                out.push((start, Tok::Cmp(Op::Ne)));
                i += 2;
            }
            '!' => {
                out.push((start, Tok::Not));
                i += 1;
            }
            '=' if cs.get(i + 1) == Some(&'=') => {
                out.push((start, Tok::Cmp(Op::Eq)));
                i += 2;
            }
            '=' => return Err(err(i, "use == to compare")),
            '<' | '>' => {
                let eq = cs.get(i + 1) == Some(&'=');
                let op = match (c, eq) {
                    ('<', false) => Op::Lt,
                    ('<', true) => Op::Le,
                    ('>', false) => Op::Gt,
                    _ => Op::Ge,
                };
                out.push((start, Tok::Cmp(op)));
                i += if eq { 2 } else { 1 };
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    match cs.get(i) {
                        None => return Err(err(start, "unterminated string")),
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some('\\') if cs.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                out.push((start, Tok::Str(s)));
            }
            c if c.is_ascii_digit()
                || (c == '-' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                i += 1;
                // `.` is allowed so `1.5e18` reaches the compiler, which explains that fractions are not supported.
                while i < cs.len()
                    && (cs[i].is_ascii_alphanumeric() || cs[i] == '_' || cs[i] == '.')
                {
                    i += 1;
                }
                out.push((start, Tok::Num(cs[start..i].iter().collect())));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                i += 1;
                while i < cs.len()
                    && (cs[i].is_ascii_alphanumeric() || cs[i] == '_' || cs[i] == '.')
                {
                    i += 1;
                }
                let w: String = cs[start..i].iter().collect();
                out.push((
                    start,
                    match w.as_str() {
                        "and" => Tok::And,
                        "or" => Tok::Or,
                        "not" => Tok::Not,
                        _ => Tok::Ident(w),
                    },
                ));
            }
            other => return Err(err(i, &format!("unexpected character '{other}'"))),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    at: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(_, t)| t)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.at).map_or(self.end, |(p, _)| *p) + 1
    }
    fn err<T>(&self, m: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            pos: self.pos(),
            message: m.into(),
        })
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut args = vec![self.and()?];
        while self.eat(&Tok::Or) {
            args.push(self.and()?);
        }
        Ok(if args.len() == 1 {
            args.remove(0)
        } else {
            Expr::Or { args }
        })
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut args = vec![self.not()?];
        while self.eat(&Tok::And) {
            args.push(self.not()?);
        }
        Ok(if args.len() == 1 {
            args.remove(0)
        } else {
            Expr::And { args }
        })
    }

    fn not(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Tok::Not) {
            return Ok(Expr::Not {
                arg: Box::new(self.not()?),
            });
        }
        self.atom()
    }

    fn literal(&mut self) -> Result<Literal, ParseError> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.at += 1;
                Ok(Literal::Raw(n))
            }
            Some(Tok::Str(s)) => {
                self.at += 1;
                Ok(Literal::Str(s))
            }
            Some(Tok::Ident(w)) => {
                self.at += 1;
                Ok(match w.as_str() {
                    "true" => Literal::Bool(true),
                    "false" => Literal::Bool(false),
                    _ => Literal::Str(w),
                })
            }
            _ => self.err("expected a value (number, 0x… hex, word or \"text\")"),
        }
    }

    fn atom(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Tok::LParen) {
            let e = self.or()?;
            if !self.eat(&Tok::RParen) {
                return self.err("expected ')'");
            }
            return Ok(e);
        }
        let Some(Tok::Ident(w)) = self.peek().cloned() else {
            return self.err("expected a field name, '(' or '!'");
        };
        self.at += 1;
        if w == "true" && !matches!(self.peek(), Some(Tok::Cmp(_))) {
            return Ok(Expr::True);
        }
        if w == "set" && self.eat(&Tok::LParen) {
            let Some(Tok::Ident(field)) = self.peek().cloned() else {
                return self.err("expected a field name inside set(...)");
            };
            self.at += 1;
            if !self.eat(&Tok::RParen) {
                return self.err("expected ')'");
            }
            return Ok(Expr::Set { field });
        }
        match self.peek().cloned() {
            Some(Tok::Cmp(op)) => {
                self.at += 1;
                Ok(Expr::Cmp {
                    field: w,
                    cmp: op,
                    value: self.literal()?,
                })
            }
            Some(Tok::Ident(k)) if k == "in" => {
                self.at += 1;
                if !self.eat(&Tok::LBracket) {
                    return self.err("expected '[' after 'in'");
                }
                let mut values = Vec::new();
                if !self.eat(&Tok::RBracket) {
                    loop {
                        values.push(self.literal()?);
                        if self.eat(&Tok::RBracket) {
                            break;
                        }
                        if !self.eat(&Tok::Comma) {
                            return self.err("expected ',' or ']'");
                        }
                    }
                }
                Ok(Expr::In { field: w, values })
            }
            Some(Tok::Ident(k)) if k == "contains" => {
                self.at += 1;
                Ok(Expr::Contains {
                    field: w,
                    value: self.literal()?,
                })
            }
            _ => self.err(format!(
                "after '{w}' expected a comparison (==, !=, <, <=, >, >=), 'in [...]' or 'contains'"
            )),
        }
    }
}

/// Parse rule text into an expression.
pub fn parse(text: &str) -> Result<Expr, ParseError> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err(ParseError {
            pos: 1,
            message: "the rule is empty (write `true` to match everything)".into(),
        });
    }
    let mut p = Parser {
        toks,
        at: 0,
        end: text.chars().count(),
    };
    let e = p.or()?;
    if p.at != p.toks.len() {
        return p.err("unexpected input after the expression");
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(text: &str) -> Expr {
        parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn precedence_is_not_then_and_then_or() {
        assert_eq!(
            p("a == 1 || b == 2 && c == 3").to_string(),
            "a == 1 || b == 2 && c == 3"
        );
        assert!(
            matches!(p("a == 1 || b == 2 && c == 3"), Expr::Or { args } if args.len() == 2 && matches!(args[1], Expr::And { .. }))
        );
        assert!(
            matches!(p("!a == 1 && b == 2"), Expr::And { args } if matches!(args[0], Expr::Not { .. }))
        );
        assert!(matches!(
            p("(a == 1 || b == 2) && c == 3"),
            Expr::And { .. }
        ));
        // Words and symbols are the same operators.
        assert_eq!(
            p("a == 1 and b == 2 or not c == 3"),
            p("a == 1 && b == 2 || ! c == 3")
        );
    }

    #[test]
    fn every_form_parses() {
        assert_eq!(p("true"), Expr::True);
        assert!(
            matches!(p("selector in [0xa415bcad, 0x573ade81]"), Expr::In { values, .. } if values.len() == 2)
        );
        assert!(matches!(
            p("via contains multicall3"),
            Expr::Contains { .. }
        ));
        assert!(matches!(p("set(swap.pool)"), Expr::Set { field } if field == "swap.pool"));
        assert!(
            matches!(p("known contains \"Universal Router\""), Expr::Contains { value: Literal::Str(s), .. } if s == "Universal Router")
        );
        assert!(
            matches!(p("liq.tick_lower >= -887_220"), Expr::Cmp { value: Literal::Raw(r), .. } if r == "-887_220")
        );
        assert!(matches!(
            p("swap.amount_in >= 50000e6"),
            Expr::Cmp { cmp: Op::Ge, .. }
        ));
        assert!(matches!(
            p("a == true"),
            Expr::Cmp {
                value: Literal::Bool(true),
                ..
            }
        ));
    }

    #[test]
    fn errors_say_where_and_what() {
        let cases = [
            ("", 1, "empty"),
            ("a = 1", 3, "use =="),
            ("a == ", 6, "expected a value"),
            ("a == 1 &", 8, "doubled"),
            ("(a == 1", 8, "expected ')'"),
            ("a == 1 b == 2", 8, "unexpected input"),
            ("a", 2, "expected a comparison"),
            ("a in 3", 6, "expected '['"),
            ("a in [1 2]", 9, "expected ',' or ']'"),
            ("a == \"x", 6, "unterminated"),
            ("a == 1 # c", 8, "unexpected character"),
        ];
        for (text, pos, needle) in cases {
            let e = parse(text).unwrap_err();
            assert!(e.message.contains(needle), "{text:?}: {e}");
            assert_eq!(e.pos, pos, "{text:?}: {e}");
        }
    }

    #[test]
    fn keyword_like_text_is_quoted_so_it_survives_a_round_trip() {
        for w in ["true", "in", "contains", "set", "9lives", "has space", ""] {
            let e = Expr::Cmp {
                field: "known".into(),
                cmp: Op::Eq,
                value: Literal::Str(w.into()),
            };
            assert_eq!(parse(&e.to_string()).unwrap(), e, "{w:?}");
        }
    }

    fn lit() -> impl Strategy<Value = Literal> {
        prop_oneof![
            "0x[0-9a-f]{2,8}".prop_map(Literal::Raw),
            "[0-9]{1,6}(e[0-9])?".prop_map(Literal::Raw),
            "[a-zA-Z_][a-zA-Z0-9_ ]{0,12}".prop_map(Literal::Str),
            any::<bool>().prop_map(Literal::Bool),
        ]
    }

    fn field() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("kind".to_string()),
            Just("swap.amount_in".to_string()),
            Just("selector".to_string()),
            Just("via".to_string())
        ]
    }

    fn expr() -> impl Strategy<Value = Expr> {
        let leaf = prop_oneof![
            Just(Expr::True),
            (
                field(),
                prop_oneof![
                    Just(Op::Eq),
                    Just(Op::Ne),
                    Just(Op::Lt),
                    Just(Op::Le),
                    Just(Op::Gt),
                    Just(Op::Ge)
                ],
                lit()
            )
                .prop_map(|(field, cmp, value)| Expr::Cmp { field, cmp, value }),
            (field(), prop::collection::vec(lit(), 1..4))
                .prop_map(|(field, values)| Expr::In { field, values }),
            (field(), lit()).prop_map(|(field, value)| Expr::Contains { field, value }),
            field().prop_map(|field| Expr::Set { field }),
        ];
        leaf.prop_recursive(4, 24, 3, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 2..4).prop_map(|args| Expr::And { args }),
                prop::collection::vec(inner.clone(), 2..4).prop_map(|args| Expr::Or { args }),
                inner.prop_map(|e| Expr::Not { arg: Box::new(e) }),
            ]
        })
    }

    proptest! {
        /// Printing then parsing then printing changes nothing: the text is a faithful form of the data.
        #[test]
        fn printing_is_stable_through_a_parse(e in expr()) {
            let text = e.to_string();
            let back = parse(&text).unwrap_or_else(|err| panic!("{text}: {err}"));
            prop_assert_eq!(back.to_string(), text);
        }

        /// The parser never panics, whatever it is given.
        #[test]
        fn garbage_never_panics(s in ".{0,60}") {
            let _ = parse(&s);
        }
    }
}
