//! In-memory pool store: indexes pools by token pair and applies on-chain events. No I/O.

use std::collections::HashMap;

use alloy::{rpc::types::Log, sol_types::SolEvent};
use mev_bindings::{IPancakeV3Pool, IPoolManager, IUniswapV2Pair, IUniswapV3Pool};
use mev_dex::{Pool, PoolState, sqrt_price_from_x96};
use mev_types::{Address, B256, PoolKind};

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
            if old.state != p.state {
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
        if let Some(p) = self.pools.get_mut(addr) {
            let changed = p.state != state;
            p.state = state;
            if changed {
                self.bump(*addr);
            }
        }
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

    /// Apply a pool event. Returns the pool it updated; the pool's version moves only when its
    /// state actually changed (a real block log that repeats a pending tx's state is a no-op).
    pub fn apply_log(&mut self, log: &Log) -> Option<Address> {
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
    use alloy::primitives::{Address, I256, U160, aliases::I24, aliases::U112};
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
}
