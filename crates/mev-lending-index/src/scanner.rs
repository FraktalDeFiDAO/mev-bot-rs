//! Range log fetching shared by every event index: shrink-on-too-large, bounded retry with the
//! workspace [`Backoff`] on rate limits and transient errors, and an explicit [`Fetched::Gap`]
//! when retries run out so a failed range is recorded instead of silently skipped or aborting the
//! whole scan step.

use std::time::Duration;

use alloy::{
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use mev_rpc::Backoff;
use tracing::warn;

/// Bounded retry: at most `attempts` retries of one range, each delayed by `Backoff` capped at
/// `max_delay`.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 4,
            max_delay: Duration::from_secs(8),
        }
    }
}

pub struct LogQuery<'a> {
    pub addresses: &'a [Address],
    pub topics: &'a [B256],
    pub name: &'a str,
}

#[derive(Debug)]
pub enum Fetched {
    /// Logs for exactly `[from, to]` (possibly a shrunk part of what was asked).
    Logs { logs: Vec<Log>, from: u64, to: u64 },
    /// `[from, to]` could not be fetched within the retry bound; the caller must record it.
    Gap { from: u64, to: u64, reason: String },
    /// The request budget ran out before anything was fetched; nothing happened.
    Deferred,
}

/// Fetch logs for `[from, to]`. A "range too large" style error shrinks the range (from the far
/// end: `to` going forward, `from` going backward) and remembers the size in `step`.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_range(
    p: &DynProvider,
    q: &LogQuery<'_>,
    mut from: u64,
    mut to: u64,
    backward: bool,
    step: &mut u64,
    throttle: Duration,
    retry: &RetryPolicy,
    budget: &mut usize,
) -> Fetched {
    let mut backoff = Backoff::default();
    let mut tries = 0u32;
    // A range that failed once is retried as is before it is shrunk: some public endpoints
    // (Pocket on Base, 2026-10-16) answer a valid query with a transient HTTP 400.
    let mut retried = false;
    loop {
        if *budget == 0 {
            return Fetched::Deferred;
        }
        *budget -= 1;
        tokio::time::sleep(throttle).await;
        let filter = Filter::new()
            .address(q.addresses.to_vec())
            .event_signature(q.topics.to_vec())
            .from_block(from)
            .to_block(to);
        let err = match p.get_logs(&filter).await {
            Ok(logs) => return Fetched::Logs { logs, from, to },
            Err(e) => anyhow::Error::from(e),
        };
        let limited = mev_rpc::is_rate_limit(&err);
        if to > from && !limited && !retried {
            retried = true;
            continue;
        }
        if to > from && !limited {
            retried = false;
            // Too many results / range too wide: shrink and retry immediately.
            if backward {
                from = to - (to - from) / 4;
            } else {
                to = from + (to - from) / 4;
            }
            *step = (to - from + 1).max(1);
            continue;
        }
        tries += 1;
        if tries > retry.attempts {
            let reason = format!("{} log scan {from}-{to}: {err:#}", q.name);
            warn!(index = q.name, from, to, %reason, "range left as a coverage gap");
            return Fetched::Gap { from, to, reason };
        }
        let delay = backoff.next_delay().min(retry.max_delay);
        warn!(index = q.name, from, to, tries, ?delay, error = %format!("{err:#}"), "log scan retry");
        tokio::time::sleep(delay).await;
    }
}

/// Block-hash ancestry check. `ring` is newest-first `(number, hash)` points recorded while
/// scanning. Returns `Consistent` when the newest point is still canonical, otherwise the newest
/// recorded point that still is (binary search: hash equality at a height implies equality of all
/// ancestors), or the canonical block just below the oldest recorded one when none survives.
pub const NO_MATCH_MARGIN: u64 = 128;

#[derive(Debug, PartialEq, Eq)]
pub enum Reorg {
    Consistent,
    /// The provider does not know the newest recorded block (a lagging backend behind a load
    /// balancer): nothing can be concluded, so nothing is rewound.
    Unverified {
        height: u64,
    },
    Rewind {
        ancestor: (u64, B256),
    },
}

async fn canonical_hash(p: &DynProvider, number: u64) -> anyhow::Result<Option<B256>> {
    Ok(p.get_block_by_number(number.into())
        .await?
        .map(|b| b.header.hash))
}

pub async fn locate_reorg(p: &DynProvider, ring: &[(u64, B256)]) -> anyhow::Result<Reorg> {
    let Some(&(n0, h0)) = ring.first() else {
        return Ok(Reorg::Consistent);
    };
    match canonical_hash(p, n0).await? {
        Some(h) if h == h0 => return Ok(Reorg::Consistent),
        None => return Ok(Reorg::Unverified { height: n0 }),
        Some(_) => {}
    }
    // Invariant: ring[lo] mismatches; ring[hi] matches (hi == len means "virtual match").
    let (mut lo, mut hi) = (0usize, ring.len());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let (n, h) = ring[mid];
        if canonical_hash(p, n).await? == Some(h) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    if hi < ring.len() {
        return Ok(Reorg::Rewind { ancestor: ring[hi] });
    }
    // Every recorded point is gone, so the fork point is somewhere below all of them and the
    // block just under the oldest one may itself be on the new fork. Go back a fixed margin
    // (rescanning is idempotent; scanning too little is not).
    let below = ring[ring.len() - 1].0.saturating_sub(NO_MATCH_MARGIN);
    match canonical_hash(p, below).await? {
        Some(h) => Ok(Reorg::Rewind {
            ancestor: (below, h),
        }),
        None => anyhow::bail!("no canonical block at height {below} to rewind to"),
    }
}

/// `eth_blockNumber` with the same bounded retry/backoff as range fetches.
pub async fn head_with_retry(p: &DynProvider, retry: &RetryPolicy) -> anyhow::Result<u64> {
    let mut backoff = Backoff::default();
    let mut tries = 0u32;
    loop {
        match p.get_block_number().await {
            Ok(n) => return Ok(n),
            Err(e) => {
                tries += 1;
                if tries > retry.attempts {
                    return Err(anyhow::Error::from(e).context("eth_blockNumber"));
                }
                tokio::time::sleep(backoff.next_delay().min(retry.max_delay)).await;
            }
        }
    }
}
