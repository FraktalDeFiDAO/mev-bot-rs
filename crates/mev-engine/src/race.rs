//! Race telemetry: for each arbitrage attempt, a timeline (trigger seen in the feed → decided →
//! signed → first endpoint accepted → our tx back in the feed) and, once it lands, who was ahead
//! of us in the block and by how much. All times are relative to the trigger's feed arrival, the
//! one clock every competitor shares.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, TxHash},
    providers::{DynProvider, Provider},
};
use mev_exec::{Landed, Sent};
use mev_sequencer::FeedLog;
use serde_json::{Value, json};

/// What set off an attempt.
#[derive(Debug, Clone, Copy)]
pub struct Trigger {
    /// The transaction whose effect created the opportunity (feed tx or block log's tx).
    pub tx: Option<TxHash>,
    /// "presim" (simulated feed tx) or "block" (block log).
    pub source: &'static str,
    /// When the engine learned of it.
    pub seen: Instant,
}

/// At most this many receipts are fetched per attempt to find the winner.
const MAX_RECEIPTS: usize = 10;

/// Where to look for the winner in a block: indices after the trigger (or, if the trigger is not
/// in this block, the last `MAX_RECEIPTS` before us) and strictly before our transaction.
pub fn candidates(
    block_txs: &[TxHash],
    ours: TxHash,
    trigger: Option<TxHash>,
) -> (Option<usize>, Option<usize>, Vec<usize>) {
    let our_idx = block_txs.iter().position(|h| *h == ours);
    let trig_idx = trigger.and_then(|t| block_txs.iter().position(|h| *h == t));
    let Some(end) = our_idx else {
        return (None, trig_idx, vec![]);
    };
    let start = match trig_idx {
        Some(t) if t < end => t + 1,
        _ => end.saturating_sub(MAX_RECEIPTS),
    };
    let between = (start..end).take(MAX_RECEIPTS).collect();
    (our_idx, trig_idx, between)
}

/// Signed milliseconds from `t0` to `t` (negative if `t` came first).
pub fn ms(t0: Instant, t: Instant) -> i64 {
    if t >= t0 {
        t.duration_since(t0).as_millis() as i64
    } else {
        -(t0.duration_since(t).as_millis() as i64)
    }
}

/// Build the race report for a landed attempt. Network errors leave fields null.
#[allow(clippy::too_many_arguments)]
pub async fn report(
    http: &DynProvider,
    feed: &Arc<Mutex<FeedLog>>,
    trigger: Trigger,
    decided: Instant,
    sent: Sent,
    landed: &Landed,
    pools: &[Address],
    est_profit_usd: f64,
) -> Value {
    let arrival = |h: &TxHash| feed.lock().unwrap().get(h).map(|(t, _)| t);
    let t0 = trigger.tx.and_then(|h| arrival(&h)).unwrap_or(trigger.seen);
    let our_feed = arrival(&sent.hash);

    let mut winner: Option<(TxHash, usize)> = None;
    let (mut our_idx, mut trig_idx) = (landed.tx_index.map(|i| i as usize), None);
    if let Some(n) = landed.block
        && let Ok(Some(block)) = http.get_block_by_number(BlockNumberOrTag::Number(n)).await
    {
        let hashes: Vec<TxHash> = block.transactions.hashes().collect();
        let (o, t, between) = candidates(&hashes, sent.hash, trigger.tx);
        (our_idx, trig_idx) = (o.or(our_idx), t);
        for i in between {
            let Ok(Some(r)) = http.get_transaction_receipt(hashes[i]).await else {
                continue;
            };
            if r.inner.logs().iter().any(|l| pools.contains(&l.address())) {
                winner = Some((hashes[i], i));
                break;
            }
        }
    }
    let winner_feed = winner.and_then(|(h, _)| arrival(&h));
    let rel = |t: Option<Instant>| t.map(|t| ms(t0, t));
    json!({
        "hash": sent.hash,
        "success": landed.success,
        "est_profit_usd": est_profit_usd,
        "source": trigger.source,
        "trigger": trigger.tx,
        "trigger_in_feed": trigger.tx.and_then(|h| arrival(&h)).is_some(),
        "block": landed.block,
        "our_index": our_idx,
        "trigger_index": trig_idx,
        "winner": winner.map(|(h, _)| h),
        "winner_index": winner.map(|(_, i)| i),
        "ms": {
            "seen": ms(t0, trigger.seen),
            "decided": ms(t0, decided),
            "signed": ms(t0, sent.signed),
            "first_accept": rel(sent.first_accept),
            "our_feed": rel(our_feed),
            "winner_feed": rel(winner_feed),
        },
        // How much later than the winner the sequencer ordered us (feed arrival gap).
        "behind_winner_ms": match (our_feed, winner_feed) {
            (Some(o), Some(w)) => Some(ms(w, o)),
            _ => None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn h(n: u8) -> TxHash {
        TxHash::with_last_byte(n)
    }

    #[test]
    fn winner_candidates_sit_between_trigger_and_us() {
        let block = [h(1), h(2), h(3), h(4), h(5)];
        // trigger at 1, us at 4 -> look at 2, 3
        assert_eq!(
            candidates(&block, h(5), Some(h(2))),
            (Some(4), Some(1), vec![2, 3])
        );
        // trigger in an earlier block -> the txs before us (capped)
        assert_eq!(
            candidates(&block, h(3), Some(h(9))),
            (Some(2), None, vec![0, 1])
        );
        // we're right after the trigger -> nobody in between
        assert_eq!(
            candidates(&block, h(2), Some(h(1))),
            (Some(1), Some(0), vec![])
        );
        // we're not in this block
        assert_eq!(
            candidates(&block, h(9), Some(h(1))),
            (None, Some(0), vec![])
        );
    }

    #[test]
    fn candidate_scan_is_capped() {
        let block: Vec<TxHash> = (0..40).map(h).collect();
        let (_, _, between) = candidates(&block, h(39), None);
        assert_eq!(between.len(), MAX_RECEIPTS);
        assert_eq!(between.last(), Some(&38));
    }

    #[test]
    fn signed_millis() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_millis(25);
        assert_eq!(ms(t0, later), 25);
        assert_eq!(ms(later, t0), -25);
    }
}
