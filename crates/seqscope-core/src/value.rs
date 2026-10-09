//! Typed values the rule engine compares. Borrowed or `Copy`, so evaluating a rule never allocates.

use alloy::primitives::{Address, B256, U256};
use std::cmp::Ordering;

/// What a field holds, and what a literal in a rule is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ty {
    /// Unsigned integer up to 2^256 (amounts, gas, nonces).
    Num,
    /// Signed integer (ticks).
    Int,
    Bool,
    /// 20-byte address.
    Addr,
    /// 4-byte function selector.
    Sel,
    /// 32-byte hash or id.
    Hash,
    Str,
    /// A list of strings (the wrapper path).
    StrList,
    /// Raw bytes (calldata): only `contains`.
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val<'a> {
    Num(U256),
    Int(i64),
    Bool(bool),
    Addr(Address),
    Sel([u8; 4]),
    Hash(B256),
    Str(&'a str),
}

impl Val<'_> {
    /// How the value reads in a stored row.
    pub fn render(&self) -> String {
        match self {
            Val::Num(n) => n.to_string(),
            Val::Int(i) => i.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::Addr(a) => format!("{a:#x}"),
            Val::Sel(s) => format!("0x{}", alloy::hex::encode(s)),
            Val::Hash(h) => format!("{h:#x}"),
            Val::Str(s) => (*s).to_string(),
        }
    }
}

/// A literal from a rule, already converted to the type of the field it is compared with.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Num(U256),
    Int(i64),
    Bool(bool),
    Addr(Address),
    Sel([u8; 4]),
    Hash(B256),
    Str(String),
    Bytes(Vec<u8>),
}

/// Comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Op {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
}

impl Op {
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }

    pub fn is_ordering(self) -> bool {
        !matches!(self, Op::Eq | Op::Ne)
    }

    fn holds(self, o: Ordering) -> bool {
        match self {
            Op::Eq => o == Ordering::Equal,
            Op::Ne => o != Ordering::Equal,
            Op::Lt => o == Ordering::Less,
            Op::Le => o != Ordering::Greater,
            Op::Gt => o == Ordering::Greater,
            Op::Ge => o != Ordering::Less,
        }
    }
}

/// `value op lit`. A mismatch of types is false (the compiler rules those out, so this is only a safety net).
pub fn compare(v: Val<'_>, op: Op, l: &Lit) -> bool {
    let ord = match (v, l) {
        (Val::Num(a), Lit::Num(b)) => a.cmp(b),
        (Val::Int(a), Lit::Int(b)) => a.cmp(b),
        (Val::Bool(a), Lit::Bool(b)) => a.cmp(b),
        (Val::Addr(a), Lit::Addr(b)) => a.cmp(b),
        (Val::Sel(a), Lit::Sel(b)) => a.cmp(b),
        (Val::Hash(a), Lit::Hash(b)) => a.cmp(b),
        (Val::Str(a), Lit::Str(b)) => a.cmp(b.as_str()),
        _ => return false,
    };
    op.holds(ord)
}
