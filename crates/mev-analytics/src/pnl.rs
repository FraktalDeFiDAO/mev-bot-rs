//! Net PnL per strategy after priority fees, from the bot's event log (`events-*.jsonl`).
//!
//! Sources per event kind:
//! - `bid`: a placed PGA bid; `tip_usd` is the committed priority-fee spend (charged if included).
//! - `landed`: a mined tx; `gas_usd` is gas × effective gas price, which already includes the
//!   priority fee actually paid; `success` and `est_profit_usd` give the (estimated) profit won.
//! - `bid_skip`: a send the policy or the tip caps declined.
//!
//! Strategy: `landed.kind` (`arb`, `liquidation`, ...); for `bid`/`bid_skip` the opportunity shape
//! (`loan` ⇒ arb, `protocol` ⇒ liquidation).

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct StrategyPnl {
    pub bids: u64,
    pub bids_skipped: u64,
    pub landed: u64,
    pub won: u64,
    pub lost: u64,
    /// Estimated profit of successful txs, USD.
    pub gross_usd: f64,
    /// Gas paid on-chain (includes priority fees actually paid), USD.
    pub gas_usd: f64,
    /// Priority fees committed at bid time, USD (for the bid/profit ratio).
    pub tip_committed_usd: f64,
}

impl StrategyPnl {
    pub fn net_usd(&self) -> f64 {
        self.gross_usd - self.gas_usd
    }
}

fn strategy_of(opp: &Value) -> &'static str {
    let o = opp.get("opp").unwrap_or(opp);
    let o = o.get("liq").unwrap_or(o);
    if o.get("loan").is_some() {
        "arb"
    } else if o.get("protocol").is_some() {
        "liquidation"
    } else {
        "other"
    }
}

/// Aggregate events (one JSON object per line: `{ "kind", "data", "ts" }`).
pub fn aggregate<'a>(events: impl IntoIterator<Item = &'a Value>) -> BTreeMap<String, StrategyPnl> {
    let mut out: BTreeMap<String, StrategyPnl> = BTreeMap::new();
    for e in events {
        let (Some(kind), Some(d)) = (e.get("kind").and_then(Value::as_str), e.get("data")) else {
            continue;
        };
        match kind {
            "bid" => {
                let s = out.entry(strategy_of(d).into()).or_default();
                s.bids += 1;
                s.tip_committed_usd += d.get("tip_usd").and_then(Value::as_f64).unwrap_or(0.0);
            }
            "bid_skip" => out.entry(strategy_of(d).into()).or_default().bids_skipped += 1,
            "landed" => {
                let k = d.get("kind").and_then(Value::as_str).unwrap_or("other");
                let s = out.entry(k.into()).or_default();
                s.landed += 1;
                s.gas_usd += d.get("gas_usd").and_then(Value::as_f64).unwrap_or(0.0);
                if d.get("success").and_then(Value::as_bool).unwrap_or(false) {
                    s.won += 1;
                    s.gross_usd += d
                        .get("est_profit_usd")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                } else {
                    s.lost += 1;
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nets_profit_after_gas_including_priority_fees_per_strategy() {
        let ev = [
            json!({"kind":"bid","data":{"opp":{"loan":"WETH"},"tip_usd":0.01}}),
            json!({"kind":"bid","data":{"opp":{"liq":{"protocol":"aave-v3"}},"tip_usd":0.05}}),
            json!({"kind":"bid_skip","data":{"opp":{"loan":"USDC"},"reason":"OutbidTooExpensive"}}),
            json!({"kind":"landed","data":{"kind":"arb","success":true,"est_profit_usd":0.5,"gas_usd":0.03}}),
            json!({"kind":"landed","data":{"kind":"arb","success":false,"est_profit_usd":0.4,"gas_usd":0.02}}),
            json!({"kind":"landed","data":{"kind":"liquidation","success":true,"est_profit_usd":3.0,"gas_usd":0.2}}),
            json!({"kind":"rpc_usage","data":{}}),
        ];
        let p = aggregate(ev.iter());
        let arb = &p["arb"];
        assert_eq!(
            (arb.bids, arb.bids_skipped, arb.won, arb.lost),
            (1, 1, 1, 1)
        );
        assert!(
            (arb.net_usd() - (0.5 - 0.05)).abs() < 1e-12,
            "lost tx's gas counts"
        );
        let liq = &p["liquidation"];
        assert!((liq.net_usd() - 2.8).abs() < 1e-12);
        assert!((liq.tip_committed_usd - 0.05).abs() < 1e-12);
    }
}
