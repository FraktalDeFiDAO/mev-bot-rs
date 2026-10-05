//! Is in-process simulation fast and accurate enough without our own node? Follows the sequencer
//! feed, simulates candidate txs locally (filling the cache from RPC on misses), and compares the
//! pool events of complete local runs with `eth_simulateV1` (ground truth). Never sends anything.
//!   cargo run --release -p mev-localsim --bin mev-localsim-probe -- [secs=600] [ttl_blocks=20] [fill_rpc] [truth_rpc]

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use mev_localsim::{Key, Logs, StateCache, fill, simulate};
use mev_sequencer::SequencedTx;
use tokio::sync::{Semaphore, mpsc};

#[derive(Default)]
struct Stats {
    candidates: u64,
    complete: u64,
    with_stale: u64,
    local_us: Vec<u64>,
    compared: u64,
    exact: u64,
    truth_movers: u64,
    movers_exact: u64,
    false_movers: u64,
    truth_errors: u64,
}

fn provider(url: &str) -> anyhow::Result<DynProvider> {
    Ok(ProviderBuilder::new().connect_http(url.parse()?).erased())
}

fn pool_events(logs: &Logs) -> Logs {
    logs.iter()
        .filter(|(_, t, _)| t.first().is_some_and(mev_presim::is_pool_event))
        .cloned()
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600);
    let ttl: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let fill_rpc = provider(
        args.get(3)
            .map(String::as_str)
            .unwrap_or("https://arbitrum-one-rpc.publicnode.com"),
    )?;
    let truth_rpc = provider(
        args.get(4)
            .map(String::as_str)
            .unwrap_or("https://arb1.arbitrum.io/rpc"),
    )?;

    let cache = Arc::new(Mutex::new(StateCache::new(ttl)));
    let stats = Arc::new(Mutex::new(Stats::default()));
    let inflight: Arc<Mutex<HashSet<Key>>> = Default::default();
    let fill_slots = Arc::new(Semaphore::new(8));
    let truth_slots = Arc::new(Semaphore::new(4));

    // Head tracking.
    {
        let (cache, p) = (cache.clone(), fill_rpc.clone());
        tokio::spawn(async move {
            loop {
                if let Ok(n) = p.get_block_number().await {
                    cache.lock().unwrap().head = n;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<SequencedTx>();
    tokio::spawn(mev_sequencer::run("wss://arb1.arbitrum.io/feed".into(), tx));

    let start = Instant::now();
    let mut last_report = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if last_report.elapsed() >= Duration::from_secs(60) {
            report(&stats, &cache, start.elapsed());
            last_report = Instant::now();
        }
        let Ok(Some(st)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await else {
            continue;
        };
        if !mev_presim::is_candidate(&st) {
            continue;
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let (out, head) = {
            let c = cache.lock().unwrap();
            (simulate(&c, &st.tx, now), c.head)
        };
        let Ok(out) = out else { continue };
        {
            let mut s = stats.lock().unwrap();
            s.candidates += 1;
            if out.complete {
                s.complete += 1;
                s.local_us.push(out.elapsed.as_micros() as u64);
                if !out.stale.is_empty() {
                    s.with_stale += 1;
                }
            }
        }
        // Warm the cache: fetch missing keys (and refresh stale ones) in the background.
        let mut want: Vec<Key> = out
            .missing
            .iter()
            .chain(out.stale.iter())
            .copied()
            .collect();
        {
            let mut f = inflight.lock().unwrap();
            want.retain(|k| f.insert(*k));
        }
        if !want.is_empty() {
            if let Ok(permit) = fill_slots.clone().try_acquire_owned() {
                let (p, cache, inflight) = (fill_rpc.clone(), cache.clone(), inflight.clone());
                tokio::spawn(async move {
                    let got = fill(&p, &want, head).await;
                    drop(permit);
                    cache.lock().unwrap().apply(got, head);
                    let mut f = inflight.lock().unwrap();
                    for k in &want {
                        f.remove(k);
                    }
                });
            } else {
                let mut f = inflight.lock().unwrap();
                for k in &want {
                    f.remove(k);
                }
            }
        }
        // Accuracy: compare complete local runs with eth_simulateV1.
        if out.complete
            && let Ok(permit) = truth_slots.clone().try_acquire_owned()
        {
            let (p, stats) = (truth_rpc.clone(), stats.clone());
            let local = pool_events(&out.logs);
            tokio::spawn(async move {
                let truth = mev_presim::simulate_logs(&p, &st.tx).await;
                drop(permit);
                let mut s = stats.lock().unwrap();
                let Ok(truth) = truth else {
                    s.truth_errors += 1;
                    return;
                };
                let truth: Logs = pool_events(
                    &truth
                        .into_iter()
                        .map(|l| (l.address, l.topics, l.data))
                        .collect(),
                );
                s.compared += 1;
                if truth == local {
                    s.exact += 1;
                }
                if !truth.is_empty() {
                    s.truth_movers += 1;
                    if truth == local {
                        s.movers_exact += 1;
                    }
                } else if !local.is_empty() {
                    s.false_movers += 1;
                }
            });
        }
    }
    report(&stats, &cache, start.elapsed());
    Ok(())
}

fn report(stats: &Arc<Mutex<Stats>>, cache: &Arc<Mutex<StateCache>>, t: Duration) {
    let s = stats.lock().unwrap();
    let mut l = s.local_us.clone();
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
    let (accts, slots) = cache.lock().unwrap().len();
    println!(
        "[{:>4}s] candidates {} | complete {:.0}% (stale-involved {:.0}%) | local us p50 {} p90 {} p99 {} | \
         truth compared {} exact {:.0}% | real pool-movers {} predicted exactly {:.0}% | false movers {} | truth errors {} | cache {} accounts {} slots",
        t.as_secs(),
        s.candidates,
        ratio(s.complete, s.candidates),
        ratio(s.with_stale, s.complete),
        pct(0.5),
        pct(0.9),
        pct(0.99),
        s.compared,
        ratio(s.exact, s.compared),
        s.truth_movers,
        ratio(s.movers_exact, s.truth_movers),
        s.false_movers,
        s.truth_errors,
        accts,
        slots,
    );
}
