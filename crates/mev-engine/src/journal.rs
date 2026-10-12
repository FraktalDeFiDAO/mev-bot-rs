//! The on-chain journal: every event a tracked pool (or the V4 PoolManager) emits, as it arrives, one JSON line each.
//!
//! The engine already receives these through its log subscription, every block, so the journal costs no extra RPC.
//! Together with the feed journal (what the sequencer showed before the block) it is the complete record of the
//! watched exchanges: swaps with their resulting price, liquidity added and removed with the ticks and amounts, fee
//! collection, flash loans, pool creation, donations. Joined on the transaction hash.

use alloy::{primitives::Address, rpc::types::Log};
use mev_decode::events;
use mev_state::PoolStore;
use serde_json::{Value, json};

/// The pool an event belongs to, as the store names it: the emitting contract, or for V4 events the stand-in address
/// derived from the pool id in topic 1 (the emitter is the PoolManager for every V4 pool).
pub fn event_pool(log: &Log, kind_name: &str) -> Address {
    let v4 = matches!(
        kind_name,
        "swap_v4" | "modify_liquidity_v4" | "initialize_v4" | "donate_v4"
    );
    match (v4, log.topics().get(1)) {
        (true, Some(id)) => mev_protocol_v4_stand_in(id),
        _ => log.address(),
    }
}

fn mev_protocol_v4_stand_in(id: &alloy::primitives::B256) -> Address {
    Address::from_slice(&id[12..])
}

/// One journal line for a log, or `None` when it is not an event the table knows (nothing is invented for those).
pub fn event_line(
    log: &Log,
    store: &PoolStore,
    watch: &std::collections::HashMap<Address, String>,
    t_ms: u64,
) -> Option<Value> {
    let topic0 = log.topics().first()?;
    let kind = events::classify(topic0)?;
    let pool = event_pool(log, kind.name);
    let known = store.get(&pool);
    let dex = known
        .map(|p| p.info.dex.clone())
        .or_else(|| watch.get(&pool).cloned());
    let tier = if known.is_some() {
        "trade"
    } else if dex.is_some() {
        "watch"
    } else {
        "unlisted"
    };
    let fields: serde_json::Map<String, Value> =
        events::describe(kind, log.topics(), log.data().data.as_ref())
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v)))
            .collect();
    // How far this event moved the pool's price against what the cache held just before applying it: the number
    // that says whether the swap was big enough to open an edge. Display/telemetry only.
    let price_move_bps = known.and_then(|p| price_move_bps(p, kind, log.data().data.as_ref()));
    Some(json!({
        "price_move_bps": price_move_bps,
        "t_ms": t_ms,
        "block": log.block_number,
        "tx": log.transaction_hash.map(|h| format!("{h:#x}")),
        "tx_index": log.transaction_index,
        "log_index": log.log_index,
        "pool": format!("{pool:#x}"),
        "emitter": format!("{:#x}", log.address()),
        "dex": dex,
        "tier": tier,
        "tracked": tier != "unlisted",
        "kind": kind.name,
        "class": kind.class.as_str(),
        "protocol": kind.protocol,
        "removed": log.removed,
        "fields": fields,
    }))
}

/// Relative change of the pool's token0 price (token1 per token0) implied by the event, in basis points.
fn price_move_bps(pool: &mev_dex::Pool, kind: &events::Kind, data: &[u8]) -> Option<f64> {
    let before = pool.price0();
    if !(before.is_finite() && before > 0.0) {
        return None;
    }
    let after = match events::price_update(kind, data)? {
        events::PriceUpdate::SqrtPrice { sqrt_price_x96, .. } => {
            // V4 pools may be stored mirrored; leave their orientation to the dedicated state path.
            if matches!(pool.info.kind, mev_types::PoolKind::UniV4 { .. }) {
                return None;
            }
            let s = f64::from(sqrt_price_x96) / 2f64.powi(96);
            s * s
        }
        events::PriceUpdate::Reserves { reserve0, reserve1 } => {
            let r0 = f64::from(reserve0);
            if r0 <= 0.0 {
                return None;
            }
            f64::from(reserve1) / r0
        }
    };
    Some((after / before - 1.0) * 10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, Bytes, LogData, U256};

    fn log(emitter: Address, topics: Vec<B256>, data: Vec<u8>) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: emitter,
                data: LogData::new_unchecked(topics, Bytes::from(data)),
            },
            block_number: Some(500),
            transaction_hash: Some(B256::repeat_byte(0xaa)),
            transaction_index: Some(3),
            log_index: Some(7),
            ..Default::default()
        }
    }

    fn kind_topic(name: &str) -> B256 {
        events::kinds()
            .iter()
            .find(|k| k.name == name)
            .unwrap()
            .topic0
    }

    #[test]
    fn a_v3_swap_on_an_untracked_pool_is_journaled_with_its_amounts_and_marked_untracked() {
        let mut data = Vec::new();
        for v in [
            U256::from(5u8),
            U256::from(6u8),
            U256::from(7u8),
            U256::from(8u8),
            U256::from(9u8),
        ] {
            data.extend(v.to_be_bytes::<32>());
        }
        let pool = Address::with_last_byte(0x42);
        let l = log(
            pool,
            vec![kind_topic("swap_v3"), B256::ZERO, B256::ZERO],
            data,
        );
        let v = event_line(&l, &PoolStore::default(), &Default::default(), 1_000).unwrap();
        assert_eq!(v["kind"], "swap_v3");
        assert_eq!(v["class"], "swap");
        assert_eq!(
            (v["tracked"].as_bool(), v["tier"].as_str()),
            (Some(false), Some("unlisted"))
        );
        assert_eq!(v["pool"], format!("{pool:#x}"));
        assert_eq!(
            (v["block"].as_u64(), v["log_index"].as_u64()),
            (Some(500), Some(7))
        );
        assert_eq!(v["fields"]["amount1"], "6");
        assert_eq!(v["fields"]["sqrt_price_x96"], "7");
    }

    #[test]
    fn a_v4_event_is_attributed_to_the_pool_id_not_to_the_manager() {
        let manager = Address::with_last_byte(0x36);
        let id = B256::repeat_byte(0xcd);
        let l = log(
            manager,
            vec![kind_topic("swap_v4"), id, B256::ZERO],
            vec![0u8; 32 * 6],
        );
        let v = event_line(&l, &PoolStore::default(), &Default::default(), 1).unwrap();
        assert_eq!(v["pool"], format!("{:#x}", Address::from_slice(&id[12..])));
        assert_eq!(v["emitter"], format!("{manager:#x}"));
        assert_eq!(v["fields"]["pool_id"], format!("{id:#x}"));
    }

    #[test]
    fn an_event_the_table_does_not_know_is_not_invented() {
        let l = log(
            Address::with_last_byte(1),
            vec![B256::repeat_byte(0x11)],
            vec![],
        );
        assert!(event_line(&l, &PoolStore::default(), &Default::default(), 1).is_none());
        let none = Log::default();
        assert!(
            event_line(&none, &PoolStore::default(), &Default::default(), 1).is_none(),
            "a log with no topics"
        );
    }

    #[test]
    fn every_liquidity_event_kind_gets_a_line_with_the_liquidity_class() {
        for name in [
            "mint_v2",
            "burn_v2",
            "mint_v3",
            "burn_v3",
            "modify_liquidity_v4",
        ] {
            let l = log(
                Address::with_last_byte(1),
                vec![kind_topic(name), B256::ZERO, B256::ZERO, B256::ZERO],
                vec![0u8; 32 * 6],
            );
            let v = event_line(&l, &PoolStore::default(), &Default::default(), 1).unwrap();
            assert_eq!(v["class"], "liquidity", "{name}");
        }
    }

    #[test]
    fn a_pool_in_the_watch_tier_is_journaled_as_watched_with_its_venue() {
        let pool = Address::with_last_byte(0x61);
        let watch: std::collections::HashMap<Address, String> =
            [(pool, "uniswap_v3".to_string())].into_iter().collect();
        let l = log(
            pool,
            vec![kind_topic("swap_v3"), B256::ZERO, B256::ZERO],
            vec![0u8; 32 * 5],
        );
        let v = event_line(&l, &PoolStore::default(), &watch, 1).unwrap();
        assert_eq!(
            (
                v["tier"].as_str(),
                v["dex"].as_str(),
                v["tracked"].as_bool()
            ),
            (Some("watch"), Some("uniswap_v3"), Some(true))
        );
    }

    #[test]
    fn a_sync_records_how_far_it_moved_the_cached_price() {
        use mev_dex::{Pool, PoolState};
        use mev_types::{PoolInfo, PoolKind};
        let pool_addr = Address::with_last_byte(0x61);
        let mut store = PoolStore::default();
        store.insert(Pool::new(
            PoolInfo {
                address: pool_addr,
                dex: "uniswap_v2".into(),
                kind: PoolKind::UniV2 { fee_bps: 30 },
                token0: Address::with_last_byte(1),
                token1: Address::with_last_byte(2),
            },
            PoolState::V2 {
                reserve0: 1_000_000.0,
                reserve1: 1_000_000.0,
            },
        ));
        let mut data = Vec::new();
        for v in [U256::from(1_000_000u64), U256::from(1_010_000u64)] {
            data.extend(v.to_be_bytes::<32>());
        }
        let l = log(pool_addr, vec![kind_topic("sync_v2")], data);
        let v = event_line(&l, &store, &Default::default(), 1).unwrap();
        let bps = v["price_move_bps"].as_f64().unwrap();
        assert!((bps - 100.0).abs() < 1e-6, "{bps}");
        // An event on a pool we hold no state for has no cached price to compare with.
        let v = event_line(&l, &PoolStore::default(), &Default::default(), 1).unwrap();
        assert!(v["price_move_bps"].is_null());
    }
}
