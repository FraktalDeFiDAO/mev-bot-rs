//! Starter rule files. `seqscope rules template NAME` prints one; a user edits it from there.

pub const NAMES: [(&str, &str); 6] = [
    (
        "aave-lending",
        "Aave V3 supply, borrow, repay, withdraw and liquidationCall, one rule each",
    ),
    ("dex-swaps", "every decoded swap, big ones flagged"),
    ("liquidity", "every decoded liquidity add and remove"),
    ("liquidations", "every decoded liquidation, as alerts"),
    (
        "gaps",
        "calls to identified contracts nothing decodes yet, and busy unidentified destinations",
    ),
    ("everything", "all calls, kept at 1 in 100, to look around"),
];

pub fn get(name: &str) -> Option<&'static str> {
    Some(match name {
        "aave-lending" => AAVE,
        "dex-swaps" => SWAPS,
        "liquidity" => LIQUIDITY,
        "liquidations" => LIQUIDATIONS,
        "gaps" => GAPS,
        "everything" => EVERYTHING,
        _ => return None,
    })
}

const AAVE: &str = r#"# Aave V3 lending actions on Arbitrum. Selectors are the Pool's supply, borrow, repay, withdraw, liquidationCall.
version = 1

[[profile]]
name = "aave-lending"
mode = "all"

[[profile.rule]]
name = "supply"
when = "protocol == aave_v3 && selector == 0x617ba037"
tag = "supply"

[[profile.rule]]
name = "borrow"
when = "protocol == aave_v3 && selector == 0xa415bcad"
tag = "borrow"

[[profile.rule]]
name = "repay"
when = "protocol == aave_v3 && selector == 0x573ade81"
tag = "repay"

[[profile.rule]]
name = "withdraw"
when = "protocol == aave_v3 && selector == 0x69328dec"
tag = "withdraw"

[[profile.rule]]
name = "liquidation"
when = "protocol == aave_v3 && selector == 0x00a718a9"
action = "alert"
tag = "liquidation"
"#;

const SWAPS: &str = r#"# Every swap the decoder reads, and an alert for large ones (amount_in is in the input token's smallest unit).
version = 1

[[profile]]
name = "swaps"
mode = "all"

[[profile.rule]]
name = "all swaps"
when = "kind == swap"

[[profile.rule]]
name = "large swaps"
when = "kind == swap && swap.amount_in >= 1e21"
action = "alert"
tag = "large"
"#;

const LIQUIDITY: &str = r#"# Liquidity added and removed (Uniswap V2/V3/V4 position managers and routers).
version = 1

[[profile]]
name = "liquidity"
mode = "all"

[[profile.rule]]
name = "adds"
when = "kind == liquidity && liq.action == add"
tag = "add"

[[profile.rule]]
name = "removes"
when = "kind == liquidity && liq.action == remove"
tag = "remove"
"#;

const LIQUIDATIONS: &str = r#"# Liquidations of any protocol the decoder reads, as alerts, with the calldata kept for later study.
version = 1

[[profile]]
name = "liquidations"
mode = "all"

[[profile.rule]]
name = "liquidations"
when = "kind == liquidation"
action = "alert"
tag = "liquidation"

[[profile.rule]]
name = "keep the calldata"
when = "kind == liquidation"
action = "raw"
"#;

const GAPS: &str = r#"# What the decoder cannot read yet: identified contracts with unread calls, then everything else with real calldata.
version = 1

[[profile]]
name = "gaps"
mode = "first"

[[profile.rule]]
name = "identified but unread"
when = "kind == undecoded_known"
tag = "known"

[[profile.rule]]
name = "unidentified"
when = "kind == unknown && len >= 4"
tag = "unknown"
sample = 10
"#;

const EVERYTHING: &str = r#"# A look around: every call, one in a hundred (chosen by transaction hash, so reruns agree).
version = 1

[[profile]]
name = "sample"
mode = "all"

[[profile.rule]]
name = "one in a hundred"
when = "true"
sample = 100
"#;
