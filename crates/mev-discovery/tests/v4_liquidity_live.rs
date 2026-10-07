//! Live read-only check (ignored by default): applying a real V4 `ModifyLiquidity` log to the state
//! read one block earlier must give the liquidity the PoolManager reports one block later.
//! Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL (archive-capable) and MEV_IT_V4_REGISTRY (a saved registry).

use std::path::Path;

use alloy::{
    eips::BlockId,
    primitives::{B256, U256},
    providers::Provider,
    rpc::types::Filter,
    sol_types::SolEvent,
};
use mev_bindings::{IPoolManager, decode_v4_slot0, v4_slot_plus, v4_state_slot};
use mev_dex::PoolState;
use mev_discovery::{load_pools, v4};
use mev_state::PoolStore;
use mev_types::PoolKind;

async fn word(
    rpc: &mev_rpc::Rpc,
    manager: alloy::primitives::Address,
    slot: B256,
    at: u64,
) -> U256 {
    let pm = IPoolManager::new(manager, &rpc.http);
    let v = pm
        .extsload(slot)
        .block(BlockId::number(at))
        .call()
        .await
        .expect("extsload at block");
    U256::from_be_bytes(v.0)
}

#[tokio::test]
#[ignore = "network"]
async fn applying_a_real_modify_liquidity_log_reproduces_the_managers_liquidity() {
    let http = std::env::var("ARBITRUM_HTTP_URL").expect("ARBITRUM_HTTP_URL");
    let ws = std::env::var("ARBITRUM_WS_URL").expect("ARBITRUM_WS_URL");
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap();
    let v4cfg = cfg.uniswap_v4.clone().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("v4.json");
    std::fs::copy(
        std::env::var("MEV_IT_V4_REGISTRY").expect("MEV_IT_V4_REGISTRY"),
        &registry,
    )
    .unwrap();
    let infos = v4::discover(&cfg, &v4cfg, &registry).await;
    let pools = load_pools(&rpc, infos).await.unwrap();
    let mut store = PoolStore::default();
    for p in pools {
        store.insert(p);
    }
    let head = rpc.http.get_block_number().await.unwrap();
    // Recent ModifyLiquidity logs on the manager, in small ranges (public endpoints cap getLogs).
    let (mut checked, mut moved) = (0, 0);
    let mut to = head;
    while to > head.saturating_sub(6_000) && checked < 6 {
        let from = to.saturating_sub(499);
        let filter = Filter::new()
            .address(v4cfg.pool_manager)
            .event_signature(IPoolManager::ModifyLiquidity::SIGNATURE_HASH)
            .from_block(from)
            .to_block(to);
        to = from.saturating_sub(1);
        let Ok(logs) = rpc.http.get_logs(&filter).await else {
            continue;
        };
        for log in logs {
            let Some(id) = log.topics().get(1).copied() else {
                continue;
            };
            let Some(block) = log.block_number else {
                continue;
            };
            let Some(addr) = store
                .pools()
                .find(|p| matches!(p.info.kind, PoolKind::UniV4 { id: i, .. } if i == id))
                .map(|p| p.info.address)
            else {
                continue;
            };
            // Only blocks where this pool had a single event (so before -> after is just this log).
            let same: Vec<_> = rpc
                .http
                .get_logs(
                    &Filter::new()
                        .address(v4cfg.pool_manager)
                        .topic1(id)
                        .from_block(block)
                        .to_block(block),
                )
                .await
                .unwrap_or_default();
            if same.len() != 1 {
                continue;
            }
            let PoolKind::UniV4 {
                flipped, manager, ..
            } = store.get(&addr).unwrap().info.kind
            else {
                continue;
            };
            let base = v4_state_slot(id);
            let (slot0_before, liq_before, liq_after) = (
                word(&rpc, manager, base, block - 1).await,
                word(&rpc, manager, v4_slot_plus(base, 3), block - 1).await,
                word(&rpc, manager, v4_slot_plus(base, 3), block).await,
            );
            let (sqrt_x96, _tick, _, _) = decode_v4_slot0(B256::from(slot0_before));
            let s = mev_dex::sqrt_price_from_x96(f64::from(sqrt_x96));
            let mask = (U256::from(1u8) << 128) - U256::from(1u8);
            let mut p = store.get(&addr).unwrap().clone();
            p.state = PoolState::V3 {
                sqrt_price: if flipped { 1.0 / s } else { s },
                liquidity: f64::from(liq_before & mask),
            };
            store.replace(p);
            let applied = store.apply_log(&log);
            let PoolState::V3 { liquidity, .. } = store.get(&addr).unwrap().state else {
                panic!()
            };
            let want = f64::from(liq_after & mask);
            println!(
                "block {block}: before {:.4e} -> model {liquidity:.4e}, chain {want:.4e}, applied {}",
                f64::from(liq_before & mask),
                applied.is_some()
            );
            assert!(
                (liquidity - want).abs() <= 1e-9 * want.max(1.0),
                "liquidity after the event differs"
            );
            checked += 1;
            if applied.is_some() {
                moved += 1;
            }
        }
    }
    assert!(
        checked >= 1,
        "found at least one single-event ModifyLiquidity block"
    );
    println!("checked {checked}, of which changed active liquidity {moved}");
}
