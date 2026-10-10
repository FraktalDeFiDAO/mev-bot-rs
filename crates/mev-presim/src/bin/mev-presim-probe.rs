//! Measure feed-driven pre-simulation: feed tx rate, how many are candidates, how many move a
//! pool (and which), simulate latency, and error/rate-limit rate.
//!   cargo run --release -p mev-presim --bin mev-presim-probe -- [seconds=60] [rpc_url] [max_in_flight=8]

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use alloy::primitives::Address;
use mev_sequencer::SequencedTx;
use tokio::sync::{Mutex, Semaphore, mpsc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let url = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "https://arbitrum-one-rpc.publicnode.com".into());
    let max_in_flight: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let rpc = mev_rpc_provider(&url)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<SequencedTx>();
    tokio::spawn(mev_sequencer::run("wss://arb1.arbitrum.io/feed".into(), tx));

    let seen = Arc::new(AtomicU64::new(0));
    let candidates = Arc::new(AtomicU64::new(0));
    let skipped_busy = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let rate_limited = Arc::new(AtomicU64::new(0));
    let moved = Arc::new(AtomicU64::new(0));
    let lat = Arc::new(Mutex::new(Vec::<u128>::new()));
    let pools = Arc::new(Mutex::new(HashMap::<Address, u64>::new()));
    let sem = Arc::new(Semaphore::new(max_in_flight));

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        let Ok(Some(st)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await else {
            continue;
        };
        seen.fetch_add(1, Ordering::Relaxed);
        if !mev_presim::is_candidate(&st) {
            continue;
        }
        candidates.fetch_add(1, Ordering::Relaxed);
        let Ok(permit) = sem.clone().try_acquire_owned() else {
            skipped_busy.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let (rpc, errors, rate_limited, moved, lat, pools) = (
            rpc.clone(),
            errors.clone(),
            rate_limited.clone(),
            moved.clone(),
            lat.clone(),
            pools.clone(),
        );
        tokio::spawn(async move {
            let t = Instant::now();
            let res = mev_presim::simulate_logs(&rpc, &st.tx).await;
            drop(permit);
            match res {
                Ok(logs) => {
                    lat.lock().await.push(t.elapsed().as_millis());
                    let ev: Vec<Address> =
                        mev_presim::pool_events(&logs).map(|l| l.address).collect();
                    if !ev.is_empty() {
                        moved.fetch_add(1, Ordering::Relaxed);
                        let mut p = pools.lock().await;
                        for a in ev {
                            *p.entry(a).or_default() += 1;
                        }
                    }
                }
                Err(e) => {
                    let s = format!("{e:#}");
                    if s.contains("429")
                        || s.contains("-32005")
                        || s.to_lowercase().contains("rate")
                    {
                        rate_limited.fetch_add(1, Ordering::Relaxed);
                    } else {
                        errors.fetch_add(1, Ordering::Relaxed);
                        if errors.load(Ordering::Relaxed) <= 3 {
                            eprintln!("error: {}", s.chars().take(200).collect::<String>());
                        }
                    }
                }
            }
        });
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut l = lat.lock().await.clone();
    l.sort_unstable();
    let pct = |q: f64| {
        l.get(((l.len() as f64 * q) as usize).min(l.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    let s = secs as f64;
    println!("rpc: {}", mev_types::redact_secrets(&url));
    println!(
        "feed txs {:.1}/s | candidates {:.1}/s | moved a pool {:.1}/s | skipped (busy) {} | errors {} | rate-limited {}",
        seen.load(Ordering::Relaxed) as f64 / s,
        candidates.load(Ordering::Relaxed) as f64 / s,
        moved.load(Ordering::Relaxed) as f64 / s,
        skipped_busy.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        rate_limited.load(Ordering::Relaxed),
    );
    println!(
        "simulate latency ms: p50 {} p90 {} p99 {} (n={})",
        pct(0.5),
        pct(0.9),
        pct(0.99),
        l.len()
    );
    let p = pools.lock().await;
    let mut top: Vec<_> = p.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1));
    println!("distinct pools moved: {}; top:", p.len());
    for (a, n) in top.into_iter().take(10) {
        println!("  {a} {n}");
    }
    Ok(())
}

fn mev_rpc_provider(url: &str) -> anyhow::Result<alloy::providers::DynProvider> {
    use alloy::providers::{Provider, ProviderBuilder};
    Ok(ProviderBuilder::new().connect_http(url.parse()?).erased())
}
