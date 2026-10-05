//! Feed-driven local simulation worker: owns the state cache and the pending overlay (no locks),
//! applies block diffs in order, executes every sequenced tx locally, and emits the pool events of
//! complete runs to the engine — the same `Presimulated` messages the RPC pre-simulator sends.
//! Candidates whose state isn't cached yet go to the RPC pre-simulator (fallback) while their
//! missing keys are fetched, so the next similar tx runs locally.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    primitives::B256,
    providers::{DynProvider, Provider},
};
use mev_presim::{Presimulated, Watchlist};
use mev_sequencer::SequencedTx;
use tokio::sync::{Semaphore, mpsc};
use tracing::warn;

use crate::{Fetched, Key, Pending, StateCache, diff, fill};

/// Endpoints: `diff` must serve `debug_traceBlockByNumber` (prestateTracer); `fill` serves state
/// reads at a block; `head` follows the chain tip.
pub struct Endpoints {
    pub diff: DynProvider,
    pub fill: DynProvider,
    pub head: DynProvider,
}

/// Counters for the engine's per-minute line.
#[derive(Debug, Default)]
pub struct LocalStats {
    pub txs: AtomicU64,
    pub local_complete: AtomicU64,
    pub local_moved_pools: AtomicU64,
    pub fallback: AtomicU64,
    pub blocks_applied: AtomicU64,
    pub fills: AtomicU64,
    pub sim_us_total: AtomicU64,
    pub lag_blocks: AtomicU64,
    /// Cache resets after a block diff couldn't be fetched.
    pub resets: AtomicU64,
    /// Keys requested by fills / returned / misses not fetched because every fill slot was busy.
    pub fill_keys: AtomicU64,
    pub fill_keys_ok: AtomicU64,
    pub fill_dropped: AtomicU64,
    /// Pending txs after the last block (gauge) and time spent rebuilding the overlay.
    pub pending: AtomicU64,
    pub rebuild_us_total: AtomicU64,
    /// Missing keys that had already been filled (should stay ~0: a fill must stick).
    pub refills: AtomicU64,
    /// Missing keys per contract over the period (diagnostics: where first-sight misses come from).
    pub miss_by_addr:
        std::sync::Mutex<std::collections::HashMap<alloy::primitives::Address, (u64, u64)>>,
}

impl LocalStats {
    /// Contracts with the most missing keys since the last call: (address, account misses, slot
    /// misses), top `n`.
    pub fn take_top_misses(&self, n: usize) -> Vec<(alloy::primitives::Address, u64, u64)> {
        let m = std::mem::take(&mut *self.miss_by_addr.lock().unwrap_or_else(|e| e.into_inner()));
        let mut v: Vec<_> = m.into_iter().map(|(a, (x, y))| (a, x, y)).collect();
        v.sort_by_key(|(_, x, y)| std::cmp::Reverse(x + y));
        v.truncate(n);
        v
    }

    pub fn take_summary(&self) -> String {
        let t = |a: &AtomicU64| a.swap(0, Relaxed);
        let (txs, complete, moved, fallback) = (
            t(&self.txs),
            t(&self.local_complete),
            t(&self.local_moved_pools),
            t(&self.fallback),
        );
        let (blocks, fills) = (t(&self.blocks_applied), t(&self.fills));
        let (keys, keys_ok, dropped) = (
            t(&self.fill_keys),
            t(&self.fill_keys_ok),
            t(&self.fill_dropped),
        );
        let mean_us = t(&self.sim_us_total).checked_div(txs).unwrap_or(0);
        let rebuild_ms = t(&self.rebuild_us_total) / 1000;
        format!(
            "txs={txs} local_complete={complete} local_moved_pools={moved} rpc_fallback={fallback} \
             blocks_applied={blocks} fills={fills} fill_keys={keys_ok}/{keys} fill_dropped={dropped} \
             mean_sim_us={mean_us} rebuild_ms={rebuild_ms} pending={} refills={} lag_blocks={} resets={}",
            self.pending.load(Relaxed),
            t(&self.refills),
            self.lag_blocks.load(Relaxed),
            self.resets.load(Relaxed)
        )
    }
}

/// A block's changes and the txs it confirmed; `None` = the diff couldn't be fetched.
type BlockDiff = Option<(Vec<diff::Change>, HashSet<B256>)>;

/// Blocks fetched concurrently; applied strictly in order.
const DIFF_IN_FLIGHT: usize = 4;
/// Concurrent state-fill batches.
const FILL_IN_FLIGHT: usize = 4;
/// Per-request limits: a hung request must not stall the in-order block queue or the fill slots.
const DIFF_TIMEOUT: Duration = Duration::from_secs(3);
const DIFF_ATTEMPTS: u64 = 8;
const FILL_TIMEOUT: Duration = Duration::from_secs(5);
/// Backstop head poll (the feed advances the head in real time).
const HEAD_POLL: Duration = Duration::from_secs(2);
/// A feed-derived head this far beyond the polled one is not trusted (protects the diff fetcher
/// from requesting blocks that don't exist if the sequence-number mapping ever changes).
const MAX_FEED_HEAD_LEAD: u64 = 64;

/// Run until the feed closes. `fallback` receives candidate txs that couldn't run locally.
pub async fn run(
    mut feed: mpsc::UnboundedReceiver<SequencedTx>,
    rpc: Endpoints,
    watch: Watchlist,
    out: mpsc::UnboundedSender<Presimulated>,
    fallback: mpsc::UnboundedSender<SequencedTx>,
    stats: Arc<LocalStats>,
) {
    let start = loop {
        match rpc.head.get_block_number().await {
            Ok(n) => break n,
            Err(e) => {
                warn!(error = %format!("{e:#}"), "localsim: head unavailable; retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    let mut cache = StateCache::new(u64::MAX);
    cache.head = start;
    cache.applied = start;
    let mut pending = Pending::default();

    // Chain head: advanced by every feed tx (its block, no RPC); a slow poll is the backstop if
    // the feed stalls. Polling at block rate got the shared public endpoint to rate-limit the IP.
    let head = Arc::new(AtomicU64::new(start));
    {
        let (head, p) = (head.clone(), rpc.head.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEAD_POLL).await;
                if let Ok(n) = p.get_block_number().await {
                    head.fetch_max(n, Relaxed);
                }
            }
        });
    }
    // Diff fetcher: results arrive out of order; applied in order below.
    let (dtx, mut drx) = mpsc::unbounded_channel::<(u64, BlockDiff)>();
    {
        let (head, p) = (head.clone(), rpc.diff.clone());
        tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(DIFF_IN_FLIGHT));
            let mut next = start + 1;
            loop {
                if dtx.is_closed() {
                    return;
                }
                if next > head.load(Relaxed) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                let n = next;
                next += 1;
                let Ok(permit) = slots.clone().acquire_owned().await else {
                    return;
                };
                let (p, dtx) = (p.clone(), dtx.clone());
                tokio::spawn(async move {
                    // A skipped block would leave the cache silently wrong: retry with a timeout,
                    // then report the gap (`None`) so the worker resets instead of stalling.
                    let mut got = None;
                    for attempt in 0..DIFF_ATTEMPTS {
                        if let Ok(Ok(b)) =
                            tokio::time::timeout(DIFF_TIMEOUT, diff::fetch_block_diff(&p, n)).await
                        {
                            got = Some((diff::changes(&b), diff::tx_hashes(&b)));
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(150 * (attempt + 1))).await;
                    }
                    let _ = dtx.send((n, got));
                    drop(permit);
                });
            }
        });
    }
    let (ftx, mut frx) = mpsc::unbounded_channel::<(u64, Vec<Key>, Vec<(Key, Fetched)>)>();
    let fill_slots = Arc::new(Semaphore::new(FILL_IN_FLIGHT));
    let mut inflight: HashSet<Key> = HashSet::new();
    // Keys a fill returned, for the `refills` health counter (cleared on reset).
    let mut filled: HashSet<Key> = HashSet::new();
    let mut buffered: BTreeMap<u64, BlockDiff> = BTreeMap::new();

    loop {
        tokio::select! {
            biased;
            // Block diffs first: they make the base exact.
            Some((n, got)) = drx.recv() => {
                buffered.insert(n, got);
                while let Some(got) = buffered.remove(&(cache.applied + 1)) {
                    let n = cache.applied + 1;
                    match got {
                        Some((ch, hashes)) => {
                            cache.apply_block(n, ch);
                            let t = std::time::Instant::now();
                            pending.on_block(&cache, &hashes);
                            stats.rebuild_us_total.fetch_add(t.elapsed().as_micros() as u64, Relaxed);
                            stats.pending.store(pending.len() as u64, Relaxed);
                            stats.blocks_applied.fetch_add(1, Relaxed);
                        }
                        None => {
                            warn!(block = n, "localsim: block diff unavailable; resetting the state cache");
                            cache.reset(n);
                            inflight.clear();
                            filled.clear();
                            pending.on_block(&cache, &HashSet::new());
                            stats.resets.fetch_add(1, Relaxed);
                        }
                    }
                }
                stats.lag_blocks.store(head.load(Relaxed).saturating_sub(cache.applied), Relaxed);
            }
            Some((at, wanted, got)) = frx.recv() => {
                // Release everything requested; keys that failed are refetched on the next miss.
                for k in &wanted {
                    inflight.remove(k);
                }
                // After a reset the fetched values may predate it: only merge recent fills.
                stats.fill_keys_ok.fetch_add(got.len() as u64, Relaxed);
                filled.extend(got.iter().map(|(k, _)| *k));
                if at <= cache.applied && cache.applied - at < 64 {
                    cache.apply(got, at);
                }
            }
            maybe = feed.recv() => {
                let Some(st) = maybe else { return };
                stats.txs.fetch_add(1, Relaxed);
                let b = st.block();
                if b <= head.load(Relaxed) + MAX_FEED_HEAD_LEAD {
                    head.fetch_max(b, Relaxed);
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let Ok(o) = pending.push(&cache, st.hash, st.tx.clone(), now) else { continue };
                stats.sim_us_total.fetch_add(o.elapsed.as_micros() as u64, Relaxed);
                let candidate = mev_presim::is_candidate(&st)
                    && mev_presim::mentions_any(&st.input, &watch.read().unwrap_or_else(|e| e.into_inner()));
                if o.complete {
                    stats.local_complete.fetch_add(1, Relaxed);
                    let logs: Vec<_> = o
                        .logs
                        .iter()
                        .filter(|(_, t, _)| t.first().is_some_and(mev_presim::is_pool_event))
                        .map(|(a, t, d)| mev_presim::to_rpc_log(&mev_presim::SimLog {
                            address: *a,
                            topics: t.clone(),
                            data: d.clone(),
                        }))
                        .collect();
                    if !logs.is_empty() {
                        stats.local_moved_pools.fetch_add(1, Relaxed);
                        let _ = out.send(Presimulated { tx: st.hash, received: st.received, logs });
                    }
                    continue;
                }
                if !candidate {
                    continue;
                }
                // Missing state: RPC pre-simulation for this one, fetch its keys for next time.
                stats.fallback.fetch_add(1, Relaxed);
                let _ = fallback.send(st);
                {
                    let mut m = stats.miss_by_addr.lock().unwrap_or_else(|e| e.into_inner());
                    for k in &o.missing {
                        match k {
                            Key::Account(a) => m.entry(*a).or_default().0 += 1,
                            Key::Slot(a, _) => m.entry(*a).or_default().1 += 1,
                        }
                    }
                }
                stats
                    .refills
                    .fetch_add(o.missing.iter().filter(|k| filled.contains(k)).count() as u64, Relaxed);
                let want: Vec<Key> = o.missing.into_iter().filter(|k| inflight.insert(*k)).collect();
                if want.is_empty() {
                    continue;
                }
                let Ok(permit) = fill_slots.clone().try_acquire_owned() else {
                    stats.fill_dropped.fetch_add(1, Relaxed);
                    for k in &want {
                        inflight.remove(k);
                    }
                    continue;
                };
                stats.fills.fetch_add(1, Relaxed);
                stats.fill_keys.fetch_add(want.len() as u64, Relaxed);
                let (p, ftx, at) = (rpc.fill.clone(), ftx.clone(), cache.applied);
                tokio::spawn(async move {
                    let got = tokio::time::timeout(FILL_TIMEOUT, fill(&p, &want, at))
                        .await
                        .unwrap_or_default();
                    drop(permit);
                    let _ = ftx.send((at, want, got));
                });
            }
        }
    }
}
