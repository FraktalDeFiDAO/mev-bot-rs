//! Turns a rule expression into a checked, typed tree and evaluates it. Every mistake a user can make (a misspelt
//! field, a hex value of the wrong length, `>` on an address) is reported here, by name, before the rule can run.

use alloy::primitives::{Address, B256, U256};

use crate::{
    expr::{Expr, Literal},
    field::{Ctx, FieldId},
    value::{Lit, Op, Ty, compare},
};

/// Why a rule could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CompileError(pub String);

fn bad<T>(m: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError(m.into()))
}

/// Closest known field names to a misspelt one, for the error message.
fn suggest(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mut near: Vec<&str> = FieldId::ALL
        .iter()
        .map(|f| f.name())
        .filter(|n| {
            n.contains(&lower)
                || lower.contains(n)
                || n.rsplit('.').next() == lower.rsplit('.').next()
        })
        .take(4)
        .collect();
    near.dedup();
    if near.is_empty() {
        String::new()
    } else {
        format!("; did you mean {}?", near.join(", "))
    }
}

fn field(name: &str) -> Result<FieldId, CompileError> {
    FieldId::from_name(name)
        .ok_or_else(|| CompileError(format!("unknown field '{name}'{}", suggest(name))))
}

/// `5e6`, `1_000`, `0xff`, `-60`: a number as written.
pub(crate) fn parse_uint(raw: &str) -> Result<U256, CompileError> {
    let r: String = raw.chars().filter(|c| *c != '_').collect();
    if let Some(h) = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X")) {
        return U256::from_str_radix(h, 16)
            .or_else(|_| bad(format!("'{raw}' is not a valid hex number")));
    }
    let (mant, exp) = match r.split_once(['e', 'E']) {
        Some((m, e)) => (
            m,
            e.parse::<u32>()
                .or_else(|_| bad(format!("bad exponent in '{raw}'")))?,
        ),
        None => (r.as_str(), 0),
    };
    if mant.is_empty() || !mant.chars().all(|c| c.is_ascii_digit()) {
        return bad(format!(
            "'{raw}' is not a whole number (use 5e6 for 5,000,000; fractions are not supported)"
        ));
    }
    let m = U256::from_str_radix(mant, 10).or_else(|_| bad(format!("'{raw}' is too large")))?;
    let scale = U256::from(10u8)
        .checked_pow(U256::from(exp))
        .ok_or_else(|| CompileError(format!("'{raw}' is too large")))?;
    m.checked_mul(scale)
        .ok_or_else(|| CompileError(format!("'{raw}' is too large")))
}

fn hex_exact(raw: &str, bytes: usize, what: &str) -> Result<Vec<u8>, CompileError> {
    let h = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    let v = alloy::hex::decode(h).map_err(|_| CompileError(format!("'{raw}' is not hex")))?;
    if v.len() != bytes {
        return bad(format!(
            "{what} must be {bytes} bytes ({} hex digits), '{raw}' has {}",
            bytes * 2,
            v.len() * 2
        ));
    }
    Ok(v)
}

/// Values a text field accepts, where the set is closed.
fn closed_set(f: FieldId) -> Option<Vec<&'static str>> {
    match f {
        FieldId::Kind => Some(vec![
            "swap",
            "liquidity",
            "liquidation",
            "undecoded_known",
            "unknown",
        ]),
        FieldId::LiqAction => Some(vec!["add", "remove"]),
        FieldId::Protocol => Some(crate::catalogue::protocol_ids()),
        FieldId::Via => Some(vec![
            "multicall3",
            "self_multicall",
            "account_execute",
            "entrypoint_v6",
            "entrypoint_v7",
        ]),
        _ => None,
    }
}

fn lit_for(f: FieldId, l: &Literal) -> Result<Lit, CompileError> {
    let name = f.name();
    match (f.ty(), l) {
        (Ty::Num, Literal::Raw(r)) => Ok(Lit::Num(parse_uint(r)?)),
        (Ty::Int, Literal::Raw(r)) => {
            let (neg, digits) = match r.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, r.as_str()),
            };
            let n = parse_uint(digits)?;
            let n = i64::try_from(n).or_else(|_| bad(format!("'{r}' is too large for {name}")))?;
            Ok(Lit::Int(if neg { -n } else { n }))
        }
        (Ty::Bool, Literal::Bool(b)) => Ok(Lit::Bool(*b)),
        (Ty::Addr, Literal::Raw(r)) => Ok(Lit::Addr(Address::from_slice(&hex_exact(
            r,
            20,
            "an address",
        )?))),
        (Ty::Sel, Literal::Raw(r)) => {
            let v = hex_exact(r, 4, "a selector")?;
            Ok(Lit::Sel([v[0], v[1], v[2], v[3]]))
        }
        (Ty::Hash, Literal::Raw(r)) => {
            Ok(Lit::Hash(B256::from_slice(&hex_exact(r, 32, "a hash")?)))
        }
        (Ty::Str | Ty::StrList, Literal::Str(s)) => {
            if let Some(set) = closed_set(f)
                && !set.contains(&s.as_str())
            {
                return bad(format!(
                    "'{s}' is not a value of {name}; choose one of: {}",
                    set.join(", ")
                ));
            }
            Ok(Lit::Str(s.clone()))
        }
        (Ty::Bytes, Literal::Raw(r)) => {
            let h = r.strip_prefix("0x").unwrap_or(r);
            if h.is_empty() || h.len() % 2 != 0 {
                return bad(format!(
                    "'{r}' must be whole bytes of hex (an even number of digits)"
                ));
            }
            Ok(Lit::Bytes(
                alloy::hex::decode(h).map_err(|_| CompileError(format!("'{r}' is not hex")))?,
            ))
        }
        (ty, lit) => bad(format!(
            "{name} is {ty:?}; the value {lit} does not fit (e.g. numbers for amounts, 0x… for addresses)"
        )),
    }
}

/// A checked rule expression.
#[derive(Debug, Clone)]
pub enum Node {
    True,
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Cmp(FieldId, Op, Lit),
    In(FieldId, Vec<Lit>),
    Contains(FieldId, Lit),
    Set(FieldId),
}

pub fn compile(e: &Expr) -> Result<Node, CompileError> {
    Ok(match e {
        Expr::True => Node::True,
        Expr::And { args } => Node::And(args.iter().map(compile).collect::<Result<_, _>>()?),
        Expr::Or { args } => Node::Or(args.iter().map(compile).collect::<Result<_, _>>()?),
        Expr::Not { arg } => Node::Not(Box::new(compile(arg)?)),
        Expr::Cmp {
            field: n,
            cmp,
            value,
        } => {
            let f = field(n)?;
            match f.ty() {
                Ty::StrList | Ty::Bytes => {
                    return bad(format!(
                        "{n} cannot be compared with {}; use `{n} contains …`",
                        cmp.symbol()
                    ));
                }
                Ty::Bool | Ty::Addr | Ty::Sel | Ty::Hash | Ty::Str if cmp.is_ordering() => {
                    return bad(format!(
                        "{n} is {:?}: only == and != apply (use < > on numbers)",
                        f.ty()
                    ));
                }
                _ => {}
            }
            Node::Cmp(f, *cmp, lit_for(f, value)?)
        }
        Expr::In { field: n, values } => {
            let f = field(n)?;
            if matches!(f.ty(), Ty::StrList | Ty::Bytes) {
                return bad(format!("{n} does not support `in`; use `{n} contains …`"));
            }
            if values.is_empty() {
                return bad(format!(
                    "`{n} in []` can never match; list at least one value"
                ));
            }
            Node::In(
                f,
                values
                    .iter()
                    .map(|v| lit_for(f, v))
                    .collect::<Result<_, _>>()?,
            )
        }
        Expr::Contains { field: n, value } => {
            let f = field(n)?;
            match f.ty() {
                Ty::StrList | Ty::Bytes | Ty::Str => Node::Contains(f, lit_for(f, value)?),
                _ => {
                    return bad(format!(
                        "`contains` works on via, input and text fields, not {n}"
                    ));
                }
            }
        }
        Expr::Set { field: n } => Node::Set(field(n)?),
    })
}

fn contains_str(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

impl Node {
    pub fn eval(&self, c: &Ctx<'_>) -> bool {
        match self {
            Node::True => true,
            Node::And(v) => v.iter().all(|n| n.eval(c)),
            Node::Or(v) => v.iter().any(|n| n.eval(c)),
            Node::Not(n) => !n.eval(c),
            Node::Cmp(f, op, l) => f.read(c).is_some_and(|v| compare(v, *op, l)),
            Node::In(f, ls) => f
                .read(c)
                .is_some_and(|v| ls.iter().any(|l| compare(v, Op::Eq, l))),
            Node::Contains(f, l) => match (f, l) {
                (FieldId::Via, Lit::Str(s)) => c.leaf.is_some_and(|x| x.via.iter().any(|w| w == s)),
                (FieldId::Input, Lit::Bytes(b)) => c
                    .leaf
                    .is_some_and(|x| x.input.windows(b.len()).any(|w| w == b.as_slice())),
                (_, Lit::Str(s)) => {
                    matches!(f.read(c), Some(crate::value::Val::Str(h)) if contains_str(h, s))
                }
                _ => false,
            },
            Node::Set(f) => match f {
                FieldId::Via => c.leaf.is_some_and(|x| !x.via.is_empty()),
                FieldId::Input => c.leaf.is_some_and(|x| !x.input.is_empty()),
                _ => f.read(c).is_some(),
            },
        }
    }
}
