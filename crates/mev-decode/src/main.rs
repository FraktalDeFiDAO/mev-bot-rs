//! `mev-decode`: decode one call, list the known event shapes, or stream the sequencer feed with every transaction
//! decoded. Nothing here needs the bot, a node or a key.
//!
//!   mev-decode calldata <to-address> <0x-input>
//!   mev-decode events
//!   mev-decode feed [--url wss://arb1.arbitrum.io/feed] [--limit N] [--decoded-only]

use alloy::primitives::{Address, U256};
use mev_decode::{
    Decoded, LiquidationIntent, LiquidityIntent, PoolRef, SwapIntent, events, known_contract,
};
use serde_json::{Value, json};

fn swap_json(s: &SwapIntent) -> Value {
    json!({
        "kind": "swap",
        "pool": match &s.pool {
            PoolRef::Address(a) => json!({ "address": format!("{a:#x}") }),
            PoolRef::Pair { token_a, token_b, fee } => json!({ "token_a": format!("{token_a:#x}"), "token_b": format!("{token_b:#x}"), "fee": fee }),
            PoolRef::PairV2 { token_a, token_b } => json!({ "v2_token_a": format!("{token_a:#x}"), "v2_token_b": format!("{token_b:#x}") }),
        },
        "token_in": format!("{:#x}", s.token_in),
        "amount_in": s.amount_in.to_string(),
        "sqrt_price_limit_x96": s.sqrt_limit_x96.map(|l: U256| l.to_string()),
        "hops": s.hops,
    })
}

fn liquidity_json(l: &LiquidityIntent) -> Value {
    json!({
        "kind": "liquidity",
        "protocol": l.protocol,
        "action": format!("{:?}", l.action).to_lowercase(),
        "pool": l.pool.as_ref().map(|p| match p {
            PoolRef::Address(a) => json!({ "address": format!("{a:#x}") }),
            PoolRef::Pair { token_a, token_b, fee } => json!({ "token_a": format!("{token_a:#x}"), "token_b": format!("{token_b:#x}"), "fee": fee }),
            PoolRef::PairV2 { token_a, token_b } => json!({ "v2_token_a": format!("{token_a:#x}"), "v2_token_b": format!("{token_b:#x}") }),
        }),
        "token_id": l.token_id.map(|t| t.to_string()),
        "tick_lower": l.tick_lower,
        "tick_upper": l.tick_upper,
        "amount0": l.amount0.map(|a| a.to_string()),
        "amount1": l.amount1.map(|a| a.to_string()),
        "liquidity": l.liquidity.map(|a| a.to_string()),
    })
}

fn liquidation_json(l: &LiquidationIntent) -> Value {
    json!({
        "kind": "liquidation",
        "protocol": l.protocol,
        "borrowers": l.borrowers.iter().map(|b| format!("{b:#x}")).collect::<Vec<_>>(),
        "collateral": l.collateral.map(|a| format!("{a:#x}")),
        "debt": l.debt.map(|a| format!("{a:#x}")),
        "amount": l.amount.map(|a| a.to_string()),
        "market": l.market.map(|m| format!("{m:#x}")),
    })
}

fn describe(to: Option<Address>, input: &[u8]) -> Value {
    let decoded = to.and_then(|t| mev_decode::decode_call(t, input));
    json!({
        "to": to.map(|a| format!("{a:#x}")),
        "selector": input.get(..4).map(alloy::hex::encode),
        "known_contract": to.and_then(known_contract).map(|c| json!({ "name": c.name, "decoded": c.decoded })),
        "decoded": decoded.map(|d| match d {
            Decoded::Swap(s) => swap_json(&s),
            Decoded::Liquidity(l) => liquidity_json(&l),
            Decoded::Liquidation(l) => liquidation_json(&l),
        }),
    })
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  mev-decode calldata <to> <0x-input>\n  mev-decode events\n  mev-decode feed [--url URL] [--limit N] [--decoded-only]"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("calldata") if args.len() == 3 => {
            let to: Address = args[1].parse().unwrap_or_else(|e| {
                eprintln!("bad address: {e}");
                std::process::exit(2)
            });
            let input = alloy::hex::decode(args[2].trim_start_matches("0x")).unwrap_or_else(|e| {
                eprintln!("bad hex: {e}");
                std::process::exit(2)
            });
            println!("{}", describe(Some(to), &input));
        }
        Some("events") => {
            for k in events::kinds() {
                println!(
                    "{}",
                    json!({ "topic0": format!("{:#x}", k.topic0), "name": k.name, "class": k.class.as_str(), "protocol": k.protocol })
                );
            }
        }
        Some("feed") => {
            let mut url = "wss://arb1.arbitrum.io/feed".to_string();
            let (mut limit, mut decoded_only) = (u64::MAX, false);
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--url" => url = it.next().cloned().unwrap_or_else(|| usage()),
                    "--limit" => {
                        limit = it
                            .next()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or_else(|| usage())
                    }
                    "--decoded-only" => decoded_only = true,
                    _ => usage(),
                }
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(mev_sequencer::run(url, tx));
            let mut shown = 0u64;
            while let Some(st) = rx.recv().await {
                let mut line = describe(st.to, &st.input);
                if decoded_only && line["decoded"].is_null() {
                    continue;
                }
                line["seq"] = json!(st.seq);
                line["hash"] = json!(format!("{:#x}", st.hash));
                println!("{line}");
                shown += 1;
                if shown >= limit {
                    break;
                }
            }
        }
        _ => usage(),
    }
}
