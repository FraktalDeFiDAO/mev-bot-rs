//! Chain-side ground truth for what the sequencer feed showed us.
//!
//! The feed carries signed transaction bodies only: no success or revert, no gas used, no events. This crate
//! subscribes to new blocks, and for every `every_n`-th block fetches all receipts in one `eth_getBlockReceipts`
//! call. It writes `chaintx-DATE.jsonl` lines for
//!  - every transaction the feed recorder saw (so revert rates, gas and block position can be joined on the hash), and
//!  - every transaction that emitted a swap, liquidity or liquidation event, even if the feed recorder did not see it
//!    as a candidate (these are the decoder's coverage gaps: group them by `to`).
//!
//! Sampling keeps the RPC cost at `4 / every_n` calls a second (Arbitrum makes about 4 blocks a second).

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use alloy::{
    primitives::B256,
    providers::{DynProvider, Provider},
};
use futures::StreamExt;
use mev_presim::record::TxRecorder;
use tracing::{info, warn};

/// `(kind, class)` of an event with this `topic0`, if it is one we keep (the table lives in `mev-decode`).
pub fn classify(topic0: &B256) -> Option<(&'static str, &'static str)> {
    mev_decode::events::classify(topic0).map(|k| (k.name, k.class.as_str()))
}

fn hex_u64(v: &serde_json::Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.trim_start_matches("0x"), 16).ok()
}

/// Logs worth keeping from a receipt's `logs` array, as JSON, with the classes present.
///
/// Receipts are read as raw JSON, not as the standard Ethereum receipt type: Arbitrum receipts include chain-specific
/// transaction types (for example the internal system transaction, type `0x6a`) that the standard parser rejects.
fn interesting(logs: &[serde_json::Value]) -> (Vec<serde_json::Value>, Vec<&'static str>) {
    let mut out = Vec::new();
    let mut classes: Vec<&'static str> = Vec::new();
    for l in logs {
        let topics: Vec<&str> = l["topics"]
            .as_array()
            .map(|t| t.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        let Some(t0) = topics.first().and_then(|t| t.parse::<B256>().ok()) else {
            continue;
        };
        let Some((kind, class)) = classify(&t0) else {
            continue;
        };
        if !classes.contains(&class) {
            classes.push(class);
        }
        let data = l["data"].as_str().unwrap_or("0x").trim_start_matches("0x");
        out.push(serde_json::json!({
            "addr": l["address"],
            "kind": kind,
            "topics": topics,
            "data": &data[..data.len().min(512)],
        }));
    }
    (out, classes)
}

/// The line for a raw receipt, or `None` if it is neither a recorded feed transaction nor an event we keep.
pub fn outcome_line(r: &serde_json::Value, feed: &TxRecorder) -> Option<serde_json::Value> {
    let hash = r["transactionHash"].as_str()?.to_lowercase();
    let was_candidate = feed.was_recorded(&hash);
    let empty = Vec::new();
    let all_logs = r["logs"].as_array().unwrap_or(&empty);
    let (logs, classes) = interesting(all_logs);
    if !was_candidate && logs.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "hash": hash,
        "block": hex_u64(&r["blockNumber"]),
        "index": hex_u64(&r["transactionIndex"]),
        "from": r["from"],
        "to": r["to"],
        "type": r["type"],
        "status": hex_u64(&r["status"]).map(|s| s == 1),
        "gas_used": hex_u64(&r["gasUsed"]),
        "gas_price": hex_u64(&r["effectiveGasPrice"]),
        "n_logs": all_logs.len(),
        "candidate": was_candidate,
        "classes": classes,
        "logs": logs,
    }))
}

#[derive(Default)]
struct Counters {
    blocks: AtomicU64,
    txs: AtomicU64,
    recorded: AtomicU64,
    reverted: AtomicU64,
    empty: AtomicU64,
}

async fn fetch(ws: &DynProvider, n: u64) -> Option<Vec<serde_json::Value>> {
    for attempt in 0..4 {
        match ws
            .raw_request::<_, Option<Vec<serde_json::Value>>>(
                "eth_getBlockReceipts".into(),
                (format!("{n:#x}"),),
            )
            .await
        {
            Ok(Some(r)) => return Some(r),
            Ok(None) => {} // not served yet
            Err(e) => {
                if attempt == 3 {
                    warn!(error = %e, block = n, "block receipts failed");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

/// Follow new blocks forever (reconnecting), sampling every `every_n`-th block into `out`.
pub async fn run(ws: DynProvider, feed: Arc<TxRecorder>, out: Arc<TxRecorder>, every_n: u64) {
    let every_n = every_n.max(1);
    let c = Arc::new(Counters::default());
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        let sub = match ws.subscribe_blocks().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "block subscription failed; retrying");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut stream = sub.into_stream();
        info!(every_n, "chain outcome sampling on");
        loop {
            tokio::select! {
                h = stream.next() => {
                    let Some(h) = h else { break };
                    if h.number % every_n != 0 {
                        continue;
                    }
                    let (ws, feed, out, c) = (ws.clone(), feed.clone(), out.clone(), c.clone());
                    tokio::spawn(async move {
                        let Some(receipts) = fetch(&ws, h.number).await else {
                            c.empty.fetch_add(1, Relaxed);
                            return;
                        };
                        c.blocks.fetch_add(1, Relaxed);
                        c.txs.fetch_add(receipts.len() as u64, Relaxed);
                        for r in &receipts {
                            if r["status"].as_str() != Some("0x1") {
                                c.reverted.fetch_add(1, Relaxed);
                            }
                            if let Some(line) = outcome_line(r, &feed) {
                                c.recorded.fetch_add(1, Relaxed);
                                out.record(&line);
                            }
                        }
                    });
                }
                _ = tick.tick() => {
                    info!(
                        blocks = c.blocks.swap(0, Relaxed),
                        txs = c.txs.swap(0, Relaxed),
                        recorded = c.recorded.swap(0, Relaxed),
                        reverted = c.reverted.swap(0, Relaxed),
                        fetch_failed = c.empty.swap(0, Relaxed),
                        dropped = out.dropped(),
                        "chain outcomes (last minute)"
                    );
                }
            }
        }
        warn!("block subscription ended; resubscribing");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap_topic() -> String {
        format!(
            "{:#x}",
            mev_decode::events::kinds()
                .iter()
                .find(|k| k.name == "swap_v3")
                .unwrap()
                .topic0
        )
    }

    fn receipt(hash: &str, status: &str, ty: &str, logs: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "transactionHash": hash, "blockNumber": "0x1f4", "transactionIndex": "0x3",
            "from": "0x0000000000000000000000000000000000000001", "to": "0x0000000000000000000000000000000000000002",
            "type": ty, "status": status, "gasUsed": "0x5208", "effectiveGasPrice": "0x989680", "logs": logs,
        })
    }

    fn recorder(tag: &str) -> Arc<TxRecorder> {
        let d = std::env::temp_dir().join(format!("outcomes-test-{tag}-{}", std::process::id()));
        TxRecorder::open(d, 7).unwrap()
    }

    #[test]
    fn swap_logs_are_kept_and_unknown_ones_ignored() {
        let logs = serde_json::json!([
            { "address": "0xpool", "topics": [swap_topic()], "data": "0x01" },
            { "address": "0xother", "topics": [format!("{:#x}", B256::repeat_byte(0x11))], "data": "0x" },
        ]);
        let (kept, classes) = interesting(logs.as_array().unwrap());
        assert_eq!(kept.len(), 1);
        assert_eq!(classes, vec!["swap"]);
        assert_eq!(kept[0]["kind"], "swap_v3");
    }

    #[test]
    fn an_arbitrum_internal_transaction_type_does_not_break_reading() {
        // Type 0x6a (106) is what the standard Ethereum receipt parser rejects ("Unexpected type flag. Got 106").
        let rec = recorder("internal");
        let r = receipt("0xaa", "0x1", "0x6a", serde_json::json!([]));
        assert!(
            outcome_line(&r, &rec).is_none(),
            "neither a candidate nor an event we keep"
        );
        rec.record(&serde_json::json!({ "hash": "0xaa" }));
        let line = outcome_line(&r, &rec).expect("a recorded candidate is kept");
        assert_eq!(line["type"], "0x6a");
        assert_eq!(line["status"], true);
        assert_eq!(line["gas_used"], 21000);
        assert_eq!(line["block"], 500);
        assert_eq!(line["index"], 3);
        assert_eq!(line["candidate"], true);
    }

    #[test]
    fn reverted_candidates_and_uncovered_event_emitters_are_both_recorded() {
        let rec = recorder("both");
        rec.record(&serde_json::json!({ "hash": "0xbb" }));
        let reverted = receipt("0xBB", "0x0", "0x2", serde_json::json!([])); // upper-case hash still matches
        let line = outcome_line(&reverted, &rec).unwrap();
        assert_eq!(line["status"], false);
        assert_eq!(line["candidate"], true);
        let event_only = receipt(
            "0xcc",
            "0x1",
            "0x2",
            serde_json::json!([{ "address": "0xpool", "topics": [swap_topic()], "data": "0x" }]),
        );
        let line = outcome_line(&event_only, &rec).unwrap();
        assert_eq!(line["candidate"], false);
        assert_eq!(line["classes"], serde_json::json!(["swap"]));
    }
}
