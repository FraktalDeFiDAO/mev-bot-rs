//! Can a free endpoint keep a hot state cache exact? Follows the chain head and fetches every
//! block's prestate diff; reports latency, rate limits, changes per block and lag behind head.
//!   cargo run --release -p mev-localsim --bin mev-diff-probe -- [secs=600] [diff_rpc] [head_rpc]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use mev_localsim::diff::{changes, fetch_block_diff};
use tokio::sync::Semaphore;

#[derive(Default)]
struct Stats {
    blocks_seen: u64,
    ok: u64,
    rate_limited: u64,
    errors: u64,
    latency_ms: Vec<u64>,
    changes: u64,
    lag_blocks: Vec<u64>,
    last_error: String,
}

fn provider(url: &str) -> anyhow::Result<DynProvider> {
    Ok(ProviderBuilder::new().connect_http(url.parse()?).erased())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600);
    let diff_rpc = provider(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("https://arbitrum.drpc.org"),
    )?;
    let head_rpc = provider(
        args.get(3)
            .map(String::as_str)
            .unwrap_or("https://arb1.arbitrum.io/rpc"),
    )?;
    let stats = Arc::new(Mutex::new(Stats::default()));
    let head = Arc::new(std::sync::atomic::AtomicU64::new(
        head_rpc.get_block_number().await?,
    ));
    {
        let (head, p) = (head.clone(), head_rpc.clone());
        tokio::spawn(async move {
            loop {
                if let Ok(n) = p.get_block_number().await {
                    head.fetch_max(n, std::sync::atomic::Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }
    let slots = Arc::new(Semaphore::new(4));
    let mut next = head.load(std::sync::atomic::Ordering::Relaxed) + 1;
    let start = Instant::now();
    let mut last_report = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if last_report.elapsed() >= Duration::from_secs(60) {
            report(&stats, start.elapsed());
            last_report = Instant::now();
        }
        let h = head.load(std::sync::atomic::Ordering::Relaxed);
        if next > h {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        let n = next;
        next += 1;
        stats.lock().unwrap().blocks_seen += 1;
        let permit = slots.clone().acquire_owned().await?;
        let (p, stats, head) = (diff_rpc.clone(), stats.clone(), head.clone());
        tokio::spawn(async move {
            let t = Instant::now();
            let r = fetch_block_diff(&p, n).await;
            drop(permit);
            let mut s = stats.lock().unwrap();
            match r {
                Ok(block) => {
                    s.ok += 1;
                    s.latency_ms.push(t.elapsed().as_millis() as u64);
                    s.changes += changes(&block).len() as u64;
                    let h = head.load(std::sync::atomic::Ordering::Relaxed);
                    s.lag_blocks.push(h.saturating_sub(n));
                }
                Err(e) => {
                    let m = format!("{e:#}");
                    if m.contains("429") || m.to_lowercase().contains("rate") || m.contains("limit")
                    {
                        s.rate_limited += 1;
                    } else {
                        s.errors += 1;
                    }
                    s.last_error = m.chars().take(160).collect();
                }
            }
        });
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    report(&stats, start.elapsed());
    Ok(())
}

fn report(stats: &Arc<Mutex<Stats>>, t: Duration) {
    let s = stats.lock().unwrap();
    let pct = |v: &[u64], q: f64| {
        let mut v = v.to_vec();
        v.sort_unstable();
        v.get(((v.len() as f64 * q) as usize).min(v.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    println!(
        "[{:>4}s] blocks {} ok {} rate-limited {} errors {} | latency ms p50 {} p90 {} p99 {} | changes/block {} | lag blocks p50 {} p90 {} max {} | last error: {}",
        t.as_secs(),
        s.blocks_seen,
        s.ok,
        s.rate_limited,
        s.errors,
        pct(&s.latency_ms, 0.5),
        pct(&s.latency_ms, 0.9),
        pct(&s.latency_ms, 0.99),
        s.changes.checked_div(s.ok).unwrap_or(0),
        pct(&s.lag_blocks, 0.5),
        pct(&s.lag_blocks, 0.9),
        s.lag_blocks.iter().max().copied().unwrap_or(0),
        s.last_error,
    );
}
