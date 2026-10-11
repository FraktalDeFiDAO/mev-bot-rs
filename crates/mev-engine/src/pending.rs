//! Decoded rival swaps whose effect our pool state does not have yet.
//!
//! The sequencer feed shows a competitor's swap the moment it is ordered. Presim then simulates it (tens of
//! milliseconds, and not at all when it is busy, or when the simulation shows no pool effect), and the block log
//! confirms it later. In between, a trade built from our state is built on a pool that has already moved. Measured
//! on 2026-10-08: in 26 of 58 failed races a decoded rival swap on a route pool had arrived before the trade was
//! even built, and presim had skipped or missed 12 of those.
//!
//! This ledger holds each decoded swap from the moment it arrives until presim has applied it, a block log confirms
//! it, or it expires (a transaction that reverts leaves no log). A trade whose pools have an open entry is vetoed.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::{Duration, Instant},
};

use alloy::primitives::{Address, B256};
use mev_decode::{PoolRef, SwapIntent};
use mev_state::PoolStore;
use mev_types::PoolKind;

/// How long an unsettled entry blocks trades on its pools.
pub const PENDING_TTL: Duration = Duration::from_secs(2);

/// A swap read from the feed.
#[derive(Debug, Clone)]
pub struct DecodedSwap {
    pub hash: B256,
    /// The exact swap, when the calldata gives one.
    pub intent: Option<SwapIntent>,
    /// Pools a call names without an exact swap (`Decoded::Pools`): held back like a swap's pools, never used to price.
    pub named_pools: Vec<Address>,
    /// The calldata of a call to an aggregator listed in `mev_decode::POOL_SCAN_DESTINATIONS`: the receiver scans it for
    /// the watched pools it contains (only the engine knows which pools are watched).
    pub scan: Option<Vec<u8>>,
    pub received: Instant,
}

#[derive(Debug, Default)]
pub struct PendingSwaps {
    open: HashMap<B256, (Vec<Address>, Instant)>,
    inserted: AtomicU64,
    settled_by_presim: AtomicU64,
    settled_by_block: AtomicU64,
}

/// Who settled an entry.
#[derive(Debug, Clone, Copy)]
pub enum Settled {
    Presim,
    Block,
}

impl PendingSwaps {
    pub fn insert(&mut self, hash: B256, pools: Vec<Address>, now: Instant) {
        if self.open.len() > 4_096 {
            self.prune(now);
        }
        // A transaction can carry several swaps (a multicall, a forwarder): its pending effect is the union.
        if let Some((have, _)) = self.open.get_mut(&hash) {
            for p in pools {
                if !have.contains(&p) {
                    have.push(p);
                }
            }
            return;
        }
        self.inserted.fetch_add(1, Relaxed);
        self.open.insert(hash, (pools, now));
    }

    /// The swap's effect is now in our state (presim applied it) or confirmed by a block log. Returns whether it was open.
    pub fn settle(&mut self, hash: &B256, by: Settled) -> bool {
        let was_open = self.open.remove(hash).is_some();
        if was_open {
            match by {
                Settled::Presim => &self.settled_by_presim,
                Settled::Block => &self.settled_by_block,
            }
            .fetch_add(1, Relaxed);
        }
        was_open
    }

    /// `inserted / settled by presim / settled by a block log` since the last call, for the minute summary. What is
    /// neither settled nor still open has expired (a swap that reverted leaves no log, presim skipped it, ...).
    pub fn take_counts(&self) -> (u64, u64, u64) {
        (
            self.inserted.swap(0, Relaxed),
            self.settled_by_presim.swap(0, Relaxed),
            self.settled_by_block.swap(0, Relaxed),
        )
    }

    /// An open entry touching any of `pools`, if there is one.
    pub fn conflict(&self, pools: &[Address], now: Instant) -> Option<B256> {
        self.open.iter().find_map(|(h, (ps, at))| {
            (now.saturating_duration_since(*at) < PENDING_TTL
                && ps.iter().any(|p| pools.contains(p)))
            .then_some(*h)
        })
    }

    pub fn prune(&mut self, now: Instant) {
        self.open
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < PENDING_TTL);
    }

    /// Entries still within their time limit. (Expired ones stay in the map until the next sweep; they no longer
    /// count and no longer block anything.)
    pub fn open_count(&self, now: Instant) -> usize {
        self.open
            .values()
            .filter(|(_, at)| now.saturating_duration_since(*at) < PENDING_TTL)
            .count()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// Every watched pool whose 20-byte address appears literally in `input` (at any offset), without repeats.
pub fn watched_pools_in(store: &PoolStore, input: &[u8]) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for w in input.windows(20) {
        let a = Address::from_slice(w);
        if let Some(p) = store.get(&a)
            && !out.contains(&p.info.address)
        {
            out.push(p.info.address);
        }
    }
    out
}

/// The pools in our store a decoded swap acts on. A pool named by address (including V4 stand-ins) is itself; a V3
/// pair or V2 pair is every pool we hold for those tokens (and fee), because the calldata does not say which
/// factory's pool is meant. Empty when none of them is one we watch.
pub fn resolve_pools(store: &PoolStore, intent: &SwapIntent) -> Vec<Address> {
    match &intent.pool {
        PoolRef::Address(a) => store.get(a).map(|p| vec![p.info.address]).unwrap_or_default(),
        PoolRef::Pair { token_a, token_b, fee } => store
            .pools_for(*token_a, *token_b)
            .into_iter()
            .filter(|p| matches!(p.info.kind, PoolKind::UniV3 { fee: f } | PoolKind::PancakeV3 { fee: f } if f == *fee)
                || matches!(p.info.kind, PoolKind::AlgebraV3))
            .map(|p| p.info.address)
            .collect(),
        PoolRef::Tokens { token_a, token_b } => store
            .pools_for(*token_a, *token_b)
            .into_iter()
            .map(|p| p.info.address)
            .collect(),
        PoolRef::PairV2 { token_a, token_b } => store
            .pools_for(*token_a, *token_b)
            .into_iter()
            .filter(|p| matches!(p.info.kind, PoolKind::UniV2 { .. } | PoolKind::CamelotV2))
            .map(|p| p.info.address)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> B256 {
        B256::repeat_byte(n)
    }
    fn a(n: u8) -> Address {
        Address::with_last_byte(n)
    }

    #[test]
    fn the_scan_finds_exactly_the_watched_pools_in_the_calldata() {
        use mev_dex::{Pool, PoolState};
        use mev_types::PoolInfo;
        let mut store = PoolStore::default();
        for n in [0x31u8, 0x32] {
            store.insert(Pool::new(
                PoolInfo {
                    address: a(n),
                    dex: "t".into(),
                    kind: PoolKind::UniV2 { fee_bps: 30 },
                    token0: a(100),
                    token1: a(101),
                },
                PoolState::V2 {
                    reserve0: 1.0,
                    reserve1: 1.0,
                },
            ));
        }
        // Calldata: junk, a watched pool (twice, once unaligned), an unwatched address, then the other watched pool.
        let mut input = vec![0xde, 0xad, 0xbe, 0xef, 7];
        input.extend_from_slice(a(0x31).as_slice());
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(a(0x31).as_slice());
        input.extend_from_slice(a(0x77).as_slice());
        input.extend_from_slice(a(0x32).as_slice());
        assert_eq!(watched_pools_in(&store, &input), vec![a(0x31), a(0x32)]);
        assert!(watched_pools_in(&store, &[0u8; 64]).is_empty());
        assert!(watched_pools_in(&store, &[]).is_empty());
    }

    #[test]
    fn an_open_entry_blocks_trades_on_its_pools_only() {
        let (mut p, t0) = (PendingSwaps::default(), Instant::now());
        p.insert(h(1), vec![a(1), a(2)], t0);
        assert_eq!(p.conflict(&[a(2), a(9)], t0), Some(h(1)));
        assert_eq!(p.conflict(&[a(9)], t0), None, "another pool");
        assert_eq!(p.conflict(&[], t0), None);
    }

    #[test]
    fn a_transaction_with_several_swaps_blocks_the_union_of_their_pools() {
        let (mut p, t0) = (PendingSwaps::default(), Instant::now());
        p.insert(h(1), vec![a(1)], t0);
        p.insert(h(1), vec![a(2), a(1)], t0);
        assert_eq!(p.conflict(&[a(1)], t0), Some(h(1)));
        assert_eq!(
            p.conflict(&[a(2)], t0),
            Some(h(1)),
            "second swap's pool is pending too"
        );
        assert_eq!(p.take_counts().0, 1, "still one pending transaction");
        assert!(p.settle(&h(1), Settled::Block));
        assert!(
            p.is_empty(),
            "one settle clears every pool of the transaction"
        );
    }

    #[test]
    fn settling_clears_the_entry_and_an_unknown_hash_is_harmless() {
        let (mut p, t0) = (PendingSwaps::default(), Instant::now());
        p.insert(h(1), vec![a(1)], t0);
        assert!(!p.settle(&h(7), Settled::Block));
        assert!(p.settle(&h(1), Settled::Presim));
        assert_eq!(p.take_counts(), (1, 1, 0));
        assert_eq!(p.take_counts(), (0, 0, 0), "counts reset");
        assert_eq!(p.conflict(&[a(1)], t0), None);
        assert!(p.is_empty());
    }

    #[test]
    fn an_entry_that_nothing_settles_expires() {
        let (mut p, t0) = (PendingSwaps::default(), Instant::now());
        p.insert(h(1), vec![a(1)], t0);
        assert!(
            p.conflict(&[a(1)], t0 + PENDING_TTL - Duration::from_millis(1))
                .is_some()
        );
        assert_eq!(p.conflict(&[a(1)], t0 + PENDING_TTL), None);
        assert_eq!(p.open_count(t0 + PENDING_TTL - Duration::from_millis(1)), 1);
        assert_eq!(p.open_count(t0 + PENDING_TTL), 0);
        assert_eq!(p.open.len(), 1, "swept only on prune");
        p.prune(t0 + PENDING_TTL);
        assert_eq!(p.open.len(), 0);
    }
}
