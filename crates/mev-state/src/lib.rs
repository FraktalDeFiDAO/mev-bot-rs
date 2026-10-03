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
