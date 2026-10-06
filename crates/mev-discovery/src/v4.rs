//! Uniswap V4 pool discovery. All V4 pools live inside one PoolManager contract; a pool is named by
//! the hash of its key, and the only on-chain list of keys is the manager's `Initialize` events. So:
//!
//! 1. a registry of every hookless pool (hooks = address(0), static fee, usable spacing) built from
//!    `Initialize` events, cached on disk and extended from the last scanned block;
//! 2. the pools that swapped recently (`Swap` events over the activity window), most active first;
//! 3. `PoolInfo`s for them, with the native ETH side mapped to WETH (the executor wraps/unwraps).
//!
//! Hooked pools are skipped: a hook can change a swap's price or fee, which our model can't see.

use std::{collections::HashMap, path::Path};

use alloy::{
    primitives::{Address, B256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::IPoolManager;
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
        match p.get_logs(&filter).await {
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
            Err(e) if span > 100_000 => {
                // Too wide for this endpoint: narrow the range and retry.
                warn!(error = %e, span, "v4 initialize scan chunk failed; narrowing");
                span /= 4;
            }
            Err(e) => return Err(e).context("v4 initialize scan"),
        }
    }
    Ok(())
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
        match p.get_logs(&filter).await {
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
    scan_initialize(&provider, v4, &mut registry, head).await?;
    if (registry.pools.len() != before || !registry_path.exists())
        && let Err(e) = registry.save(registry_path)
    {
        warn!(error = %e, "could not save the v4 pool registry");
    }
    let activity = swap_activity(&provider, v4, head).await;
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
