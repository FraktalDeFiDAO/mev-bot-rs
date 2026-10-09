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
pub fn event_line(log: &Log, store: &PoolStore, t_ms: u64) -> Option<Value> {
    let topic0 = log.topics().first()?;
    let kind = events::classify(topic0)?;
    let pool = event_pool(log, kind.name);
    let known = store.get(&pool);
    let fields: serde_json::Map<String, Value> =
        events::describe(kind, log.topics(), log.data().data.as_ref())
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v)))
            .collect();
    Some(json!({
        "t_ms": t_ms,
        "block": log.block_number,
        "tx": log.transaction_hash.map(|h| format!("{h:#x}")),
        "tx_index": log.transaction_index,
        "log_index": log.log_index,
        "pool": format!("{pool:#x}"),
        "emitter": format!("{:#x}", log.address()),
        "dex": known.map(|p| p.info.dex.clone()),
        "tracked": known.is_some(),
        "kind": kind.name,
        "class": kind.class.as_str(),
        "protocol": kind.protocol,
        "removed": log.removed,
        "fields": fields,
    }))
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
        let v = event_line(&l, &PoolStore::default(), 1_000).unwrap();
        assert_eq!(v["kind"], "swap_v3");
        assert_eq!(v["class"], "swap");
        assert_eq!(v["tracked"], false);
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
        let v = event_line(&l, &PoolStore::default(), 1).unwrap();
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
        assert!(event_line(&l, &PoolStore::default(), 1).is_none());
        let none = Log::default();
        assert!(
            event_line(&none, &PoolStore::default(), 1).is_none(),
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
            let v = event_line(&l, &PoolStore::default(), 1).unwrap();
            assert_eq!(v["class"], "liquidity", "{name}");
        }
    }
}
