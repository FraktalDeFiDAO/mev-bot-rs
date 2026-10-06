//! Missed-liquidation audit: for each liquidation that happened on-chain, was the bot running,
//! did it know the borrower, did it act, and how long was the position open before the winner
//! took it. Pure logic here; on-chain fetching lives in the `mev-liq-audit` binary.

use std::collections::{BTreeSet, HashMap, HashSet};

use alloy::primitives::{Address, address};

/// Atlas v1.6.4 on Arbitrum One (Chainlink SVR: oracle update + solver liquidation in one tx).
pub const ATLAS: Address = address!("8ad1ae9d97c79aa68a0a151e83ff3942f68f86c1");

/// One on-chain liquidation (a competitor's or ours).
#[derive(Debug, Clone)]
pub struct Liquidation {
    pub protocol: &'static str,
    pub block: u64,
    /// Unix seconds.
    pub ts: i64,
    pub tx: alloy::primitives::TxHash,
    pub tx_index: u64,
    pub borrower: Address,
    /// Address that sent the transaction, and the contract it called.
    pub sender: Address,
    pub to: Option<Address>,
    pub debt_usd: Option<f64>,
    /// Seconds the position was liquidatable before this tx (None = unknown / not measured).
    pub open_secs: Option<i64>,
}

/// What the bot's event log says about its own activity.
#[derive(Debug, Default)]
pub struct BotLog {
    /// Minutes (unix secs / 60) with any event; `rpc_usage` is written every minute while up.
    up_minutes: BTreeSet<i64>,
    /// Lowercased addresses mentioned in liquidation events -> (unix secs, event kind).
    touches: HashMap<Address, Vec<(i64, String)>>,
}

impl BotLog {
    /// Parse `events-*.jsonl` lines (`{"ts": rfc3339, "kind": ..., "data": ...}`); bad lines skipped.
    pub fn parse<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut log = BotLog::default();
        for line in lines {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(ts) = v["ts"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp())
            else {
                continue;
            };
            log.up_minutes.insert(ts / 60);
            let kind = v["kind"].as_str().unwrap_or_default();
            if kind.starts_with("liq") || kind == "sent" || kind == "landed" {
                for a in addresses_in(&v["data"].to_string()) {
                    log.touches
                        .entry(a)
                        .or_default()
                        .push((ts, kind.to_string()));
                }
            }
        }
        log
    }

    /// Running at `ts`: an event within the same or neighbouring minute.
    pub fn up_at(&self, ts: i64) -> bool {
        let m = ts / 60;
        (m - 1..=m + 1).any(|x| self.up_minutes.contains(&x))
    }

    /// Bot events about `who` in `[ts - window, ts + window]`.
    pub fn touches_near(&self, who: Address, ts: i64, window: i64) -> Vec<&str> {
        self.touches
            .get(&who)
            .map(|v| {
                v.iter()
                    .filter(|(t, _)| (t - ts).abs() <= window)
                    .map(|(_, k)| k.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// First and last logged second, if any.
    pub fn span(&self) -> Option<(i64, i64)> {
        Some((
            self.up_minutes.first()? * 60,
            self.up_minutes.last()? * 60 + 59,
        ))
    }

    /// Minutes with the bot up in `[from, to]`.
    pub fn up_minutes_between(&self, from: i64, to: i64) -> usize {
        self.up_minutes.range(from / 60..=to / 60).count()
    }
}

/// Every 20-byte hex address in `s` (also inside `account@market` strings), deduplicated.
fn addresses_in(s: &str) -> HashSet<Address> {
    let b = s.as_bytes();
    let mut out = HashSet::new();
    let mut i = 0;
    while i + 42 <= b.len() {
        if b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') {
            let hex = &s[i + 2..i + 42];
            let next_is_hex = b.get(i + 42).is_some_and(u8::is_ascii_hexdigit);
            if hex.bytes().all(|c| c.is_ascii_hexdigit()) && !next_is_hex {
                if let Ok(a) = hex.parse::<Address>() {
                    out.insert(a);
                }
                i += 42;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verdict {
    /// Our executor or wallet did it.
    Won,
    /// Sent to the Atlas contract: the liquidation was won in the off-chain SVR auction and
    /// bundled with the oracle update, so there was no speed race a send of ours could win.
    AtlasAuction,
    /// The bot wasn't running.
    BotDown,
    /// Borrower not in our account index (discovery gap).
    NotIndexed,
    /// The bot saw it and logged a skip/failure (`liq_no_route`, `liq_sim_revert`, ...).
    Skipped(String),
    /// Running and indexed, but no logged action. With `open_secs`: liquidated in the same
    /// second it became liquidatable (a latency race) vs. open longer (we should have acted).
    Silent,
}

/// Seconds a position must stay open before a silent miss counts as ours to lose rather than a
/// same-block race.
pub const RACE_SECS: i64 = 2;

pub fn classify(l: &Liquidation, log: &BotLog, indexed: bool, ours: &HashSet<Address>) -> Verdict {
    if ours.contains(&l.sender) || l.to.is_some_and(|t| ours.contains(&t)) {
        return Verdict::Won;
    }
    if l.to == Some(ATLAS) {
        return Verdict::AtlasAuction;
    }
    if !log.up_at(l.ts) {
        return Verdict::BotDown;
    }
    if !indexed {
        return Verdict::NotIndexed;
    }
    let mut kinds = log.touches_near(l.borrower, l.ts, 15 * 60);
    kinds.sort();
    kinds.dedup();
    match kinds.as_slice() {
        [] => Verdict::Silent,
        ks => Verdict::Skipped(ks.join("+")),
    }
}

/// Earliest block in `[lo, hi]` where `liquidatable(block)` holds, assuming it stays true until
/// `hi` (true at `hi`). Binary search; `None` if it is already true at `lo` (open longer than the
/// search window).
pub async fn first_liquidatable<F, Fut>(lo: u64, hi: u64, mut liquidatable: F) -> Option<u64>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if liquidatable(lo).await {
        return None;
    }
    let (mut ok_false, mut ok_true) = (lo, hi);
    while ok_true - ok_false > 1 {
        let mid = ok_false + (ok_true - ok_false) / 2;
        if liquidatable(mid).await {
            ok_true = mid;
        } else {
            ok_false = mid;
        }
    }
    Some(ok_true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"{"ts":"2026-10-04T10:00:05+00:00","kind":"rpc_usage","data":{}}
{"ts":"2026-10-04T10:01:10+00:00","kind":"liq_no_route","data":{"account":"0x20eE3E1f2a2dc0750FD8A0d449C32E898Bb96692@0x6c247b1F6182318877311737BaC0844bAa518F5e"}}
{"ts":"2026-10-04T10:02:00+00:00","kind":"liq_sim_revert","data":{"liq":{"user":"0x04511e23685239c56ee59d155423d973abf8a2e1"}}}
not json
{"ts":"2026-10-04T12:00:00+00:00","kind":"rpc_usage","data":{}}"#;

    fn t(hms: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&format!("2026-10-04T{hms}+00:00"))
            .unwrap()
            .timestamp()
    }

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn liq(borrower: Address, sender: Address, ts: i64) -> Liquidation {
        Liquidation {
            protocol: "aave-v3",
            block: 1,
            ts,
            tx: Default::default(),
            tx_index: 0,
            borrower,
            sender,
            to: None,
            debt_usd: None,
            open_secs: None,
        }
    }

    #[test]
    fn parses_uptime_and_touched_accounts() {
        let log = BotLog::parse(LOG.lines());
        assert!(log.up_at(t("10:01:30")));
        assert!(!log.up_at(t("11:00:00")), "gap between 10:02 and 12:00");
        let a = addr("0x20eE3E1f2a2dc0750FD8A0d449C32E898Bb96692");
        assert_eq!(
            log.touches_near(a, t("10:05:00"), 600),
            vec!["liq_no_route"]
        );
        // Lowercase user field parses too.
        let u = addr("0x04511e23685239c56ee59d155423d973abf8a2e1");
        assert_eq!(
            log.touches_near(u, t("10:02:00"), 1),
            vec!["liq_sim_revert"]
        );
        assert_eq!(log.up_minutes_between(t("10:00:00"), t("10:59:59")), 3);
    }

    #[test]
    fn address_scan_ignores_longer_hex() {
        let long = format!("0x{}", "ab".repeat(32)); // bytes32, not an address
        assert!(addresses_in(&long).is_empty());
        assert_eq!(addresses_in(&format!("x 0x{} y", "11".repeat(20))).len(), 1);
    }

    #[test]
    fn atlas_bundles_are_auctions_not_races() {
        let log = BotLog::parse(LOG.lines());
        let other = addr("0x00000000000000000000000000000000000000bb");
        let me = addr("0x00000000000000000000000000000000000000aa");
        let mut l = liq(other, other, t("10:01:00"));
        l.to = Some(ATLAS);
        // Even with the bot down at that time, an Atlas bundle is an auction win, not a miss.
        assert_eq!(
            classify(&l, &log, true, &HashSet::new()),
            Verdict::AtlasAuction
        );
        // Ours still wins the classification first.
        assert_eq!(
            classify(&l, &log, true, &HashSet::from([other])),
            Verdict::Won
        );
        let _ = me;
    }

    #[test]
    fn classify_orders_reasons() {
        let log = BotLog::parse(LOG.lines());
        let me = addr("0x00000000000000000000000000000000000000aa");
        let other = addr("0x00000000000000000000000000000000000000bb");
        let touched = addr("0x20eE3E1f2a2dc0750FD8A0d449C32E898Bb96692");
        let ours = HashSet::from([me]);
        assert_eq!(
            classify(&liq(touched, me, t("11:00:00")), &log, true, &ours),
            Verdict::Won
        );
        assert_eq!(
            classify(&liq(touched, other, t("11:00:00")), &log, true, &ours),
            Verdict::BotDown
        );
        assert_eq!(
            classify(&liq(other, other, t("10:01:00")), &log, false, &ours),
            Verdict::NotIndexed
        );
        assert_eq!(
            classify(&liq(touched, other, t("10:01:20")), &log, true, &ours),
            Verdict::Skipped("liq_no_route".into())
        );
        assert_eq!(
            classify(&liq(other, other, t("10:01:00")), &log, true, &ours),
            Verdict::Silent
        );
    }

    #[tokio::test]
    async fn first_liquidatable_finds_the_crossing_block() {
        let crossing = 1_037;
        let f = |b: u64| async move { b >= crossing };
        assert_eq!(first_liquidatable(1_000, 1_100, f).await, Some(crossing));
        // Already liquidatable at the window start: open longer than we searched.
        assert_eq!(first_liquidatable(1_040, 1_100, f).await, None);
    }
}
