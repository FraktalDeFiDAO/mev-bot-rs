//! Uniswap V4 pool discovery. All V4 pools live inside one PoolManager contract; a pool is named by
//! the hash of its key, and the only on-chain list of keys is the manager's `Initialize` events. So:
//!
//! 1. a registry of every hookless pool (hooks = address(0), static fee, usable spacing) built from
//!    `Initialize` events, cached on disk and extended from the last scanned block;
//! 2. the pools that swapped recently (`Swap` events over the activity window), most active first;
//! 3. `PoolInfo`s for them, with the native ETH side mapped to WETH (the executor wraps/unwraps).
//!
//! Where the free RPCs can't scan `Initialize` history (Base, 2026-10-20), active pools missing from
//! the registry get their keys from the PositionManager's `poolKeys` table instead (`position_manager`).
//!
//! Hooked pools are skipped: a hook can change a swap's price or fee, which our model can't see.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use alloy::{
    primitives::{Address, B256, FixedBytes, keccak256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::Filter,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use mev_bindings::{IMulticall3, IPoolManager, IV4PositionManager, MULTICALL3};
use mev_config::{ChainConfig, UniswapV4Config};
use mev_types::{PoolInfo, PoolKind, v4_pool_address};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Blocks per `Initialize` request (the manager has few such events; the official RPC allows wide
/// ranges for a single address and topic).
const INIT_SPAN: u64 = 4_000_000;
/// Dynamic-fee flag in a pool's fee field (needs a hook to set the fee).
const DYNAMIC_FEE_FLAG: u32 = 0x80_0000;

/// One hookless pool's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct V4Pool {
    pub id: B256,
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    /// Last block whose `Initialize` events are included.
    pub last_block: u64,
    pub pools: Vec<V4Pool>,
}

impl Registry {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Keep a pool only if we can model it exactly: no hook, a static fee that fits the executor's
/// hop (`u16`), and a spacing the executor accepts.
pub fn usable(hooks: Address, fee: u32, tick_spacing: i32) -> bool {
    hooks.is_zero()
        && fee & DYNAMIC_FEE_FLAG == 0
        && fee <= u32::from(u16::MAX)
        && (1..=32_767).contains(&tick_spacing)
}

/// `PoolInfo` for a registry pool: the native currency (address 0) becomes WETH, and the pair is put
/// in address order (`flipped` records when that reversed the pool's currency order).
pub fn pool_info(p: &V4Pool, weth: Address, manager: Address) -> Option<PoolInfo> {
    let native = p.currency0.is_zero();
    let t0 = if native { weth } else { p.currency0 };
    let t1 = p.currency1;
    if t0 == t1 || t1.is_zero() {
        return None;
    }
    let flipped = t0 > t1;
    let (token0, token1) = if flipped { (t1, t0) } else { (t0, t1) };
    Some(PoolInfo {
        address: v4_pool_address(p.id),
        dex: "uniswap_v4".into(),
        kind: PoolKind::UniV4 {
            fee: p.fee,
            tick_spacing: p.tick_spacing,
            native,
            flipped,
            id: p.id,
            manager,
        },
        token0,
        token1,
    })
}

/// The pools to track: registry pools with at least `min_swaps` swaps in `activity`, most active first.
pub fn select<'a>(
    registry: &'a Registry,
    activity: &HashMap<B256, u64>,
    min_swaps: u64,
    max_pools: usize,
) -> Vec<&'a V4Pool> {
    let mut ranked: Vec<(&V4Pool, u64)> = registry
        .pools
        .iter()
        .filter_map(|p| activity.get(&p.id).map(|n| (p, *n)))
        .filter(|(_, n)| *n >= min_swaps)
        .collect();
    ranked.sort_by_key(|(p, n)| (std::cmp::Reverse(*n), p.id));
    ranked.truncate(max_pools);
    ranked.into_iter().map(|(p, _)| p).collect()
}

/// A rate limit (HTTP 429): wait and retry the same request, never narrow it.
fn is_rate_limit(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("429") || m.contains("too many requests") || m.contains("rate limit")
}

/// The endpoint refused the block range itself: narrow the request.
fn is_range_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("range")
        || m.contains("span")
        || m.contains("too large")
        || m.contains("limit exceeded")
}

/// `get_logs` that waits out rate limits (1, 2, 4, 8, 16 s) before giving up.
async fn get_logs_retry(
    p: &DynProvider,
    filter: &Filter,
) -> Result<Vec<alloy::rpc::types::Log>, String> {
    let mut wait = 1;
    for attempt in 0..6 {
        match p.get_logs(filter).await {
            Ok(logs) => return Ok(logs),
            Err(e) => {
                let msg = e.to_string();
                if attempt < 5 && is_rate_limit(&msg) {
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    wait *= 2;
                } else {
                    return Err(msg);
                }
            }
        }
    }
    Err("rate limited".into())
}

fn logs_provider(url: &str) -> Result<DynProvider> {
    Ok(ProviderBuilder::new()
        .connect_http(url.parse().context("invalid uniswap_v4.log_rpc")?)
        .erased())
}

/// Extend the registry with pools initialized since `registry.last_block`.
async fn scan_initialize(
    p: &DynProvider,
    v4: &UniswapV4Config,
    registry: &mut Registry,
    head: u64,
) -> Result<()> {
    let mut start = registry.last_block.max(v4.first_block.saturating_sub(1)) + 1;
    let mut span = INIT_SPAN;
    while start <= head {
        let end = (start + span - 1).min(head);
        let filter = Filter::new()
            .address(v4.pool_manager)
            .event_signature(IPoolManager::Initialize::SIGNATURE_HASH)
            .from_block(start)
            .to_block(end);
        match get_logs_retry(p, &filter).await {
            Ok(logs) => {
                for l in logs {
                    let Ok(ev) = IPoolManager::Initialize::decode_log_data(l.data()) else {
                        continue;
                    };
                    let (fee, spacing) = (ev.fee.to::<u32>(), ev.tickSpacing.as_i32());
                    if usable(ev.hooks, fee, spacing) {
                        registry.pools.push(V4Pool {
                            id: ev.id,
                            currency0: ev.currency0,
                            currency1: ev.currency1,
                            fee,
                            tick_spacing: spacing as u32,
                        });
                    }
                }
                registry.last_block = end;
                start = end + 1;
            }
            // Down to ~4k blocks: free endpoints cap ranges at 10k (dRPC, Base) or less.
            Err(e) if span > 5_000 && is_range_error(&e) => {
                // Too wide for this endpoint: narrow the range and retry.
                warn!(error = %e, span, "v4 initialize scan chunk failed; narrowing");
                span /= 4;
            }
            Err(e) => anyhow::bail!("v4 initialize scan: {e}"),
        }
    }
    Ok(())
}

/// The pool id of a key: keccak256(abi.encode(currency0, currency1, fee, tickSpacing, hooks)).
fn pool_id(c0: Address, c1: Address, fee: u32, spacing: i32, hooks: Address) -> B256 {
    let mut b = [0u8; 160];
    b[12..32].copy_from_slice(c0.as_slice());
    b[44..64].copy_from_slice(c1.as_slice());
    b[92..96].copy_from_slice(&fee.to_be_bytes());
    // int24, sign-extended to 32 bytes.
    let fill = if spacing < 0 { 0xff } else { 0 };
    b[96..124].fill(fill);
    b[124..128].copy_from_slice(&spacing.to_be_bytes());
    b[140..160].copy_from_slice(hooks.as_slice());
    keccak256(b)
}

/// Hookless pools among `ids`, keyed from the PositionManager's `poolKeys` (Multicall3 batches).
/// A key is kept only if it hashes back to the id; pools it has never seen return an empty key.
async fn keys_from_position_manager(p: &DynProvider, posm: Address, ids: &[B256]) -> Vec<V4Pool> {
    let mc = IMulticall3::new(MULTICALL3, p);
    let mut out = Vec::new();
    for chunk in ids.chunks(150) {
        let calls: Vec<IMulticall3::Call3> = chunk
            .iter()
            .map(|id| IMulticall3::Call3 {
                target: posm,
                allowFailure: true,
                callData: IV4PositionManager::poolKeysCall {
                    poolId: FixedBytes::<25>::from_slice(&id[..25]),
                }
                .abi_encode()
                .into(),
            })
            .collect();
        let results = match mc.aggregate3(calls).call().await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "v4 poolKeys batch failed; skipping");
                continue;
            }
        };
        for (id, r) in chunk.iter().zip(results) {
            if !r.success {
                continue;
            }
            let Ok(k) = IV4PositionManager::poolKeysCall::abi_decode_returns(&r.returnData) else {
                continue;
            };
            let (fee, spacing) = (k.fee.to::<u32>(), k.tickSpacing.as_i32());
            if usable(k.hooks, fee, spacing)
                && pool_id(k.currency0, k.currency1, fee, spacing, k.hooks) == *id
            {
                out.push(V4Pool {
                    id: *id,
                    currency0: k.currency0,
                    currency1: k.currency1,
                    fee,
                    tick_spacing: spacing as u32,
                });
            }
        }
    }
    out
}

/// Swaps per pool id over the last `v4.blocks` blocks.
async fn swap_activity(p: &DynProvider, v4: &UniswapV4Config, head: u64) -> HashMap<B256, u64> {
    let mut activity: HashMap<B256, u64> = HashMap::new();
    let mut start = head.saturating_sub(v4.blocks);
    while start <= head {
        let end = (start + v4.step.max(1) - 1).min(head);
        let filter = Filter::new()
            .address(v4.pool_manager)
            .event_signature(IPoolManager::Swap::SIGNATURE_HASH)
            .from_block(start)
            .to_block(end);
        match get_logs_retry(p, &filter).await {
            Ok(logs) => {
                for l in logs {
                    if let Some(id) = l.topics().get(1) {
                        *activity.entry(*id).or_default() += 1;
                    }
                }
            }
            Err(e) => warn!(error = %e, start, end, "v4 swap scan chunk failed; skipping"),
        }
        start = end + 1;
    }
    activity
}

/// The V4 pools to track (see the module docs). Failures leave V4 out rather than failing startup.
pub async fn discover(
    cfg: &ChainConfig,
    v4: &UniswapV4Config,
    registry_path: &Path,
) -> Vec<PoolInfo> {
    match try_discover(cfg, v4, registry_path).await {
        Ok(infos) => infos,
        Err(e) => {
            warn!(error = %format!("{e:#}"), "uniswap v4 discovery failed; continuing without V4 pools");
            Vec::new()
        }
    }
}

async fn try_discover(
    cfg: &ChainConfig,
    v4: &UniswapV4Config,
    registry_path: &Path,
) -> Result<Vec<PoolInfo>> {
    let weth = cfg.token(&cfg.native_token)?;
    let provider = logs_provider(&v4.log_rpc)?;
    let head = provider.get_block_number().await?;
    let mut registry = Registry::load(registry_path);
    let before = registry.pools.len();
    if let Err(e) = scan_initialize(&provider, v4, &mut registry, head).await {
        if v4.position_manager.is_none() {
            return Err(e);
        }
        warn!(error = %format!("{e:#}"), "v4 initialize scan failed; active pools' keys come from the PositionManager");
    }
    let activity = swap_activity(&provider, v4, head).await;
    if let Some(posm) = v4.position_manager {
        let known: HashSet<B256> = registry.pools.iter().map(|p| p.id).collect();
        let mut missing: Vec<(B256, u64)> = activity
            .iter()
            .filter(|(id, n)| **n >= v4.min_swaps && !known.contains(*id))
            .map(|(id, n)| (*id, *n))
            .collect();
        missing.sort_by_key(|(id, n)| (std::cmp::Reverse(*n), *id));
        missing.truncate(v4.max_pools.saturating_mul(2));
        let ids: Vec<B256> = missing.into_iter().map(|(id, _)| id).collect();
        let found = keys_from_position_manager(&provider, posm, &ids).await;
        info!(
            looked_up = ids.len(),
            hookless = found.len(),
            "v4 pool keys from the PositionManager"
        );
        registry.pools.extend(found);
    }
    if (registry.pools.len() != before || !registry_path.exists())
        && let Err(e) = registry.save(registry_path)
    {
        warn!(error = %e, "could not save the v4 pool registry");
    }
    let chosen = select(&registry, &activity, v4.min_swaps, v4.max_pools);
    let infos: Vec<PoolInfo> = chosen
        .iter()
        .filter_map(|p| pool_info(p, weth, v4.pool_manager))
        .collect();
    info!(
        registry = registry.pools.len(),
        active = activity.len(),
        tracked = infos.len(),
        "uniswap v4 pools"
    );
    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_id_matches_a_live_base_pool() {
        // Base USDC/USDT, fee 5, spacing 1, no hooks: its Swap events carry id 0x7681c00520...
        // (PositionManager.poolKeys, 2026-10-20).
        let usdc: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
            .parse()
            .unwrap();
        let usdt: Address = "0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2"
            .parse()
            .unwrap();
        let id = pool_id(usdc, usdt, 5, 1, Address::ZERO);
        assert!(format!("{id:#x}").starts_with("0x7681c00520"), "{id:#x}");
        // A negative spacing is sign-extended, so it hashes differently from its absolute value.
        assert_ne!(
            pool_id(usdc, usdt, 5, -1, Address::ZERO),
            pool_id(usdc, usdt, 5, 1, Address::ZERO)
        );
    }

    const WETH: Address = Address::with_last_byte(0x82);
    const USDC: Address = Address::with_last_byte(0xaf);
    const ZRO: Address = Address::with_last_byte(0x69);
    const MGR: Address = Address::with_last_byte(0x36);

    fn pool(id: u8, c0: Address, c1: Address, fee: u32, ts: u32) -> V4Pool {
        V4Pool {
            id: B256::repeat_byte(id),
            currency0: c0,
            currency1: c1,
            fee,
            tick_spacing: ts,
        }
    }

    #[test]
    fn rate_limits_are_retried_and_range_errors_narrow_the_request() {
        let limited = "server returned an error response: error code 429: Too Many Requests";
        assert!(is_rate_limit(limited) && !is_range_error(limited));
        let range = "query spans 1000000 blocks (1 to 1000000), limit is 100000";
        assert!(is_range_error(range) && !is_rate_limit(range));
        assert!(!is_rate_limit("connection reset") && !is_range_error("connection reset"));
    }

    #[test]
    fn only_hookless_static_fee_pools_the_executor_can_name_are_usable() {
        let z = Address::ZERO;
        assert!(usable(z, 500, 10));
        assert!(!usable(Address::with_last_byte(1), 500, 10), "hooked");
        assert!(!usable(z, 0x80_0000 | 3000, 60), "dynamic fee");
        assert!(!usable(z, 70_000, 60), "fee does not fit the hop");
        assert!(!usable(z, 500, 0), "no spacing");
        assert!(!usable(z, 500, 40_000), "spacing out of range");
        assert!(!usable(z, 500, -10), "negative spacing");
    }

    #[test]
    fn a_native_pool_trades_as_weth_and_keeps_address_order() {
        // ETH/USDC: ETH (0x00) is currency0, USDC > WETH, so the order holds.
        let i = pool_info(&pool(1, Address::ZERO, USDC, 500, 10), WETH, MGR).unwrap();
        assert_eq!((i.token0, i.token1), (WETH, USDC));
        let PoolKind::UniV4 {
            native, flipped, ..
        } = i.kind
        else {
            panic!()
        };
        assert!(native && !flipped);
        assert_eq!(i.kind.hop_kind(), 4);
        // ETH/ZRO: ZRO < WETH, so mapping ETH to WETH reverses the order: flipped.
        let i = pool_info(&pool(2, Address::ZERO, ZRO, 3000, 60), WETH, MGR).unwrap();
        assert_eq!((i.token0, i.token1), (ZRO, WETH));
        let PoolKind::UniV4 { flipped, .. } = i.kind else {
            panic!()
        };
        assert!(flipped);
    }

    #[test]
    fn an_erc20_pool_keeps_its_currencies_and_hop_kind() {
        let i = pool_info(&pool(3, WETH, USDC, 500, 10), WETH, MGR).unwrap();
        assert_eq!((i.token0, i.token1), (WETH, USDC));
        assert_eq!(i.kind.hop_kind(), 3);
        assert_eq!(i.address, v4_pool_address(B256::repeat_byte(3)));
        assert!(pool_info(&pool(4, USDC, USDC, 500, 10), WETH, MGR).is_none());
    }

    #[test]
    fn the_most_active_pools_win_and_quiet_ones_are_dropped() {
        let reg = Registry {
            last_block: 1,
            pools: vec![
                pool(1, WETH, USDC, 500, 10),
                pool(2, WETH, ZRO, 3000, 60),
                pool(3, USDC, ZRO, 3000, 60),
                pool(4, WETH, USDC, 100, 1),
            ],
        };
        let act: HashMap<B256, u64> = [(1, 50), (2, 7), (3, 2), (9, 99)]
            .into_iter()
            .map(|(i, n)| (B256::repeat_byte(i), n))
            .collect();
        let picked = select(&reg, &act, 3, 10);
        assert_eq!(
            picked.iter().map(|p| p.id[0]).collect::<Vec<_>>(),
            vec![1, 2],
            "3 is too quiet, 4 and 9 never swapped or are unknown"
        );
        assert_eq!(select(&reg, &act, 3, 1).len(), 1);
    }

    #[test]
    fn the_registry_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("v4.json");
        assert_eq!(Registry::load(&path).pools.len(), 0, "missing file = empty");
        let reg = Registry {
            last_block: 42,
            pools: vec![pool(1, WETH, USDC, 500, 10)],
        };
        reg.save(&path).unwrap();
        let back = Registry::load(&path);
        assert_eq!((back.last_block, back.pools), (42, reg.pools));
    }
}
