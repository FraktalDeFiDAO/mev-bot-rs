//! In-memory pool store: indexes pools by token pair and applies on-chain events. No I/O.

use std::collections::HashMap;

use alloy::{rpc::types::Log, sol_types::SolEvent};
use mev_bindings::{IPancakeV3Pool, IPoolManager, IUniswapV2Pair, IUniswapV3Pool};
use mev_dex::{Pool, PoolState, sqrt_price_from_x96};
use mev_types::{Address, B256, PoolKind};

/// A pool whose presim-applied (tentative) state never got confirmed by a real log and was rolled back.
#[derive(Debug, Clone)]
pub struct Rollback {
    pub pool: Address,
    /// The unconfirmed state that had been applied.
    pub tentative: PoolState,
    /// The confirmed state it was restored to.
    pub restored: PoolState,
}

/// What a background state read changed in the store.
#[derive(Debug, Default)]
pub struct RefreshOutcome {
    /// Pools whose state was replaced by the fresh read.
    pub applied: usize,
    /// Pools left alone because an event updated them after the read began (the event is newer).
    pub skipped: usize,
    /// Applied pools whose fresh state differed from what we held: the chain won.
    pub overwrites: Vec<Overwrite>,
}

/// One cached state replaced by the chain's (becomes a `StateCorrection` record).
#[derive(Debug, Clone)]
pub struct Overwrite {
    pub pool: Address,
    pub predicted: PoolState,
    pub actual: PoolState,
}

#[derive(Default, Clone)]
pub struct PoolStore {
    pools: HashMap<Address, Pool>,
    /// Bumped each time a pool's state or tick map actually changes: an opportunity records the
    /// versions it was computed from and is dropped if any moved (invariant 3).
    versions: HashMap<Address, u64>,
    by_pair: HashMap<(Address, Address), Vec<Address>>,
    /// Uniswap V4 pool id -> the stand-in address it is stored under (their events come from the
    /// shared PoolManager and name the pool only by id).
    v4: HashMap<B256, Address>,
    /// Pools holding a state applied from a **pending** feed transaction (presim), with the confirmed state and tick
    /// table from before it and when it was last applied. A real block log, a resync or a replacement confirms the
    /// pool and clears the entry; `expire_tentative` rolls back what nothing confirmed (a transaction that reverted
    /// or never landed leaves no log).
    tentative: HashMap<
        Address,
        (
            PoolState,
            Option<std::sync::Arc<mev_dex::TickMap>>,
            std::time::Instant,
        ),
    >,
}

/// Relative tolerance below which two states are the same pool state (they differ only by floating-point
/// rounding in how they were derived); such a read is applied but not recorded as a correction.
const SAME_STATE_REL: f64 = 1e-12;

/// Do two states of one pool differ by more than rounding?
pub fn materially_different(a: &PoolState, b: &PoolState) -> bool {
    let far = |x: f64, y: f64| {
        (x - y).abs() > SAME_STATE_REL * x.abs().max(y.abs()).max(f64::MIN_POSITIVE)
    };
    match (a, b) {
        (
            PoolState::V2 {
                reserve0: a0,
                reserve1: a1,
            },
            PoolState::V2 {
                reserve0: b0,
                reserve1: b1,
            },
        ) => far(*a0, *b0) || far(*a1, *b1),
        (
            PoolState::V3 {
                sqrt_price: ap,
                liquidity: al,
            },
            PoolState::V3 {
                sqrt_price: bp,
                liquidity: bl,
            },
        ) => far(*ap, *bp) || far(*al, *bl),
        _ => true,
    }
}

fn pair_key(a: Address, b: Address) -> (Address, Address) {
    if a < b { (a, b) } else { (b, a) }
}

impl PoolStore {
    pub fn insert(&mut self, pool: Pool) {
        let key = pair_key(pool.info.token0, pool.info.token1);
        let addr = pool.info.address;
        if let PoolKind::UniV4 { id, .. } = pool.info.kind {
            self.v4.insert(id, addr);
        }
        if self.pools.insert(addr, pool).is_none() {
            self.by_pair.entry(key).or_default().push(addr);
        }
        self.bump(addr);
    }

    fn bump(&mut self, addr: Address) {
        *self.versions.entry(addr).or_insert(0) += 1;
    }

    /// The pool's state version (0 if unknown).
    pub fn version(&self, addr: &Address) -> u64 {
        self.versions.get(addr).copied().unwrap_or(0)
    }

    /// Versions of every pool now, to compare against later (a background read records them first).
    pub fn versions_snapshot(&self) -> HashMap<Address, u64> {
        self.versions.clone()
    }

    /// Apply a background read of pool states. A pool that an event updated after the read began
    /// (`read_at` older than its version) is skipped: the event is newer than the read. Every
    /// applied pool whose state differs from ours is reported as an overwrite.
    pub fn apply_refresh(
        &mut self,
        fresh: Vec<Pool>,
        read_at: &HashMap<Address, u64>,
    ) -> RefreshOutcome {
        let mut out = RefreshOutcome::default();
        for mut p in fresh {
            let addr = p.info.address;
            let Some(old) = self.pools.get(&addr) else {
                continue;
            };
            if self.version(&addr) != read_at.get(&addr).copied().unwrap_or(0) {
                out.skipped += 1;
                continue;
            }
            p.ticks = old.ticks.clone();
            if materially_different(&old.state, &p.state) {
                out.overwrites.push(Overwrite {
                    pool: addr,
                    predicted: old.state.clone(),
                    actual: p.state.clone(),
                });
            }
            self.replace(p);
            out.applied += 1;
        }
        out
    }

    pub fn get(&self, addr: &Address) -> Option<&Pool> {
        self.pools.get(addr)
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    pub fn addresses(&self) -> Vec<Address> {
        self.pools.keys().copied().collect()
    }

    /// The contracts whose logs move our pools: each pool's own address, and for Uniswap V4 pools the
    /// shared PoolManager (once).
    pub fn log_addresses(&self) -> Vec<Address> {
        let mut out: Vec<Address> = self
            .pools
            .values()
            .map(|p| match p.info.kind {
                PoolKind::UniV4 { manager, .. } => manager,
                _ => p.info.address,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn pools(&self) -> impl Iterator<Item = &Pool> {
        self.pools.values()
    }

    /// All pools trading the same pair as `addr` (including itself).
    pub fn siblings(&self, addr: &Address) -> Vec<&Pool> {
        let Some(p) = self.pools.get(addr) else {
            return vec![];
        };
        self.pools_for(p.info.token0, p.info.token1)
    }

    pub fn pools_for(&self, a: Address, b: Address) -> Vec<&Pool> {
        self.by_pair
            .get(&pair_key(a, b))
            .map(|v| v.iter().filter_map(|x| self.pools.get(x)).collect())
            .unwrap_or_default()
    }

    /// Replace an existing pool's state and fees (e.g. after a resync). Unknown pools are ignored.
    pub fn replace(&mut self, pool: Pool) {
        let addr = pool.info.address;
        self.tentative.remove(&addr);
        if let Some(p) = self.pools.get_mut(&addr) {
            let changed = p.state != pool.state || p.fee0 != pool.fee0 || p.fee1 != pool.fee1;
            *p = pool;
            if changed {
                self.bump(addr);
            }
        }
    }

    /// Replace a pool's tick map (concentrated-liquidity pricing across tick boundaries).
    pub fn set_ticks(&mut self, addr: &Address, ticks: mev_dex::TickMap) {
        if let Some(p) = self.pools.get_mut(addr) {
            let changed = p.ticks.as_deref() != Some(&ticks);
            p.ticks = Some(std::sync::Arc::new(ticks));
            if changed {
                self.bump(*addr);
            }
        }
    }

    pub fn set_state(&mut self, addr: &Address, state: PoolState) {
        self.tentative.remove(addr);
        if let Some(p) = self.pools.get_mut(addr) {
            let changed = p.state != state;
            p.state = state;
            if changed {
                self.bump(*addr);
            }
        }
    }

    /// A V3-style `Mint` (`add`) or `Burn`: liquidity over `[tickLower, tickUpper)` changes the pool's
    /// active liquidity only if its current tick is inside that range, and always moves the tick
    /// table's `liquidityNet` (+amount at the lower tick, -amount at the upper; reversed for a burn)
    /// when those ticks are inside the loaded window.
    fn apply_v3_liquidity(&mut self, log: &Log, add: bool) -> Option<Address> {
        let addr = log.address();
        let (lo, hi, amount) = if add {
            let ev = IUniswapV3Pool::Mint::decode_log_data(log.data()).ok()?;
            (ev.tickLower.as_i32(), ev.tickUpper.as_i32(), ev.amount)
        } else {
            let ev = IUniswapV3Pool::Burn::decode_log_data(log.data()).ok()?;
            (ev.tickLower.as_i32(), ev.tickUpper.as_i32(), ev.amount)
        };
        if amount == 0 {
            return None; // a "poke" collects fees, no liquidity moves
        }
        let pool = self.pools.get_mut(&addr)?;
        if !matches!(
            pool.info.kind,
            PoolKind::UniV3 { .. } | PoolKind::PancakeV3 { .. } | PoolKind::AlgebraV3
        ) {
            return None;
        }
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = pool.state
        else {
            return None;
        };
        let delta = if add { amount as f64 } else { -(amount as f64) };
        let tick = ((sqrt_price * sqrt_price).ln() / 1.0001f64.ln()).floor() as i32;
        if lo <= tick && tick < hi {
            pool.state = PoolState::V3 {
                sqrt_price,
                liquidity: (liquidity + delta).max(0.0),
            };
        }
        if let Some(map) = pool.ticks.as_deref() {
            let mut map = map.clone();
            for (t, d) in [(lo, delta), (hi, -delta)] {
                if t < map.lo || t > map.hi {
                    continue;
                }
                match map.ticks.binary_search_by_key(&t, |(k, _)| *k) {
                    Ok(i) => map.ticks[i].1 += d,
                    Err(i) => map.ticks.insert(i, (t, d)),
                }
            }
            pool.ticks = Some(std::sync::Arc::new(map));
        }
        self.bump(addr);
        Some(addr)
    }

    /// A V4 `ModifyLiquidity`: liquidity added or removed over `[tickLower, tickUpper)` changes the
    /// active liquidity only if the pool's current tick is inside that range. Without this the pool's
    /// depth would stay stale until the next periodic refresh.
    fn apply_v4_liquidity(&mut self, log: &Log) -> Option<Address> {
        let id = *log.topics().get(1)?;
        let addr = *self.v4.get(&id)?;
        let pool = self.pools.get_mut(&addr)?;
        let PoolKind::UniV4 {
            manager, flipped, ..
        } = pool.info.kind
        else {
            return None;
        };
        if manager != log.address() {
            return None;
        }
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = pool.state
        else {
            return None;
        };
        let ev = IPoolManager::ModifyLiquidity::decode_log_data(log.data()).ok()?;
        // The pool's own price orientation (ours is mirrored when flipped).
        let sqrt = if flipped {
            1.0 / sqrt_price
        } else {
            sqrt_price
        };
        let tick = (2.0 * sqrt.ln() / 1.0001f64.ln()).floor();
        let (lo, hi) = (ev.tickLower.as_i32() as f64, ev.tickUpper.as_i32() as f64);
        if !(lo <= tick && tick < hi) {
            return None; // outside the active range: depth at the current price is unchanged
        }
        let delta: f64 = ev.liquidityDelta.to_string().parse().ok()?;
        pool.state = PoolState::V3 {
            sqrt_price,
            liquidity: (liquidity + delta).max(0.0),
        };
        Some(addr)
    }

    /// A V4 `Swap` from the PoolManager: new price and active liquidity, and the fee that swap paid
    /// (protocol + LP) for its direction. Pools we don't track are ignored.
    fn apply_v4_swap(&mut self, log: &Log) -> Option<Address> {
        let id = *log.topics().get(1)?;
        let addr = *self.v4.get(&id)?;
        let pool = self.pools.get_mut(&addr)?;
        let PoolKind::UniV4 {
            manager, flipped, ..
        } = pool.info.kind
        else {
            return None;
        };
        if manager != log.address() {
            return None;
        }
        let ev = IPoolManager::Swap::decode_log_data(log.data()).ok()?;
        let sqrt = sqrt_price_from_x96(f64::from(ev.sqrtPriceX96));
        if sqrt <= 0.0 {
            return None;
        }
        pool.state = PoolState::V3 {
            // Flipped pools store the mirrored price (token order reversed).
            sqrt_price: if flipped { 1.0 / sqrt } else { sqrt },
            liquidity: ev.liquidity as f64,
        };
        // amount0 < 0: the swapper paid currency0, i.e. a zeroForOne swap; its fee is that
        // direction's. Mirrored when our token order is flipped.
        let fee = ev.fee.to::<u32>() as f64 / 1_000_000.0;
        if ev.amount0.is_negative() != flipped {
            pool.fee0 = fee;
        } else {
            pool.fee1 = fee;
        }
        Some(addr)
    }

    /// Apply a V2 `Sync` or V3 `Swap` log. Returns the pool address if state changed.
    /// The pool a log would update, if any (for V4 the pool named by the event's id).
    fn log_pool(&self, log: &Log) -> Option<Address> {
        if log.address() == Address::ZERO {
            return None;
        }
        match log.topics().get(1) {
            Some(id) if self.v4.contains_key(id) => self.v4.get(id).copied(),
            _ => self
                .pools
                .contains_key(&log.address())
                .then(|| log.address()),
        }
    }

    /// Apply a **real** pool event (a block log): it is absolute state from the chain, so it also confirms the pool
    /// (any tentative presim state is dropped). Returns the pool it updated.
    pub fn apply_log(&mut self, log: &Log) -> Option<Address> {
        let updated = self.apply_log_core(log);
        if let Some(a) = updated {
            self.tentative.remove(&a);
        }
        updated
    }

    /// Apply an event from a **pending** transaction (presim). The confirmed state is remembered; unless a real log,
    /// a resync or a replacement confirms the pool, `expire_tentative` restores it.
    pub fn apply_log_tentative(&mut self, log: &Log, now: std::time::Instant) -> Option<Address> {
        let target = self.log_pool(log)?;
        let snapshot = self
            .pools
            .get(&target)
            .map(|p| (p.state.clone(), p.ticks.clone()))?;
        let updated = self.apply_log_core(log)?;
        self.tentative
            .entry(updated)
            .and_modify(|e| e.2 = now)
            .or_insert((snapshot.0, snapshot.1, now));
        Some(updated)
    }

    /// Expire every tentative state older than `ttl` that nothing confirmed. With `restore` the confirmed state
    /// comes back; without it the state is left as it is and only the miss is reported (observe-only).
    pub fn expire_tentative(
        &mut self,
        now: std::time::Instant,
        ttl: std::time::Duration,
        restore: bool,
    ) -> Vec<Rollback> {
        let due: Vec<Address> = self
            .tentative
            .iter()
            .filter(|(_, (_, _, at))| now.saturating_duration_since(*at) >= ttl)
            .map(|(a, _)| *a)
            .collect();
        let mut out = Vec::new();
        for a in due {
            let Some((state, ticks, _)) = self.tentative.remove(&a) else {
                continue;
            };
            let Some(p) = self.pools.get_mut(&a) else {
                continue;
            };
            let tentative = if restore {
                let t = std::mem::replace(&mut p.state, state.clone());
                p.ticks = ticks;
                self.bump(a);
                t
            } else {
                p.state.clone()
            };
            out.push(Rollback {
                pool: a,
                tentative,
                restored: state,
            });
        }
        out
    }

    /// Pools currently holding unconfirmed (presim) state.
    pub fn tentative_len(&self) -> usize {
        self.tentative.len()
    }

    /// Shared by real and tentative application: the pool's version moves only when its state actually changed.
    fn apply_log_core(&mut self, log: &Log) -> Option<Address> {
        let before = self
            .log_pool(log)
            .and_then(|a| self.pools.get(&a).map(|p| (a, p.state.clone())));
        let updated = self.apply_log_inner(log);
        if let (Some(a), Some((b, old))) = (updated, before) {
            if a != b || self.pools.get(&a).is_some_and(|p| p.state != old) {
                self.bump(a);
            }
        } else if let Some(a) = updated {
            self.bump(a);
        }
        updated
    }

    fn apply_log_inner(&mut self, log: &Log) -> Option<Address> {
        let addr = log.address();
        let topic0: B256 = *log.topic0()?;
        if topic0 == IPoolManager::Swap::SIGNATURE_HASH {
            return self.apply_v4_swap(log);
        }
        if topic0 == IPoolManager::ModifyLiquidity::SIGNATURE_HASH {
            return self.apply_v4_liquidity(log);
        }
        if topic0 == IUniswapV3Pool::Mint::SIGNATURE_HASH {
            return self.apply_v3_liquidity(log, true);
        }
        if topic0 == IUniswapV3Pool::Burn::SIGNATURE_HASH {
            return self.apply_v3_liquidity(log, false);
        }
        let pool = self.pools.get_mut(&addr)?;
        if topic0 == IUniswapV2Pair::Sync::SIGNATURE_HASH {
            let ev = IUniswapV2Pair::Sync::decode_log_data(log.data()).ok()?;
            pool.state = PoolState::V2 {
                reserve0: f64::from(ev.reserve0),
                reserve1: f64::from(ev.reserve1),
            };
            Some(addr)
        } else if topic0 == IPancakeV3Pool::Swap::SIGNATURE_HASH {
            let ev = IPancakeV3Pool::Swap::decode_log_data(log.data()).ok()?;
            pool.state = PoolState::V3 {
                sqrt_price: sqrt_price_from_x96(f64::from(ev.sqrtPriceX96)),
                liquidity: ev.liquidity as f64,
            };
            Some(addr)
        } else if topic0 == IUniswapV3Pool::Swap::SIGNATURE_HASH {
            let ev = IUniswapV3Pool::Swap::decode_log_data(log.data()).ok()?;
            pool.state = PoolState::V3 {
                sqrt_price: sqrt_price_from_x96(f64::from(ev.sqrtPriceX96)),
                liquidity: ev.liquidity as f64,
            };
            Some(addr)
        } else {
            None
        }
    }

    /// USD price of one raw unit of `token`, derived from the deepest pool against the USD token
    /// or (via native) against native/USD.
    pub fn usd_per_raw_unit(
        &self,
        token: Address,
        usd_token: Address,
        usd_decimals: u8,
        native: Address,
    ) -> Option<f64> {
        let usd_unit = 10f64.powi(-(usd_decimals as i32));
        if token == usd_token {
            return Some(usd_unit);
        }
        let direct = self.rate(token, usd_token);
        if let Some(r) = direct {
            return Some(r * usd_unit);
        }
        let to_native = self.rate(token, native)?;
        let native_usd = self.rate(native, usd_token)?;
        Some(to_native * native_usd * usd_unit)
    }

    /// Mid rate token -> other (raw units) from the deepest pool for the pair.
    fn rate(&self, token: Address, other: Address) -> Option<f64> {
        let best = self
            .pools_for(token, other)
            .into_iter()
            .max_by(|a, b| a.reserve_of(token).total_cmp(&b.reserve_of(token)))?;
        let p0 = best.price0();
        if p0 <= 0.0 {
            return None;
        }
        Some(if best.info.token0 == token {
            p0
        } else {
            1.0 / p0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, I256, U160, U256, aliases::I24, aliases::U112};
    use mev_bindings::IPancakeV3Pool;
    use mev_types::{PoolInfo, PoolKind};

    const A: Address = Address::with_last_byte(0xa);
    const B: Address = Address::with_last_byte(0xb);
    const USD: Address = Address::with_last_byte(0xc);

    fn pool(addr: u8, kind: PoolKind, t0: Address, t1: Address, state: PoolState) -> Pool {
        Pool::new(
            PoolInfo {
                address: Address::with_last_byte(addr),
                dex: "t".into(),
                kind,
                token0: t0,
                token1: t1,
            },
            state,
        )
    }

    fn rpc_log(address: Address, data: alloy::primitives::LogData) -> Log {
        Log {
            inner: alloy::primitives::Log { address, data },
            ..Default::default()
        }
    }

    #[test]
    fn indexes_by_pair_and_ignores_duplicate_inserts() {
        let mut s = PoolStore::default();
        let v2 = PoolKind::UniV2 { fee_bps: 30 };
        s.insert(pool(
            1,
            v2,
            A,
            B,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        ));
        s.insert(pool(
            2,
            v2,
            A,
            B,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        ));
        s.insert(pool(
            1,
            v2,
            A,
            B,
            PoolState::V2 {
                reserve0: 9.0,
                reserve1: 9.0,
            },
        ));
        assert_eq!(s.len(), 2);
        assert_eq!(s.siblings(&Address::with_last_byte(1)).len(), 2);
        assert_eq!(
            s.pools_for(B, A).len(),
            2,
            "pair lookup is order-independent"
        );
        assert!(s.siblings(&Address::with_last_byte(9)).is_empty());
    }

    #[test]
    fn applies_v2_sync() {
        let mut s = PoolStore::default();
        s.insert(pool(
            1,
            PoolKind::UniV2 { fee_bps: 30 },
            A,
            B,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        ));
        let ev = IUniswapV2Pair::Sync {
            reserve0: U112::from(500),
            reserve1: U112::from(700),
        };
        let addr = Address::with_last_byte(1);
        assert_eq!(
            s.apply_log(&rpc_log(addr, ev.encode_log_data())),
            Some(addr)
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 500.0,
                reserve1: 700.0
            }
        );
    }

    #[test]
    fn applies_v3_and_pancake_swaps() {
        let mut s = PoolStore::default();
        let start = PoolState::V3 {
            sqrt_price: 1.0,
            liquidity: 1.0,
        };
        s.insert(pool(1, PoolKind::UniV3 { fee: 500 }, A, B, start.clone()));
        s.insert(pool(2, PoolKind::PancakeV3 { fee: 500 }, A, B, start));
        let q96 = U160::from(1u8) << 96; // sqrt price 1.0
        let uni = IUniswapV3Pool::Swap {
            sender: A,
            recipient: B,
            amount0: I256::ZERO,
            amount1: I256::ZERO,
            sqrtPriceX96: q96 * U160::from(2u8),
            liquidity: 42,
            tick: I24::ZERO,
        };
        assert!(
            s.apply_log(&rpc_log(Address::with_last_byte(1), uni.encode_log_data()))
                .is_some()
        );
        assert_eq!(
            s.get(&Address::with_last_byte(1)).unwrap().state,
            PoolState::V3 {
                sqrt_price: 2.0,
                liquidity: 42.0
            }
        );
        let pcs = IPancakeV3Pool::Swap {
            sender: A,
            recipient: B,
            amount0: I256::ZERO,
            amount1: I256::ZERO,
            sqrtPriceX96: q96,
            liquidity: 7,
            tick: I24::ZERO,
            protocolFeesToken0: 0,
            protocolFeesToken1: 0,
        };
        assert!(
            s.apply_log(&rpc_log(Address::with_last_byte(2), pcs.encode_log_data()))
                .is_some()
        );
        assert_eq!(
            s.get(&Address::with_last_byte(2)).unwrap().state,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 7.0
            }
        );
    }

    #[test]
    fn ignores_unknown_pools_and_events() {
        let mut s = PoolStore::default();
        let ev = IUniswapV2Pair::Sync {
            reserve0: U112::from(1),
            reserve1: U112::from(1),
        };
        assert_eq!(
            s.apply_log(&rpc_log(Address::with_last_byte(5), ev.encode_log_data())),
            None
        );
        s.insert(pool(
            1,
            PoolKind::UniV2 { fee_bps: 30 },
            A,
            B,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        ));
        let other = alloy::primitives::LogData::new_unchecked(
            vec![B256::with_last_byte(1)],
            Default::default(),
        );
        assert_eq!(
            s.apply_log(&rpc_log(Address::with_last_byte(1), other)),
            None
        );
    }

    #[test]
    fn usd_price_direct_and_via_native() {
        let mut s = PoolStore::default();
        let v2 = PoolKind::UniV2 { fee_bps: 30 };
        // 1 NATIVE(A, 18 dec raw) = 2000 USD raw units (USD has 6 dec): reserves 1e18 : 2000e6.
        s.insert(pool(
            1,
            v2,
            A,
            USD,
            PoolState::V2 {
                reserve0: 1e18,
                reserve1: 2_000e6,
            },
        ));
        // 1 B = 0.5 A: reserves 2e18 B : 1e18 A (token0 = A, token1 = B).
        s.insert(pool(
            2,
            v2,
            A,
            B,
            PoolState::V2 {
                reserve0: 1e18,
                reserve1: 2e18,
            },
        ));
        let usd_per_a = s.usd_per_raw_unit(A, USD, 6, A).unwrap();
        assert!((usd_per_a * 1e18 - 2_000.0).abs() < 1e-6);
        let usd_per_b = s.usd_per_raw_unit(B, USD, 6, A).unwrap();
        assert!((usd_per_b * 1e18 - 1_000.0).abs() < 1e-6);
        assert!((s.usd_per_raw_unit(USD, USD, 6, A).unwrap() - 1e-6).abs() < 1e-18);
    }

    fn v4_pool(flipped: bool) -> Pool {
        let id = B256::repeat_byte(0x44);
        pool_with_addr(
            mev_types::v4_pool_address(id),
            PoolKind::UniV4 {
                fee: 500,
                tick_spacing: 10,
                native: false,
                flipped,
                id,
                manager: Address::with_last_byte(0x77),
            },
            A,
            B,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1.0,
            },
        )
    }

    fn pool_with_addr(
        addr: Address,
        kind: PoolKind,
        t0: Address,
        t1: Address,
        state: PoolState,
    ) -> Pool {
        Pool::new(
            PoolInfo {
                address: addr,
                dex: "uniswap_v4".into(),
                kind,
                token0: t0,
                token1: t1,
            },
            state,
        )
    }

    fn v4_swap_log(id: B256, manager: Address, amount0: i128, fee: u32) -> Log {
        use alloy::sol_types::SolEvent;
        let ev = IPoolManager::Swap {
            id,
            sender: Address::with_last_byte(1),
            amount0,
            amount1: -amount0,
            sqrtPriceX96: U160::from(2u128 << 96), // sqrt price 2
            liquidity: 5_000,
            tick: I24::unchecked_from(0),
            fee: alloy::primitives::aliases::U24::from(fee),
        };
        rpc_log(manager, ev.encode_log_data())
    }

    #[test]
    fn a_v4_swap_event_updates_the_pool_it_names_and_the_fee_of_its_direction() {
        let mut s = PoolStore::default();
        let p = v4_pool(false);
        let (addr, id) = (p.info.address, B256::repeat_byte(0x44));
        s.insert(p);
        // currency0 paid (amount0 < 0): a zeroForOne swap that paid a 0.0625 % total fee.
        let changed = s.apply_log(&v4_swap_log(id, Address::with_last_byte(0x77), -100, 625));
        assert_eq!(changed, Some(addr));
        let p = s.get(&addr).unwrap();
        assert_eq!(
            p.state,
            PoolState::V3 {
                sqrt_price: 2.0,
                liquidity: 5_000.0
            }
        );
        assert!(
            (p.fee0 - 0.000625).abs() < 1e-12,
            "zeroForOne fee is token0's"
        );
        assert!(
            (p.fee1 - 0.0005).abs() < 1e-12,
            "the other direction is untouched"
        );
        // oneForZero updates the other fee.
        s.apply_log(&v4_swap_log(id, Address::with_last_byte(0x77), 100, 700));
        assert!((s.get(&addr).unwrap().fee1 - 0.0007).abs() < 1e-12);
    }

    #[test]
    fn a_flipped_v4_pool_stores_the_mirrored_price_and_fees() {
        let mut s = PoolStore::default();
        let p = v4_pool(true);
        let (addr, id) = (p.info.address, B256::repeat_byte(0x44));
        s.insert(p);
        s.apply_log(&v4_swap_log(id, Address::with_last_byte(0x77), -100, 625));
        let p = s.get(&addr).unwrap();
        assert_eq!(
            p.state,
            PoolState::V3 {
                sqrt_price: 0.5,
                liquidity: 5_000.0
            }
        );
        // A zeroForOne swap on the pool's currencies pays with our token1 when flipped.
        assert!((p.fee1 - 0.000625).abs() < 1e-12);
    }

    #[test]
    fn v4_events_for_other_pools_or_other_emitters_are_ignored() {
        let mut s = PoolStore::default();
        s.insert(v4_pool(false));
        let id = B256::repeat_byte(0x44);
        assert_eq!(
            s.apply_log(&v4_swap_log(
                B256::repeat_byte(1),
                Address::with_last_byte(0x77),
                -1,
                500
            )),
            None
        );
        assert_eq!(
            s.apply_log(&v4_swap_log(id, Address::with_last_byte(0x88), -1, 500)),
            None,
            "not the manager"
        );
    }

    #[test]
    fn v4_pools_are_watched_through_their_manager() {
        let mut s = PoolStore::default();
        s.insert(v4_pool(false));
        s.insert(pool(
            3,
            PoolKind::UniV3 { fee: 500 },
            A,
            B,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1.0,
            },
        ));
        let logs = s.log_addresses();
        assert_eq!(logs.len(), 2, "one V3 pool plus the single manager");
        assert!(logs.contains(&Address::with_last_byte(0x77)));
        assert!(!logs.contains(&mev_types::v4_pool_address(B256::repeat_byte(0x44))));
    }

    fn v4_liquidity_log(id: B256, lower: i32, upper: i32, delta: i64) -> Log {
        use alloy::sol_types::SolEvent;
        let ev = IPoolManager::ModifyLiquidity {
            id,
            sender: Address::with_last_byte(1),
            tickLower: I24::try_from(lower).unwrap(),
            tickUpper: I24::try_from(upper).unwrap(),
            liquidityDelta: I256::try_from(delta).unwrap(),
            salt: B256::ZERO,
        };
        rpc_log(Address::with_last_byte(0x77), ev.encode_log_data())
    }

    #[test]
    fn liquidity_changes_apply_only_when_the_current_tick_is_inside_the_range() {
        let mut s = PoolStore::default();
        let p = v4_pool(false); // sqrt price 1.0 = tick 0, liquidity 1.0
        let (addr, id) = (p.info.address, B256::repeat_byte(0x44));
        s.insert(p);
        // Range straddling tick 0: depth grows.
        assert_eq!(
            s.apply_log(&v4_liquidity_log(id, -10, 10, 1_000)),
            Some(addr)
        );
        let PoolState::V3 { liquidity, .. } = s.get(&addr).unwrap().state else {
            panic!()
        };
        assert_eq!(liquidity, 1_001.0);
        // A range above the current tick does not touch the active depth.
        assert_eq!(s.apply_log(&v4_liquidity_log(id, 10, 20, 5_000)), None);
        // Removal (negative delta) shrinks it, and never below zero.
        s.apply_log(&v4_liquidity_log(id, 0, 10, -400));
        let PoolState::V3 { liquidity, .. } = s.get(&addr).unwrap().state else {
            panic!()
        };
        assert_eq!(liquidity, 601.0);
        s.apply_log(&v4_liquidity_log(id, -5, 5, -1_000_000));
        let PoolState::V3 { liquidity, .. } = s.get(&addr).unwrap().state else {
            panic!()
        };
        assert_eq!(liquidity, 0.0);
        // Unknown pool or foreign emitter: ignored.
        assert_eq!(
            s.apply_log(&v4_liquidity_log(B256::repeat_byte(1), -10, 10, 5)),
            None
        );
    }

    #[test]
    fn the_range_test_uses_the_pools_own_orientation_when_flipped() {
        let mut s = PoolStore::default();
        // Flipped pool at stored sqrt price 1/1.0002 (the pool's own price is 1.0002, tick ~ 2):
        // a range [5, 10) is above its tick, [0, 5) contains it.
        let mut p = v4_pool(true);
        p.state = PoolState::V3 {
            sqrt_price: 1.0 / 1.0002,
            liquidity: 100.0,
        };
        let (addr, id) = (p.info.address, B256::repeat_byte(0x44));
        s.insert(p);
        assert_eq!(s.apply_log(&v4_liquidity_log(id, 5, 10, 50)), None);
        assert_eq!(s.apply_log(&v4_liquidity_log(id, 0, 5, 50)), Some(addr));
        let PoolState::V3 { liquidity, .. } = s.get(&addr).unwrap().state else {
            panic!()
        };
        assert_eq!(liquidity, 150.0);
    }

    #[test]
    fn a_pool_version_moves_only_when_its_state_changes() {
        let mut s = PoolStore::default();
        let addr = Address::with_last_byte(1);
        s.insert(pool(
            1,
            PoolKind::UniV2 { fee_bps: 30 },
            A,
            B,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        ));
        let v0 = s.version(&addr);
        let sync = |r0: u64, r1: u64| {
            rpc_log(
                addr,
                IUniswapV2Pair::Sync {
                    reserve0: U112::from(r0),
                    reserve1: U112::from(r1),
                }
                .encode_log_data(),
            )
        };
        s.apply_log(&sync(500, 700));
        let v1 = s.version(&addr);
        assert!(v1 > v0, "a changed state bumps the version");
        s.apply_log(&sync(500, 700));
        assert_eq!(s.version(&addr), v1, "repeating the same state is a no-op");
        assert_eq!(
            s.version(&Address::with_last_byte(9)),
            0,
            "unknown pools are version 0"
        );
    }

    #[test]
    fn a_background_read_never_overwrites_a_pool_an_event_updated_since() {
        let mut s = PoolStore::default();
        let (a1, a2) = (Address::with_last_byte(1), Address::with_last_byte(2));
        for n in [1u8, 2] {
            s.insert(pool(
                n,
                PoolKind::UniV2 { fee_bps: 30 },
                A,
                B,
                PoolState::V2 {
                    reserve0: 1.0,
                    reserve1: 1.0,
                },
            ));
        }
        let read_at = s.versions_snapshot();
        // The read began; then an event moved pool 1.
        s.apply_log(&rpc_log(
            a1,
            IUniswapV2Pair::Sync {
                reserve0: U112::from(40),
                reserve1: U112::from(60),
            }
            .encode_log_data(),
        ));
        let fresh = |n: u8, r0: f64| {
            pool(
                n,
                PoolKind::UniV2 { fee_bps: 30 },
                A,
                B,
                PoolState::V2 {
                    reserve0: r0,
                    reserve1: 9.0,
                },
            )
        };
        let out = s.apply_refresh(vec![fresh(1, 5.0), fresh(2, 7.0)], &read_at);
        assert_eq!((out.applied, out.skipped), (1, 1));
        assert_eq!(
            s.get(&a1).unwrap().state,
            PoolState::V2 {
                reserve0: 40.0,
                reserve1: 60.0
            },
            "the newer event wins"
        );
        assert_eq!(
            s.get(&a2).unwrap().state,
            PoolState::V2 {
                reserve0: 7.0,
                reserve1: 9.0
            },
            "the untouched pool takes the chain's state"
        );
        assert_eq!(out.overwrites.len(), 1);
        assert_eq!(out.overwrites[0].pool, a2);
        assert_eq!(
            out.overwrites[0].predicted,
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0
            }
        );
    }

    fn v3_liquidity_log(addr: Address, mint: bool, lo: i32, hi: i32, amount: u128) -> Log {
        let (l, h) = (I24::try_from(lo).unwrap(), I24::try_from(hi).unwrap());
        let data = if mint {
            IUniswapV3Pool::Mint {
                sender: Address::with_last_byte(1),
                owner: Address::with_last_byte(2),
                tickLower: l,
                tickUpper: h,
                amount,
                amount0: U256::ZERO,
                amount1: U256::ZERO,
            }
            .encode_log_data()
        } else {
            IUniswapV3Pool::Burn {
                owner: Address::with_last_byte(2),
                tickLower: l,
                tickUpper: h,
                amount,
                amount0: U256::ZERO,
                amount1: U256::ZERO,
            }
            .encode_log_data()
        };
        rpc_log(addr, data)
    }

    #[test]
    fn v3_mint_and_burn_move_active_liquidity_only_when_the_price_is_in_range() {
        let mut s = PoolStore::default();
        let addr = Address::with_last_byte(1);
        // sqrt price 1.0 = tick 0.
        s.insert(pool(
            1,
            PoolKind::UniV3 { fee: 500 },
            A,
            B,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1_000.0,
            },
        ));
        let liq = |s: &PoolStore| match s.get(&addr).unwrap().state {
            PoolState::V3 { liquidity, .. } => liquidity,
            _ => panic!(),
        };
        let v0 = s.version(&addr);
        assert_eq!(
            s.apply_log(&v3_liquidity_log(addr, true, -10, 10, 400)),
            Some(addr)
        );
        assert_eq!(liq(&s), 1_400.0);
        assert!(s.version(&addr) > v0);
        // Out of range: active liquidity untouched.
        s.apply_log(&v3_liquidity_log(addr, true, 10, 20, 999));
        assert_eq!(liq(&s), 1_400.0);
        // Burn inside the range, never below zero.
        s.apply_log(&v3_liquidity_log(addr, false, -10, 10, 100));
        assert_eq!(liq(&s), 1_300.0);
        s.apply_log(&v3_liquidity_log(addr, false, -10, 10, 1_000_000));
        assert_eq!(liq(&s), 0.0);
        // A zero-amount poke and an unknown pool do nothing.
        assert_eq!(s.apply_log(&v3_liquidity_log(addr, true, -10, 10, 0)), None);
        assert_eq!(
            s.apply_log(&v3_liquidity_log(
                Address::with_last_byte(9),
                true,
                -10,
                10,
                5
            )),
            None
        );
    }

    #[test]
    fn v3_mint_and_burn_keep_the_tick_table_exact() {
        let mut s = PoolStore::default();
        let addr = Address::with_last_byte(1);
        let p = pool(
            1,
            PoolKind::UniV3 { fee: 500 },
            A,
            B,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 100.0,
            },
        )
        .with_ticks(mev_dex::TickMap {
            ticks: vec![(-10, 100.0), (10, -100.0)],
            lo: -50,
            hi: 50,
            hint: None,
        });
        s.insert(p);
        let net = |s: &PoolStore| s.get(&addr).unwrap().ticks.as_ref().unwrap().ticks.clone();
        s.apply_log(&v3_liquidity_log(addr, true, -10, 20, 40));
        // +40 at -10, new entry -40 at 20; the existing upper tick at 10 is unchanged.
        assert_eq!(net(&s), vec![(-10, 140.0), (10, -100.0), (20, -40.0)]);
        s.apply_log(&v3_liquidity_log(addr, false, -10, 20, 40));
        assert_eq!(net(&s), vec![(-10, 100.0), (10, -100.0), (20, 0.0)]);
        // A bound outside the loaded window is ignored.
        s.apply_log(&v3_liquidity_log(addr, true, 40, 400, 5));
        assert_eq!(net(&s).last(), Some(&(40, 5.0)));
        assert!(!net(&s).iter().any(|(t, _)| *t == 400));
    }

    #[test]
    fn rounding_noise_is_not_a_correction_but_a_real_change_is() {
        let a = PoolState::V3 {
            sqrt_price: 0.003_420_211_04,
            liquidity: 1.535e22,
        };
        let noise = PoolState::V3 {
            sqrt_price: 0.003_420_211_04 * (1.0 + 1e-15),
            liquidity: 1.535e22,
        };
        let step = PoolState::V3 {
            sqrt_price: 0.003_420_705_68,
            liquidity: 1.535e22,
        };
        let liq = PoolState::V3 {
            sqrt_price: 0.003_420_211_04,
            liquidity: 1.4e22,
        };
        assert!(!materially_different(&a, &noise));
        assert!(materially_different(&a, &step));
        assert!(materially_different(&a, &liq));
        let v2 = |r0, r1| PoolState::V2 {
            reserve0: r0,
            reserve1: r1,
        };
        assert!(!materially_different(&v2(5.0, 7.0), &v2(5.0, 7.0)));
        assert!(materially_different(&v2(5.0, 7.0), &v2(5.0, 7.5)));
        assert!(
            materially_different(&a, &v2(1.0, 1.0)),
            "different kinds always differ"
        );
    }

    fn sync_log(addr: Address, r0: u64, r1: u64) -> Log {
        rpc_log(
            addr,
            IUniswapV2Pair::Sync {
                reserve0: U112::from(r0),
                reserve1: U112::from(r1),
            }
            .encode_log_data(),
        )
    }

    fn v2_store(addr_byte: u8) -> (PoolStore, Address) {
        let mut s = PoolStore::default();
        s.insert(pool(
            addr_byte,
            PoolKind::UniV2 { fee_bps: 30 },
            A,
            B,
            PoolState::V2 {
                reserve0: 100.0,
                reserve1: 200.0,
            },
        ));
        (s, Address::with_last_byte(addr_byte))
    }

    #[test]
    fn a_tentative_state_is_rolled_back_when_nothing_confirms_it() {
        let (mut s, addr) = v2_store(1);
        let t0 = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(3);
        let v0 = s.version(&addr);
        assert_eq!(
            s.apply_log_tentative(&sync_log(addr, 150, 150), t0),
            Some(addr)
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 150.0,
                reserve1: 150.0
            }
        );
        assert_eq!(s.tentative_len(), 1);
        // Too early: nothing happens.
        assert!(
            s.expire_tentative(t0 + std::time::Duration::from_secs(2), ttl, true)
                .is_empty()
        );
        // Past the limit: the confirmed state comes back and the version moves.
        let rb = s.expire_tentative(t0 + ttl, ttl, true);
        assert_eq!(rb.len(), 1);
        assert_eq!(rb[0].pool, addr);
        assert_eq!(
            rb[0].tentative,
            PoolState::V2 {
                reserve0: 150.0,
                reserve1: 150.0
            }
        );
        assert_eq!(
            rb[0].restored,
            PoolState::V2 {
                reserve0: 100.0,
                reserve1: 200.0
            }
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 100.0,
                reserve1: 200.0
            }
        );
        assert!(s.version(&addr) > v0);
        assert_eq!(s.tentative_len(), 0);
    }

    #[test]
    fn a_real_log_confirms_a_tentative_state_so_it_is_never_rolled_back() {
        let (mut s, addr) = v2_store(1);
        let t0 = std::time::Instant::now();
        s.apply_log_tentative(&sync_log(addr, 150, 150), t0);
        s.apply_log(&sync_log(addr, 150, 150)); // the block's own log for the same transaction
        assert_eq!(s.tentative_len(), 0);
        assert!(
            s.expire_tentative(
                t0 + std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(3),
                true,
            )
            .is_empty()
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 150.0,
                reserve1: 150.0
            }
        );
    }

    #[test]
    fn a_resync_or_replacement_confirms_too_and_two_pending_txs_keep_the_first_confirmed_state() {
        let (mut s, addr) = v2_store(1);
        let t0 = std::time::Instant::now();
        s.apply_log_tentative(&sync_log(addr, 150, 150), t0);
        s.apply_log_tentative(
            &sync_log(addr, 170, 130),
            t0 + std::time::Duration::from_secs(1),
        );
        // The second application extends the clock but keeps the original confirmed state.
        assert!(
            s.expire_tentative(
                t0 + std::time::Duration::from_secs(3),
                std::time::Duration::from_secs(3),
                true,
            )
            .is_empty()
        );
        let rb = s.expire_tentative(
            t0 + std::time::Duration::from_secs(4),
            std::time::Duration::from_secs(3),
            true,
        );
        assert_eq!(
            rb[0].restored,
            PoolState::V2 {
                reserve0: 100.0,
                reserve1: 200.0
            }
        );
        // A replacement (what a resync does) clears it.
        let (mut s, addr) = v2_store(2);
        s.apply_log_tentative(&sync_log(addr, 150, 150), t0);
        s.replace(pool(
            2,
            PoolKind::UniV2 { fee_bps: 30 },
            A,
            B,
            PoolState::V2 {
                reserve0: 120.0,
                reserve1: 180.0,
            },
        ));
        assert_eq!(s.tentative_len(), 0);
        assert!(
            s.expire_tentative(
                t0 + std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(3),
                true,
            )
            .is_empty()
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 120.0,
                reserve1: 180.0
            }
        );
    }

    #[test]
    fn events_for_unknown_pools_leave_no_tentative_state() {
        let (mut s, _) = v2_store(1);
        let stranger = Address::with_last_byte(9);
        assert_eq!(
            s.apply_log_tentative(&sync_log(stranger, 1, 1), std::time::Instant::now()),
            None
        );
        assert_eq!(s.tentative_len(), 0);
    }

    #[test]
    fn observe_only_reports_the_miss_without_touching_the_state() {
        let (mut s, addr) = v2_store(1);
        let t0 = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(3);
        s.apply_log_tentative(&sync_log(addr, 150, 150), t0);
        let v = s.version(&addr);
        let rb = s.expire_tentative(t0 + ttl, ttl, false);
        assert_eq!(rb.len(), 1);
        assert_eq!(
            rb[0].tentative,
            PoolState::V2 {
                reserve0: 150.0,
                reserve1: 150.0
            }
        );
        assert_eq!(
            rb[0].restored,
            PoolState::V2 {
                reserve0: 100.0,
                reserve1: 200.0
            }
        );
        assert_eq!(
            s.get(&addr).unwrap().state,
            PoolState::V2 {
                reserve0: 150.0,
                reserve1: 150.0
            },
            "left as it was"
        );
        assert_eq!(s.version(&addr), v, "and not versioned as a change");
        assert_eq!(s.tentative_len(), 0, "the mark is dropped either way");
    }

    #[test]
    fn journal_only_events_leave_pool_state_and_versions_alone() {
        use alloy::primitives::{B256, Bytes, LogData};
        // Events the journal records (V2 swaps, V3 fee collection and flash loans, V4 pool creation) must not move a pool.
        let (mut s, addr) = v2_store(1);
        let v0 = s.version(&addr);
        let before = s.get(&addr).unwrap().state.clone();
        for topic in [
            mev_decode::events::kinds()
                .iter()
                .find(|k| k.name == "swap_v2")
                .unwrap()
                .topic0,
            mev_decode::events::kinds()
                .iter()
                .find(|k| k.name == "collect_v3")
                .unwrap()
                .topic0,
            mev_decode::events::kinds()
                .iter()
                .find(|k| k.name == "flash_v3")
                .unwrap()
                .topic0,
            mev_decode::events::kinds()
                .iter()
                .find(|k| k.name == "initialize_v4")
                .unwrap()
                .topic0,
            B256::repeat_byte(0x77),
        ] {
            let l = rpc_log(
                addr,
                LogData::new_unchecked(
                    vec![topic, B256::ZERO, B256::ZERO],
                    Bytes::from(vec![1u8; 160]),
                ),
            );
            assert_eq!(s.apply_log(&l), None, "{topic:#x}");
        }
        assert_eq!(s.get(&addr).unwrap().state, before);
        assert_eq!(
            s.version(&addr),
            v0,
            "and the opportunity versions do not move"
        );
    }
}
