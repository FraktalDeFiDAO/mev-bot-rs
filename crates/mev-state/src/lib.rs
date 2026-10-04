//! In-memory pool store: indexes pools by token pair and applies on-chain events. No I/O.

use std::collections::HashMap;

use alloy::{rpc::types::Log, sol_types::SolEvent};
use mev_bindings::{IPancakeV3Pool, IUniswapV2Pair, IUniswapV3Pool};
use mev_dex::{Pool, PoolState, sqrt_price_from_x96};
use mev_types::{Address, B256};

#[derive(Default)]
pub struct PoolStore {
    pools: HashMap<Address, Pool>,
    by_pair: HashMap<(Address, Address), Vec<Address>>,
}

fn pair_key(a: Address, b: Address) -> (Address, Address) {
    if a < b { (a, b) } else { (b, a) }
}

impl PoolStore {
    pub fn insert(&mut self, pool: Pool) {
        let key = pair_key(pool.info.token0, pool.info.token1);
        let addr = pool.info.address;
        if self.pools.insert(addr, pool).is_none() {
            self.by_pair.entry(key).or_default().push(addr);
        }
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
        if let Some(p) = self.pools.get_mut(&pool.info.address) {
            *p = pool;
        }
    }

    pub fn set_state(&mut self, addr: &Address, state: PoolState) {
        if let Some(p) = self.pools.get_mut(addr) {
            p.state = state;
        }
    }

    /// Apply a V2 `Sync` or V3 `Swap` log. Returns the pool address if state changed.
    pub fn apply_log(&mut self, log: &Log) -> Option<Address> {
        let addr = log.address();
        let topic0: B256 = *log.topic0()?;
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
}
