//! Record Base Flashblocks: one JSON line per transaction with its block, flashblock index and arrival time.
//!   cargo run --release -p mev-sequencer --bin mev-flashblocks-record -- [seconds=600] [url]
//!
//! Joined with block receipts this shows *where* in a block a competitor's backrun landed relative to its trigger:
//! a later flashblock means it reacted to what it saw (latency decides), the same flashblock means it was sent before
//! the trigger was public (prediction or spam; latency cannot help).

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mev_sequencer::{FeedStats, SequencedTx, flashblocks};
use serde_json::json;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600);
    let url = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "wss://mainnet.flashblocks.base.org/ws".into());
    let (tx, mut rx) = mpsc::unbounded_channel::<SequencedTx>();
    let stats = Arc::new(FeedStats::default());
    tokio::spawn(flashblocks::run_tracked(url, tx, stats.clone()));

    // Instant -> wall clock, anchored once: `received` is an Instant taken when the frame arrived.
    let (anchor_i, anchor_ms) = (Instant::now(), now_ms());
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < Duration::from_secs(secs) {
        let Ok(Some(t)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await else {
            continue;
        };
        let recv_ms = if t.received >= anchor_i {
            anchor_ms + t.received.duration_since(anchor_i).as_millis() as u64
        } else {
            anchor_ms - anchor_i.duration_since(t.received).as_millis() as u64
        };
        println!(
            "{}",
            json!({"block": t.seq >> 8, "index": t.seq & 0xff, "recv_ms": recv_ms, "hash": t.hash, "to": t.to})
        );
        n += 1;
    }
    eprintln!(
        "recorded {n} txs in {secs}s; gaps={} stale={} reconnects={}",
        stats.gaps.load(std::sync::atomic::Ordering::Relaxed),
        stats.stale.load(std::sync::atomic::Ordering::Relaxed),
        stats.reconnects.load(std::sync::atomic::Ordering::Relaxed)
    );
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
