//! Measures the production local-simulation worker against the chain. Runs `worker::run` on the
//! live sequencer feed exactly as the engine does and compares every pool-event prediction it
//! emits with the transaction's **mined receipt** (the real ground truth). Never sends anything.
//!   cargo run --release -p mev-localsim --bin mev-localsim-probe -- [secs=900] [diff_rpc] [fill_rpc]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use alloy::{
    primitives::{Address, B256, Bytes},
    providers::{DynProvider, Provider, ProviderBuilder},
};
use mev_localsim::{Logs, worker};
use mev_sequencer::SequencedTx;
use tokio::sync::{Semaphore, mpsc};

#[derive(Default)]
struct Stats {
    predictions: u64,
    compared: u64,
    exact: u64,
    pools_match: u64,
    reverted_on_chain: u64,
    no_receipt: u64,
    latency_ms: Vec<u64>,
    fallback_seen: u64,
    fallback_missed_movers: u64,
}

fn provider(url: &str) -> anyhow::Result<DynProvider> {
    Ok(ProviderBuilder::new().connect_http(url.parse()?).erased())
}

fn pool_events(logs: impl IntoIterator<Item = (Address, Vec<B256>, Bytes)>) -> Logs {
    logs.into_iter()
        .filter(|(_, t, _)| t.first().is_some_and(mev_presim::is_pool_event))
        .collect()
}

/// (succeeded, pool events) from the mined receipt, if it shows up within ~7 s.
async fn mined_pool_events(p: &DynProvider, hash: B256) -> Option<(bool, Logs)> {
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(600)).await;
        if let Ok(Some(r)) = p.get_transaction_receipt(hash).await {
            let logs = pool_events(
                r.inner
                    .logs()
                    .iter()
                    .map(|l| (l.address(), l.topics().to_vec(), l.data().data.clone())),
            );
            return Some((r.status(), logs));
        }
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(900);
    let url = |i: usize, d: &'static str| args.get(i).map(String::as_str).unwrap_or(d).to_string();
    let endpoints = worker::Endpoints {
        diff: provider(&url(2, "https://arbitrum.drpc.org"))?,
        // dRPC is reserved for block diffs (the only free endpoint serving them).
        fill: provider(&url(3, "https://arbitrum-one-rpc.publicnode.com"))?,
        head: provider("https://arb1.arbitrum.io/rpc")?,
    };
    // Receipts from arb1: complete, and not the rate-limited simulate endpoint.
    let receipts = provider("https://arb1.arbitrum.io/rpc")?;

    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<SequencedTx>();
    tokio::spawn(mev_sequencer::run(
        "wss://arb1.arbitrum.io/feed".into(),
        feed_tx,
    ));
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<mev_presim::Presimulated>();
    let (fb_tx, mut fb_rx) = mpsc::unbounded_channel::<SequencedTx>();
    let wstats = Arc::new(worker::LocalStats::default());
    // Empty watchlist: every contract call counts as a candidate (the engine narrows this).
    tokio::spawn(worker::run(
        feed_rx,
        endpoints,
        Default::default(),
        out_tx,
        fb_tx,
        wstats.clone(),
    ));

    let stats = Arc::new(Mutex::new(Stats::default()));
    let slots = Arc::new(Semaphore::new(16));
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if last.elapsed() >= Duration::from_secs(60) {
            report(&stats, &wstats, start.elapsed());
            last = Instant::now();
        }
        tokio::select! {
            Some(pre) = out_rx.recv() => {
                let latency = pre.received.elapsed().as_millis() as u64;
                let local = pool_events(
                    pre.logs.iter().map(|l| (l.address(), l.topics().to_vec(), l.data().data.clone())),
                );
                {
                    let mut s = stats.lock().unwrap();
                    s.predictions += 1;
                    s.latency_ms.push(latency);
                }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                let (p, stats) = (receipts.clone(), stats.clone());
                tokio::spawn(async move {
                    let mined = mined_pool_events(&p, pre.tx).await;
                    drop(permit);
                    let mut s = stats.lock().unwrap();
                    let Some((ok, mined)) = mined else {
                        s.no_receipt += 1;
                        return;
                    };
                    s.compared += 1;
                    if !ok {
                        s.reverted_on_chain += 1;
                    }
                    if mined == local {
                        s.exact += 1;
                    }
                    let addrs = |l: &Logs| l.iter().map(|(a, _, _)| *a).collect::<Vec<_>>();
                    if addrs(&mined) == addrs(&local) {
                        s.pools_match += 1;
                    }
                });
            }
            Some(st) = fb_rx.recv() => {
                // Candidates the worker couldn't run locally: how many actually moved a pool?
                stats.lock().unwrap().fallback_seen += 1;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                let (p, stats) = (receipts.clone(), stats.clone());
                tokio::spawn(async move {
                    let mined = mined_pool_events(&p, st.hash).await;
                    drop(permit);
                    if let Some((true, m)) = mined
                        && !m.is_empty()
                    {
                        stats.lock().unwrap().fallback_missed_movers += 1;
                    }
                });
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
    report(&stats, &wstats, start.elapsed());
    Ok(())
}

fn report(stats: &Arc<Mutex<Stats>>, w: &Arc<worker::LocalStats>, t: Duration) {
    let s = stats.lock().unwrap();
    let mut l = s.latency_ms.clone();
    l.sort_unstable();
    let pct = |q: f64| {
        l.get(((l.len() as f64 * q) as usize).min(l.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    let ratio = |a: u64, b: u64| {
        if b == 0 {
            0.0
        } else {
            100.0 * a as f64 / b as f64
        }
    };
    println!(
        "[{:>4}s] local pool-move predictions {} | vs MINED receipt: compared {} exact {:.1}% same-pools {:.1}% (reverted on chain {}, no receipt {}) | feed->prediction ms p50 {} p90 {} p99 {} | fallback candidates {} (sampled; moved pools {})",
        t.as_secs(),
        s.predictions,
        s.compared,
        ratio(s.exact, s.compared),
        ratio(s.pools_match, s.compared),
        s.reverted_on_chain,
        s.no_receipt,
        pct(0.5),
        pct(0.9),
        pct(0.99),
        s.fallback_seen,
        s.fallback_missed_movers,
    );
    println!("[{:>4}s] worker: {}", t.as_secs(), w.take_summary());
}
