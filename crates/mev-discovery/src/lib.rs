//! Pool discovery and state loading via DEX factories + Multicall3.
//!
//! Pools are only ever taken from the configured factories, so the executor never routes through
//! an arbitrary (possibly malicious) contract. Auto-discovery only uses on-chain activity to pick
//! *which tokens* to look up; the pools themselves still come from the factories.

use std::collections::{HashMap, HashSet};

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::{
    IAlgebraFactory, IAlgebraPool, ICamelotPair, IERC20, IPancakeV3Pool, IPoolMeta,
    IUniswapV2Factory, IUniswapV2Pair, IUniswapV3Factory, IUniswapV3Pool,
};
use mev_config::{AutoDiscovery, ChainConfig, DexType};
use mev_dex::{Pool, PoolState, sqrt_price_from_x96};
use mev_rpc::{Rpc, call, decode};
use mev_state::PoolStore;
use mev_types::{PoolInfo, PoolKind};
use tracing::{info, warn};

/// Camelot V2 fee percent denominator.
const CAMELOT_FEE_DENOM: f64 = 100_000.0;
/// Algebra / V3 fee denominator (hundredths of a bip).
const V3_FEE_DENOM: f64 = 1_000_000.0;

/// Find every pool for `pairs` on the configured DEXes.
pub async fn discover_pools(
    rpc: &Rpc,
    cfg: &ChainConfig,
    pairs: &[(Address, Address)],
) -> Result<Vec<PoolInfo>> {
    let mut calls = Vec::new();
    let mut meta: Vec<(String, PoolKind, Address, Address)> = Vec::new();
    for &(a, b) in pairs {
        if a == b {
            continue;
        }
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
                DexType::Univ3 | DexType::Pancakev3 => {
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
                        let kind = match dex.kind {
                            DexType::Pancakev3 => PoolKind::PancakeV3 { fee },
                            _ => PoolKind::UniV3 { fee },
                        };
                        meta.push((dex.name.clone(), kind, t0, t1));
                    }
                }
                DexType::Algebra => {
                    calls.push(call(
                        dex.factory,
                        IAlgebraFactory::poolByPairCall {
                            tokenA: a,
                            tokenB: b,
                        },
                    ));
                    meta.push((dex.name.clone(), PoolKind::AlgebraV3, t0, t1));
                }
            }
        }
    }
    let results = rpc.multicall(&calls).await?;
    let mut seen = HashSet::new();
    let mut pools = Vec::new();
    for (res, (dex, kind, token0, token1)) in results.iter().zip(meta) {
        let addr = match kind {
            PoolKind::UniV3 { .. } | PoolKind::PancakeV3 { .. } => {
                decode::<IUniswapV3Factory::getPoolCall>(res)
            }
            PoolKind::AlgebraV3 => decode::<IAlgebraFactory::poolByPairCall>(res),
            _ => decode::<IUniswapV2Factory::getPairCall>(res),
        };
        if let Some(address) = addr.filter(|a| !a.is_zero())
            && seen.insert(address)
        {
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
        pairs = pairs.len(),
        candidates = calls.len(),
        found = pools.len(),
        "pool discovery"
    );
    Ok(pools)
}

/// Calls per pool kind for `load_pools` (fixed so results can be sliced positionally).
fn calls_for(p: &PoolInfo) -> Vec<(Address, alloy::primitives::Bytes)> {
    match p.kind {
        PoolKind::UniV3 { .. } => vec![
            call(p.address, IUniswapV3Pool::slot0Call {}),
            call(p.address, IUniswapV3Pool::liquidityCall {}),
        ],
        PoolKind::PancakeV3 { .. } => vec![
            call(p.address, IPancakeV3Pool::slot0Call {}),
            call(p.address, IUniswapV3Pool::liquidityCall {}),
        ],
        PoolKind::AlgebraV3 => vec![
            call(p.address, IAlgebraPool::globalStateCall {}),
            call(p.address, IUniswapV3Pool::liquidityCall {}),
        ],
        PoolKind::UniV2 { .. } => vec![call(p.address, IUniswapV2Pair::getReservesCall {})],
        PoolKind::CamelotV2 => vec![
            call(p.address, IUniswapV2Pair::getReservesCall {}),
            call(p.address, ICamelotPair::stableSwapCall {}),
            call(p.address, ICamelotPair::token0FeePercentCall {}),
            call(p.address, ICamelotPair::token1FeePercentCall {}),
        ],
    }
}

/// Read current state for pools. Camelot stable pairs (non constant-product) are dropped.
pub async fn load_pools(rpc: &Rpc, infos: Vec<PoolInfo>) -> Result<Vec<Pool>> {
    let per_pool: Vec<_> = infos.iter().map(calls_for).collect();
    let flat: Vec<_> = per_pool.iter().flatten().cloned().collect();
    let res = rpc.multicall(&flat).await?;
    let mut i = 0;
    let mut pools = Vec::new();
    for (info, calls) in infos.into_iter().zip(per_pool) {
        let r = &res[i..i + calls.len()];
        i += calls.len();
        let v3 = |sqrt_x96: alloy::primitives::U160, liq: u128| PoolState::V3 {
            sqrt_price: sqrt_price_from_x96(f64::from(sqrt_x96)),
            liquidity: liq as f64,
        };
        let pool = match info.kind {
            PoolKind::UniV3 { .. } => {
                match (
                    decode::<IUniswapV3Pool::slot0Call>(&r[0]),
                    decode::<IUniswapV3Pool::liquidityCall>(&r[1]),
                ) {
                    (Some(s), Some(l)) => Some(Pool::new(info, v3(s.sqrtPriceX96, l))),
                    _ => None,
                }
            }
            PoolKind::PancakeV3 { .. } => {
                match (
                    decode::<IPancakeV3Pool::slot0Call>(&r[0]),
                    decode::<IUniswapV3Pool::liquidityCall>(&r[1]),
                ) {
                    (Some(s), Some(l)) => Some(Pool::new(info, v3(s.sqrtPriceX96, l))),
                    _ => None,
                }
            }
            PoolKind::AlgebraV3 => {
                match (
                    decode::<IAlgebraPool::globalStateCall>(&r[0]),
                    decode::<IUniswapV3Pool::liquidityCall>(&r[1]),
                ) {
                    (Some(g), Some(l)) => {
                        Some(Pool::new(info, v3(g.price, l)).with_directional_fees(
                            g.feeZto as f64 / V3_FEE_DENOM,
                            g.feeOtz as f64 / V3_FEE_DENOM,
                        ))
                    }
                    _ => None,
                }
            }
            PoolKind::UniV2 { .. } => decode::<IUniswapV2Pair::getReservesCall>(&r[0]).map(|x| {
                Pool::new(
                    info,
                    PoolState::V2 {
                        reserve0: f64::from(x.reserve0),
                        reserve1: f64::from(x.reserve1),
                    },
                )
            }),
            PoolKind::CamelotV2 => {
                let reserves = decode::<IUniswapV2Pair::getReservesCall>(&r[0]);
                let stable = decode::<ICamelotPair::stableSwapCall>(&r[1]);
                let f0 = decode::<ICamelotPair::token0FeePercentCall>(&r[2]);
                let f1 = decode::<ICamelotPair::token1FeePercentCall>(&r[3]);
                match (reserves, stable, f0, f1) {
                    (Some(x), Some(false), Some(f0), Some(f1)) => Some(
                        Pool::new(
                            info,
                            PoolState::V2 {
                                reserve0: f64::from(x.reserve0),
                                reserve1: f64::from(x.reserve1),
                            },
                        )
                        .with_directional_fees(
                            f0 as f64 / CAMELOT_FEE_DENOM,
                            f1 as f64 / CAMELOT_FEE_DENOM,
                        ),
                    ),
                    _ => None,
                }
            }
        };
        if let Some(p) = pool {
            pools.push(p);
        }
    }
    Ok(pools)
}

/// Re-read state (and Algebra/Camelot fees) for every pool in the store.
pub async fn refresh(rpc: &Rpc, store: &mut PoolStore) -> Result<()> {
    let infos: Vec<PoolInfo> = store.pools().map(|p| p.info.clone()).collect();
    for p in load_pools(rpc, infos).await? {
        store.replace(p);
    }
    Ok(())
}

/// Tokens traded recently in pools created by our configured factories, most active first.
///
/// Scans Swap/Sync logs chain-wide on `ad.log_rpc`, then verifies each emitting pool's factory
/// on-chain. Returned tokens are meant to be paired with `ad.pair_with` and resolved through the
/// factories by `discover_pools`.
pub async fn active_tokens(
    rpc: &Rpc,
    cfg: &ChainConfig,
    ad: &AutoDiscovery,
) -> Result<Vec<Address>> {
    let logs_rpc: DynProvider = ProviderBuilder::new()
        .connect_http(
            ad.log_rpc
                .parse()
                .context("invalid auto_discovery.log_rpc")?,
        )
        .erased();
    let head = logs_rpc.get_block_number().await?;
    let from = head.saturating_sub(ad.blocks);
    let topics = vec![
        IUniswapV2Pair::Sync::SIGNATURE_HASH,
        IUniswapV3Pool::Swap::SIGNATURE_HASH,
        IPancakeV3Pool::Swap::SIGNATURE_HASH,
    ];
    let mut activity: HashMap<Address, u64> = HashMap::new();
    let mut start = from;
    while start <= head {
        let end = (start + ad.step - 1).min(head);
        let filter = Filter::new()
            .event_signature(topics.clone())
            .from_block(start)
            .to_block(end);
        match logs_rpc.get_logs(&filter).await {
            Ok(logs) => {
                for l in logs {
                    *activity.entry(l.address()).or_default() += 1;
                }
            }
            Err(e) => warn!(error = %e, start, end, "log scan chunk failed; skipping"),
        }
        start = end + 1;
    }

    // Verify emitters: keep only pools whose factory() is one of ours.
    let factories: HashSet<Address> = cfg.dexes.iter().map(|d| d.factory).collect();
    let emitters: Vec<Address> = activity.keys().copied().collect();
    let mut calls = Vec::new();
    for a in &emitters {
        calls.push(call(*a, IPoolMeta::factoryCall {}));
        calls.push(call(*a, IPoolMeta::token0Call {}));
        calls.push(call(*a, IPoolMeta::token1Call {}));
    }
    let res = rpc.multicall(&calls).await?;
    let mut token_activity: HashMap<Address, u64> = HashMap::new();
    for (i, a) in emitters.iter().enumerate() {
        let (Some(f), Some(t0), Some(t1)) = (
            decode::<IPoolMeta::factoryCall>(&res[3 * i]),
            decode::<IPoolMeta::token0Call>(&res[3 * i + 1]),
            decode::<IPoolMeta::token1Call>(&res[3 * i + 2]),
        ) else {
            continue;
        };
        if !factories.contains(&f) {
            continue;
        }
        let n = activity[a];
        *token_activity.entry(t0).or_default() += n;
        *token_activity.entry(t1).or_default() += n;
    }
    let mut ranked: Vec<(Address, u64)> = token_activity.into_iter().collect();
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    ranked.truncate(ad.max_tokens);
    info!(
        blocks = ad.blocks,
        emitters = emitters.len(),
        tokens = ranked.len(),
        "auto-discovery scan"
    );
    Ok(ranked.into_iter().map(|(t, _)| t).collect())
}

/// Configured pairs plus (active token × `pair_with`) pairs when auto-discovery is enabled.
/// Long-tail X/Y pairs: the `n` most active tokens that aren't base partners, paired with each
/// other (a triangle L -> X -> Y -> L needs the X/Y pool). `tokens` is most active first.
pub fn cross_pairs(tokens: &[Address], bases: &[Address], n: usize) -> Vec<(Address, Address)> {
    let top: Vec<Address> = tokens
        .iter()
        .copied()
        .filter(|t| !bases.contains(t))
        .take(n)
        .collect();
    let mut out = Vec::new();
    for i in 0..top.len() {
        for j in i + 1..top.len() {
            out.push((top[i], top[j]));
        }
    }
    out
}

pub async fn candidate_pairs(rpc: &Rpc, cfg: &ChainConfig) -> Result<Vec<(Address, Address)>> {
    let mut pairs: Vec<(Address, Address)> = cfg.pair_addresses()?;
    if let Some(ad) = cfg.auto_discovery.as_ref().filter(|a| a.enabled) {
        let partners: Vec<Address> = ad
            .pair_with
            .iter()
            .map(|s| cfg.token(s))
            .collect::<Result<_>>()?;
        match active_tokens(rpc, cfg, ad).await {
            Ok(tokens) => {
                pairs.extend(cross_pairs(&tokens, &partners, ad.cross_pairs));
                for t in tokens {
                    for &p in &partners {
                        pairs.push((t, p));
                    }
                }
            }
            Err(e) => {
                warn!(error = %format!("{e:#}"), "auto-discovery failed; using configured pairs only")
            }
        }
    }
    let mut seen = HashSet::new();
    pairs.retain(|(a, b)| a != b && seen.insert(if a < b { (*a, *b) } else { (*b, *a) }));
    Ok(pairs)
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

/// Build a store from pools, dropping pools whose liquidity is below `min_usd` on either side,
/// and pools that are the only one for their pair (nothing to arbitrage against).
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
    let mut liquid = Vec::new();
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
            liquid.push(p);
        }
    }
    let mut pair_count: HashMap<(Address, Address), usize> = HashMap::new();
    for p in &liquid {
        *pair_count
            .entry((p.info.token0, p.info.token1))
            .or_default() += 1;
    }
    let mut kept = PoolStore::default();
    for p in liquid {
        if pair_count[&(p.info.token0, p.info.token1)] >= 2 {
            kept.insert(p);
        }
    }
    info!(
        kept = kept.len(),
        dropped = all.len() - kept.len(),
        min_usd,
        "liquidity + pair filter"
    );
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD: Address = Address::with_last_byte(0xc);
    const WETH: Address = Address::with_last_byte(0xe);
    const TOKEN: Address = Address::with_last_byte(0xf);

    fn v2(addr: u8, t0: Address, t1: Address, r0: f64, r1: f64) -> Pool {
        let info = PoolInfo {
            address: Address::with_last_byte(addr),
            dex: "t".into(),
            kind: PoolKind::UniV2 { fee_bps: 30 },
            token0: t0,
            token1: t1,
        };
        Pool::new(
            info,
            PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
        )
    }

    #[test]
    fn build_store_drops_illiquid_and_single_pool_pairs() {
        // WETH/USD at $2,000 (USD 6 dec): two deep pools ($200k/side) -> kept.
        let deep_a = v2(1, USD, WETH, 200_000e6, 100e18);
        let deep_b = v2(2, USD, WETH, 200_000e6, 100e18);
        // A third WETH/USD pool with ~$200/side -> dropped by the $5k floor.
        let shallow = v2(3, USD, WETH, 200e6, 0.1e18);
        // TOKEN/WETH has a single deep pool -> nothing to arbitrage against -> dropped.
        let lonely = v2(4, TOKEN, WETH, 1_000_000e18, 100e18);
        let store = build_store(vec![deep_a, deep_b, shallow, lonely], 5_000.0, USD, 6, WETH);
        let mut kept: Vec<u8> = store.addresses().iter().map(|a| a.as_slice()[19]).collect();
        kept.sort();
        assert_eq!(kept, vec![1, 2]);
    }
}

#[cfg(test)]
mod cross_pair_tests {
    use super::*;

    fn a(n: u8) -> Address {
        Address::with_last_byte(n)
    }

    #[test]
    fn cross_pairs_skip_bases_and_take_the_most_active_first() {
        let tokens = [a(1), a(9), a(2), a(3), a(4)]; // most active first; a(1) is a base
        let pairs = cross_pairs(&tokens, &[a(1)], 3);
        assert_eq!(pairs, vec![(a(9), a(2)), (a(9), a(3)), (a(2), a(3))]);
        assert!(cross_pairs(&tokens, &[a(1)], 0).is_empty());
        assert!(
            cross_pairs(&tokens, &[a(1)], 1).is_empty(),
            "one token makes no pair"
        );
    }
}
