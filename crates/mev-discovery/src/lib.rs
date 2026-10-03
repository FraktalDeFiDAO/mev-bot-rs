//! Pool discovery and state loading via DEX factories + Multicall3.
//!
//! Pools are only ever taken from the configured factories, so the executor never routes through
//! an arbitrary (possibly malicious) contract.

use std::collections::HashMap;

use alloy::primitives::{Address, U256};
use anyhow::Result;
use mev_bindings::{
    ICamelotPair, IERC20, IUniswapV2Factory, IUniswapV2Pair, IUniswapV3Factory, IUniswapV3Pool,
};
use mev_config::{ChainConfig, DexType};
use mev_dex::{Pool, PoolState, sqrt_price_from_x96};
use mev_rpc::{Rpc, call, decode};
use mev_state::PoolStore;
use mev_types::{PoolInfo, PoolKind};
use tracing::info;

/// Camelot fee percent denominator.
const CAMELOT_FEE_DENOM: f64 = 100_000.0;

/// Find every pool for the configured pairs on the configured DEXes.
pub async fn discover_pools(rpc: &Rpc, cfg: &ChainConfig) -> Result<Vec<PoolInfo>> {
    let mut calls = Vec::new();
    let mut meta: Vec<(String, PoolKind, Address, Address)> = Vec::new();
    for [sa, sb] in &cfg.pairs {
        let (a, b) = (cfg.token(sa)?, cfg.token(sb)?);
        let (t0, t1) = if a < b { (a, b) } else { (b, a) };
        for dex in &cfg.dexes {
            match dex.kind {
                DexType::Univ2 | DexType::Camelot => {
                    calls.push(call(
                        dex.factory,
                        IUniswapV2Factory::getPairCall {
                            tokenA: a,
                            tokenB: b,
                        },
                    ));
                    let kind = match dex.kind {
                        DexType::Camelot => PoolKind::CamelotV2,
                        _ => PoolKind::UniV2 {
                            fee_bps: dex.fee_bps,
                        },
                    };
                    meta.push((dex.name.clone(), kind, t0, t1));
                }
                DexType::Univ3 => {
                    for &fee in &dex.fees {
                        let fee_u24 = alloy::primitives::aliases::U24::from(fee);
                        calls.push(call(
                            dex.factory,
                            IUniswapV3Factory::getPoolCall {
                                tokenA: a,
                                tokenB: b,
                                fee: fee_u24,
                            },
                        ));
                        meta.push((dex.name.clone(), PoolKind::UniV3 { fee }, t0, t1));
                    }
                }
            }
        }
    }
    let results = rpc.multicall(&calls).await?;
    let mut pools = Vec::new();
    for (res, (dex, kind, token0, token1)) in results.iter().zip(meta) {
        let addr = match kind {
            PoolKind::UniV3 { .. } => decode::<IUniswapV3Factory::getPoolCall>(res),
            _ => decode::<IUniswapV2Factory::getPairCall>(res),
        };
        if let Some(address) = addr.filter(|a| !a.is_zero()) {
            pools.push(PoolInfo {
                address,
                dex,
                kind,
                token0,
                token1,
            });
        }
    }
    info!(
        candidates = calls.len(),
        found = pools.len(),
        "pool discovery"
    );
    Ok(pools)
}

/// Read current state for pools. Camelot stable pairs (non constant-product) are dropped.
pub async fn load_pools(rpc: &Rpc, infos: Vec<PoolInfo>) -> Result<Vec<Pool>> {
    let mut calls = Vec::new();
    for p in &infos {
        match p.kind {
            PoolKind::UniV3 { .. } => {
                calls.push(call(p.address, IUniswapV3Pool::slot0Call {}));
                calls.push(call(p.address, IUniswapV3Pool::liquidityCall {}));
            }
            PoolKind::UniV2 { .. } => {
                calls.push(call(p.address, IUniswapV2Pair::getReservesCall {}));
                calls.push(call(p.address, IUniswapV2Pair::getReservesCall {})); // keep 2 per pool
            }
            PoolKind::CamelotV2 => {
                calls.push(call(p.address, IUniswapV2Pair::getReservesCall {}));
                calls.push(call(p.address, ICamelotPair::stableSwapCall {}));
                calls.push(call(p.address, ICamelotPair::token0FeePercentCall {}));
                calls.push(call(p.address, ICamelotPair::token1FeePercentCall {}));
            }
        }
    }
    let res = rpc.multicall(&calls).await?;
    let mut i = 0;
    let mut pools = Vec::new();
    for info in infos {
        match info.kind {
            PoolKind::UniV3 { .. } => {
                let slot0 = decode::<IUniswapV3Pool::slot0Call>(&res[i]);
                let liq = decode::<IUniswapV3Pool::liquidityCall>(&res[i + 1]);
                i += 2;
                if let (Some(s), Some(l)) = (slot0, liq) {
                    let state = PoolState::V3 {
                        sqrt_price: sqrt_price_from_x96(f64::from(s.sqrtPriceX96)),
                        liquidity: l as f64,
                    };
                    pools.push(Pool::new(info, state));
                }
            }
            PoolKind::UniV2 { .. } => {
                let r = decode::<IUniswapV2Pair::getReservesCall>(&res[i]);
                i += 2;
                if let Some(r) = r {
                    let state = PoolState::V2 {
                        reserve0: f64::from(r.reserve0),
                        reserve1: f64::from(r.reserve1),
                    };
                    pools.push(Pool::new(info, state));
                }
            }
            PoolKind::CamelotV2 => {
                let r = decode::<IUniswapV2Pair::getReservesCall>(&res[i]);
                let stable = decode::<ICamelotPair::stableSwapCall>(&res[i + 1]);
                let f0 = decode::<ICamelotPair::token0FeePercentCall>(&res[i + 2]);
                let f1 = decode::<ICamelotPair::token1FeePercentCall>(&res[i + 3]);
                i += 4;
                if let (Some(r), Some(false), Some(f0), Some(f1)) = (r, stable, f0, f1) {
                    let state = PoolState::V2 {
                        reserve0: f64::from(r.reserve0),
                        reserve1: f64::from(r.reserve1),
                    };
                    pools.push(Pool::new(info, state).with_directional_fees(
                        f0 as f64 / CAMELOT_FEE_DENOM,
                        f1 as f64 / CAMELOT_FEE_DENOM,
                    ));
                }
            }
        }
    }
    Ok(pools)
}

/// Re-read state for every pool in the store (periodic resync for missed events / V3 liquidity changes).
pub async fn refresh(rpc: &Rpc, store: &mut PoolStore) -> Result<()> {
    let infos: Vec<PoolInfo> = store.pools().map(|p| p.info.clone()).collect();
    for p in load_pools(rpc, infos).await? {
        store.set_state(&p.info.address, p.state);
    }
    Ok(())
}

pub async fn token_decimals(rpc: &Rpc, tokens: &[Address]) -> Result<HashMap<Address, u8>> {
    let calls: Vec<_> = tokens
        .iter()
        .map(|t| call(*t, IERC20::decimalsCall {}))
        .collect();
    let res = rpc.multicall(&calls).await?;
    Ok(tokens
        .iter()
        .zip(res.iter())
        .filter_map(|(t, r)| decode::<IERC20::decimalsCall>(r).map(|d| (*t, d)))
        .collect())
}

pub async fn balances(
    rpc: &Rpc,
    holder: Address,
    tokens: &[Address],
) -> Result<HashMap<Address, U256>> {
    let calls: Vec<_> = tokens
        .iter()
        .map(|t| call(*t, IERC20::balanceOfCall { owner: holder }))
        .collect();
    let res = rpc.multicall(&calls).await?;
    Ok(tokens
        .iter()
        .zip(res.iter())
        .filter_map(|(t, r)| decode::<IERC20::balanceOfCall>(r).map(|b| (*t, b)))
        .collect())
}

/// Build a store from pools, dropping pools whose liquidity is below `min_usd` on either side.
pub fn build_store(
    pools: Vec<Pool>,
    min_usd: f64,
    usd_token: Address,
    usd_decimals: u8,
    native: Address,
) -> PoolStore {
    let mut all = PoolStore::default();
    for p in pools.iter().cloned() {
        all.insert(p);
    }
    let mut kept = PoolStore::default();
    for p in pools {
        let side = |t: Address| {
            all.usd_per_raw_unit(t, usd_token, usd_decimals, native)
                .map(|px| px * p.reserve_of(t))
        };
        let liq = match (side(p.info.token0), side(p.info.token1)) {
            (Some(a), Some(b)) => a.min(b),
            _ => 0.0,
        };
        if liq >= min_usd {
            kept.insert(p);
        }
    }
    info!(
        kept = kept.len(),
        dropped = all.len() - kept.len(),
        min_usd,
        "liquidity filter"
    );
    kept
}
