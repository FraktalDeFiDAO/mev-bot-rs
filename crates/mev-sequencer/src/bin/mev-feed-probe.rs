//! Measure the sequencer feed: tx throughput and how much earlier txs appear here than on RPC.
//!   cargo run --release -p mev-sequencer --bin mev-feed-probe -- [seconds=30] [rpc_url]

use std::time::{Duration, Instant};

use alloy::providers::{Provider, ProviderBuilder};
use mev_sequencer::SequencedTx;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let rpc_url = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "https://arb1.arbitrum.io/rpc".into());
    let rpc = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let (tx, mut rx) = mpsc::unbounded_channel::<SequencedTx>();
    tokio::spawn(mev_sequencer::run("wss://arb1.arbitrum.io/feed".into(), tx));

    let start = Instant::now();
    let (mut n, mut first_seq, mut last_seq) = (0u64, None, 0u64);
    let mut leads = Vec::new();
    while start.elapsed() < Duration::from_secs(secs) {
        let Ok(Some(t)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await else {
            continue;
        };
        n += 1;
        first_seq.get_or_insert(t.seq);
        last_seq = t.seq;
        // Every 50th tx: how long until RPC serves its receipt?
        if n % 50 == 1 && leads.len() < 8 {
            let seen = t.received;
            let mut tries = 0;
            loop {
                if rpc.get_transaction_receipt(t.hash).await?.is_some() {
                    leads.push(seen.elapsed().as_millis());
                    break;
                }
                tries += 1;
                if tries > 100 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
    let el = start.elapsed().as_secs_f64();
    println!(
        "txs: {n} in {el:.0}s = {:.1} tx/s, sequencer msgs {}..{}",
        n as f64 / el,
        first_seq.unwrap_or(0),
        last_seq
    );
    leads.sort();
    println!(
        "feed lead over RPC receipt (ms), {} samples: {:?}",
        leads.len(),
        leads
    );
    if !leads.is_empty() {
        println!("median lead: {} ms", leads[leads.len() / 2]);
    }
    Ok(())
}
