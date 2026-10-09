//! A rule in plain English. The same expression the engine runs, said the way a person would: "it is a swap and it sells
//! at least 10,000 USDC". Used everywhere a rule is shown so nobody has to read `swap.amount_in >= 10000e6`.

use alloy::primitives::{Address, U256};

use crate::{
    compile::parse_uint,
    expr::{Expr, Literal},
    field::FieldId,
    names::{
        address_name, group_thousands, human_amount, protocol_label, selector_name, short,
        token_by_address, wrapper_label,
    },
    value::Op,
};

/// A field as a person names it.
pub fn label(f: FieldId) -> &'static str {
    match f {
        FieldId::Hash => "transaction hash",
        FieldId::Seq => "sequence number",
        FieldId::From => "sender",
        FieldId::TxTo => "transaction destination",
        FieldId::TxSelector => "transaction function",
        FieldId::TxType => "transaction type",
        FieldId::Value => "ETH sent (in wei)",
        FieldId::GasLimit => "gas limit",
        FieldId::MaxFee => "max fee per gas (in wei)",
        FieldId::Tip => "priority fee (in wei)",
        FieldId::Nonce => "sender's nonce",
        FieldId::To => "destination",
        FieldId::Selector => "function",
        FieldId::Input => "call data",
        FieldId::Len => "call data length (bytes)",
        FieldId::Via => "wrapper",
        FieldId::Depth => "wrapper depth",
        FieldId::Known => "contract name",
        FieldId::Protocol => "protocol",
        FieldId::Kind => "kind",
        FieldId::SwapPool => "pool",
        FieldId::SwapTokenIn => "token sold",
        FieldId::SwapAmountIn => "amount sold (raw units)",
        FieldId::SwapHops => "route length (hops)",
        FieldId::SwapLimit => "price limit",
        FieldId::LiqAction => "liquidity action",
        FieldId::LiqPool => "pool",
        FieldId::LiqTokenId => "position id",
        FieldId::LiqTickLower => "lower tick",
        FieldId::LiqTickUpper => "upper tick",
        FieldId::LiqAmount0 => "token0 amount",
        FieldId::LiqAmount1 => "token1 amount",
        FieldId::LiqLiquidity => "liquidity units",
        FieldId::LiquidationBorrower => "borrower",
        FieldId::LiquidationCollateral => "collateral asset",
        FieldId::LiquidationDebt => "debt asset",
        FieldId::LiquidationAmount => "liquidation amount",
        FieldId::LiquidationMarket => "market",
    }
}

fn op_words(op: Op) -> &'static str {
    match op {
        Op::Eq => "exactly",
        Op::Ne => "anything but",
        Op::Lt => "less than",
        Op::Le => "at most",
        Op::Gt => "more than",
        Op::Ge => "at least",
    }
}

fn raw(l: &Literal) -> String {
    match l {
        Literal::Raw(r) => r.clone(),
        Literal::Str(s) => s.clone(),
        Literal::Bool(b) => b.to_string(),
    }
}

fn as_address(l: &Literal) -> Option<Address> {
    match l {
        Literal::Raw(r) => r.parse().ok(),
        _ => None,
    }
}

/// A number as written (`1e21`, `0xff`) in plain digits with separators; unreadable input is returned as typed.
fn number(l: &Literal) -> String {
    match l {
        Literal::Raw(r) => match parse_uint(r.trim_start_matches('-')) {
            Ok(n) => format!(
                "{}{}",
                if r.starts_with('-') { "-" } else { "" },
                group_thousands(&n.to_string())
            ),
            Err(_) => r.clone(),
        },
        other => raw(other),
    }
}

fn value_text(field: FieldId, l: &Literal) -> String {
    match field {
        FieldId::To
        | FieldId::TxTo
        | FieldId::From
        | FieldId::SwapPool
        | FieldId::LiqPool
        | FieldId::SwapTokenIn
        | FieldId::LiquidationBorrower
        | FieldId::LiquidationCollateral
        | FieldId::LiquidationDebt => as_address(l).map(address_name).unwrap_or_else(|| raw(l)),
        FieldId::Selector | FieldId::TxSelector => {
            let r = raw(l);
            selector_name(&r).map(str::to_string).unwrap_or(r)
        }
        FieldId::Protocol => protocol_label(&raw(l)).to_string(),
        FieldId::Via => wrapper_label(&raw(l)).to_string(),
        FieldId::Hash | FieldId::LiquidationMarket => short(&raw(l)),
        FieldId::Kind | FieldId::LiqAction | FieldId::Known => raw(l),
        _ => number(l),
    }
}

fn kind_phrase(k: &str, negate: bool) -> String {
    let is = match k {
        "swap" => "a swap",
        "liquidity" => "a liquidity change (added or removed)",
        "liquidation" => "a liquidation",
        "undecoded_known" => "a call to a known contract that we cannot read yet",
        "unknown" => "a call to a contract we do not know",
        other => other,
    };
    format!("it is {}{is}", if negate { "not " } else { "" })
}

/// One comparison in words.
fn leaf(field: &str, op: Op, l: &Literal) -> String {
    let Some(f) = FieldId::from_name(field) else {
        return format!("{field} {} {}", op.symbol(), raw(l));
    };
    let neg = op == Op::Ne;
    let v = value_text(f, l);
    match f {
        FieldId::Kind => kind_phrase(&raw(l), neg),
        FieldId::Protocol => format!("it is {}on {v}", if neg { "not " } else { "" }),
        FieldId::To => format!("it {} {v}", if neg { "does not go to" } else { "goes to" }),
        FieldId::TxTo => format!(
            "the transaction {} {v}",
            if neg { "is not sent to" } else { "is sent to" }
        ),
        FieldId::From => format!(
            "it {} {v}",
            if neg { "is not sent by" } else { "is sent by" }
        ),
        FieldId::Selector => format!("the function {} {v}", if neg { "is not" } else { "is" }),
        FieldId::TxSelector => format!(
            "the transaction's function {} {v}",
            if neg { "is not" } else { "is" }
        ),
        FieldId::SwapPool | FieldId::LiqPool => {
            format!("the pool {} {v}", if neg { "is not" } else { "is" })
        }
        FieldId::SwapTokenIn => format!("it {} {v}", if neg { "does not sell" } else { "sells" }),
        FieldId::LiqAction => match raw(l).as_str() {
            "add" => format!("liquidity is {}being added", if neg { "not " } else { "" }),
            "remove" => format!(
                "liquidity is {}being removed",
                if neg { "not " } else { "" }
            ),
            other => format!(
                "the liquidity action {} {other}",
                if neg { "is not" } else { "is" }
            ),
        },
        FieldId::Via => format!("it is {}inside {v}", if neg { "not " } else { "" }),
        FieldId::Depth => match (op, raw(l).as_str()) {
            (Op::Eq, "0") | (Op::Le, "0") | (Op::Lt, "1") => {
                "it is called directly (not inside a wrapper)".to_string()
            }
            (Op::Gt, "0") | (Op::Ge, "1") => "it is inside a wrapper".to_string(),
            _ => format!("the wrapper depth is {} {v}", op_words(op)),
        },
        FieldId::Value => {
            let eth = match l {
                Literal::Raw(r) => parse_uint(r).ok().map(|n| human_amount(&n.to_string(), 18)),
                _ => None,
            };
            match eth {
                Some(e) => format!("it sends {} {e} ETH", op_words(op)),
                None => format!("the ETH sent is {} {v}", op_words(op)),
            }
        }
        FieldId::Known => match op {
            Op::Eq => format!("the contract is named \"{v}\""),
            _ => format!("the contract name is not \"{v}\""),
        },
        FieldId::SwapAmountIn => format!(
            "the amount sold is {} {v} (in the token's smallest unit)",
            op_words(op)
        ),
        FieldId::SwapHops => format!(
            "the route has {} {v} hop{}",
            op_words(op),
            if v == "1" { "" } else { "s" }
        ),
        _ => format!("the {} is {} {v}", label(f), op_words(op)),
    }
}

fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

fn clause(e: &Expr) -> String {
    match e {
        Expr::True => "anything".to_string(),
        Expr::Cmp { field, cmp, value } => leaf(field, *cmp, value),
        Expr::In { field, values } => {
            let names: Vec<String> = match FieldId::from_name(field) {
                Some(f) => values.iter().map(|v| value_text(f, v)).collect(),
                None => values.iter().map(raw).collect(),
            };
            let what = match FieldId::from_name(field) {
                Some(FieldId::Kind) => "it is one of these".to_string(),
                Some(FieldId::Protocol) => "it is on one of".to_string(),
                Some(FieldId::To) => "it goes to one of".to_string(),
                Some(FieldId::Selector) => "the function is one of".to_string(),
                Some(f) => format!("the {} is one of", label(f)),
                None => format!("{field} is one of"),
            };
            format!("{what}: {}", names.join(", "))
        }
        Expr::Contains { field, value } => match FieldId::from_name(field) {
            Some(FieldId::Via) => format!("it is inside {}", wrapper_label(&raw(value))),
            Some(FieldId::Input) => match as_address(value) {
                Some(a) => format!("the call data mentions {}", address_name(a)),
                None => format!("the call data contains {}", short(&raw(value))),
            },
            Some(FieldId::Known) => format!("the contract name contains \"{}\"", raw(value)),
            _ => format!("{field} contains {}", raw(value)),
        },
        Expr::Set { field } => match FieldId::from_name(field) {
            Some(FieldId::Via) => "it is inside a wrapper".to_string(),
            Some(f) => format!("the {} is known", label(f)),
            None => format!("{field} is present"),
        },
        Expr::Not { arg } => match &**arg {
            Expr::Cmp { .. } | Expr::In { .. } | Expr::Contains { .. } | Expr::Set { .. } => {
                format!("it is not true that {}", clause(arg))
            }
            other => format!("it is not true that ({})", clause(other)),
        },
        Expr::And { args } => and_clauses(args),
        Expr::Or { args } => {
            let parts: Vec<String> = args.iter().map(paren_if_compound).collect();
            match parts.len() {
                0 => String::new(),
                1 => parts[0].clone(),
                2 => format!("either {} or {}", parts[0], parts[1]),
                _ => format!("any one of: {}", parts.join("; ")),
            }
        }
    }
}

fn paren_if_compound(e: &Expr) -> String {
    match e {
        Expr::And { args } | Expr::Or { args } if args.len() > 1 => format!("({})", clause(e)),
        _ => clause(e),
    }
}

/// "It sells at least 10,000 USDC" from a token-sold test and an amount test side by side.
fn sells_phrase(token: Address, op: Op, amount: &Literal) -> Option<String> {
    let t = token_by_address(token)?;
    let Literal::Raw(r) = amount else { return None };
    let n: U256 = parse_uint(r).ok()?;
    Some(format!(
        "it sells {} {} {}",
        op_words(op),
        human_amount(&n.to_string(), t.decimals),
        t.symbol
    ))
}

fn and_clauses(args: &[Expr]) -> String {
    let mut used = vec![false; args.len()];
    let mut out: Vec<String> = Vec::new();
    // Pair "token sold is T" with "amount sold ..." so the amount is said in that token.
    for (i, a) in args.iter().enumerate() {
        if let Expr::Cmp {
            field,
            cmp: Op::Eq,
            value,
        } = a
            && field == "swap.token_in"
            && let Some(addr) = as_address(value)
            && token_by_address(addr).is_some()
            && let Some((j, op, amt)) = args.iter().enumerate().find_map(|(j, b)| match b {
                Expr::Cmp { field, cmp, value } if field == "swap.amount_in" && !used[j] => {
                    Some((j, *cmp, value))
                }
                _ => None,
            })
            && let Some(p) = sells_phrase(addr, op, amt)
        {
            out.push(p);
            used[i] = true;
            used[j] = true;
        }
    }
    let mut rest: Vec<String> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if !used[i] {
            rest.push(paren_if_compound(a));
        }
    }
    rest.extend(out);
    join_and(&rest)
}

/// "Catches every call" / "Catches a call when it is a swap and it sells at least 10,000 USDC".
pub fn explain(e: &Expr) -> String {
    if matches!(e, Expr::True) {
        return "Catches every call.".to_string();
    }
    format!("Catches a call when {}.", clause(e))
}

/// Parse rule text and explain it; the parse error otherwise.
pub fn explain_text(when: &str) -> Result<String, crate::expr::ParseError> {
    crate::expr::parse(when).map(|e| explain(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(when: &str) -> String {
        explain_text(when).unwrap_or_else(|e| panic!("{when}: {e}"))
    }

    #[test]
    fn the_basics_read_like_a_sentence() {
        assert_eq!(x("true"), "Catches every call.");
        assert_eq!(x("kind == swap"), "Catches a call when it is a swap.");
        assert_eq!(
            x("kind == liquidation"),
            "Catches a call when it is a liquidation."
        );
        assert_eq!(
            x("kind != unknown"),
            "Catches a call when it is not a call to a contract we do not know."
        );
        assert_eq!(
            x("protocol == aave_v3"),
            "Catches a call when it is on Aave V3."
        );
        assert_eq!(
            x("protocol == uniswap_v4 && kind == swap"),
            "Catches a call when it is on Uniswap V4 and it is a swap."
        );
    }

    #[test]
    fn a_token_and_an_amount_are_said_together_in_human_units() {
        assert_eq!(
            x(
                "kind == swap && swap.token_in == 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 && swap.amount_in >= 10000e6"
            ),
            "Catches a call when it is a swap and it sells at least 10,000 USDC."
        );
        assert_eq!(
            x(
                "swap.amount_in > 5e17 && swap.token_in == 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"
            ),
            "Catches a call when it sells more than 0.5 WETH."
        );
        // Without a known token the amount is honest about being a raw number.
        assert_eq!(
            x("swap.amount_in >= 1e21"),
            "Catches a call when the amount sold is at least 1,000,000,000,000,000,000,000 (in the token's smallest unit)."
        );
    }

    #[test]
    fn addresses_functions_and_wrappers_use_names_when_there_are_any() {
        assert_eq!(
            x("selector == 0xa415bcad"),
            "Catches a call when the function is Aave: borrow."
        );
        assert_eq!(
            x("selector in [0x617ba037, 0x573ade81]"),
            "Catches a call when the function is one of: Aave: supply, Aave: repay."
        );
        assert_eq!(
            x("to == 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45"),
            "Catches a call when it goes to Uniswap SwapRouter02."
        );
        assert_eq!(
            x("to == 0x0000000000000000000000000000000000000001"),
            "Catches a call when it goes to 0x000000…0001."
        );
        assert_eq!(
            x("via contains multicall3"),
            "Catches a call when it is inside a Multicall3 call."
        );
        assert_eq!(
            x("depth == 0"),
            "Catches a call when it is called directly (not inside a wrapper)."
        );
        assert_eq!(x("set(via)"), "Catches a call when it is inside a wrapper.");
        assert_eq!(
            x("liq.action == add"),
            "Catches a call when liquidity is being added."
        );
        assert_eq!(
            x("known contains \"universal router\""),
            "Catches a call when the contract name contains \"universal router\"."
        );
    }

    #[test]
    fn combinations_say_either_or_and_not() {
        assert_eq!(
            x("kind == swap || kind == liquidity"),
            "Catches a call when either it is a swap or it is a liquidity change (added or removed)."
        );
        assert_eq!(
            x("kind == swap || kind == liquidity || kind == liquidation"),
            "Catches a call when any one of: it is a swap; it is a liquidity change (added or removed); it is a liquidation."
        );
        assert_eq!(
            x("!(kind == swap)"),
            "Catches a call when it is not true that it is a swap."
        );
        assert_eq!(
            x("kind == unknown && (protocol == aave_v3 || protocol == morpho_blue)"),
            "Catches a call when it is a call to a contract we do not know and (either it is on Aave V3 or it is on Morpho Blue)."
        );
    }

    #[test]
    fn plain_numbers_and_eth_read_naturally() {
        assert_eq!(
            x("value >= 1e18"),
            "Catches a call when it sends at least 1 ETH."
        );
        assert_eq!(
            x("gas_limit > 500000"),
            "Catches a call when the gas limit is more than 500,000."
        );
        assert_eq!(
            x("swap.hops >= 2"),
            "Catches a call when the route has at least 2 hops."
        );
        assert_eq!(
            x("len >= 4"),
            "Catches a call when the call data length (bytes) is at least 4."
        );
    }

    #[test]
    fn every_field_has_a_label_and_every_rule_the_engine_accepts_can_be_explained() {
        for f in FieldId::ALL {
            assert!(!label(*f).is_empty());
        }
        for t in [
            "aave-lending",
            "dex-swaps",
            "liquidity",
            "liquidations",
            "gaps",
            "everything",
        ] {
            let set = crate::engine::RuleSet::from_toml(crate::templates::get(t).unwrap()).unwrap();
            for p in &set.profile {
                for r in &p.rule {
                    let s = x(&r.when);
                    assert!(
                        s.starts_with("Catches") && s.ends_with('.'),
                        "{t}/{}: {s}",
                        r.name
                    );
                }
            }
        }
        assert!(
            explain_text("kind ==").is_err(),
            "a bad rule is an error, not a sentence"
        );
    }
}
