//! What the bot tracks, written out so decoding can be checked against it.
//!
//! "All tracked markets and exchanges are decoded" is only checkable against a list of what is tracked. Written at
//! startup to `tracked-venues.json`: every pool in the store (venue, kind, tokens, fee, V4 id) and the lending markets.
//! `scripts/venue-coverage.py` joins it with the feed recording and the chain outcomes.

use alloy::primitives::Address;
use mev_types::{PoolInfo, PoolKind};
use serde_json::{Value, json};

fn kind_name(k: &PoolKind) -> (&'static str, Option<u32>) {
    match k {
        PoolKind::UniV2 { fee_bps } => ("uniswap_v2", Some(u32::from(*fee_bps))),
        PoolKind::CamelotV2 => ("camelot_v2", None),
        PoolKind::UniV3 { fee } => ("uniswap_v3", Some(*fee)),
        PoolKind::PancakeV3 { fee } => ("pancake_v3", Some(*fee)),
        PoolKind::AlgebraV3 => ("algebra_v3", None),
        PoolKind::UniV4 { fee, .. } => ("uniswap_v4", Some(*fee)),
    }
}

/// Pool contracts whose logs can be checked for unrecognised events: every pool that is a contract (V4 pools are
/// not; their events come from the PoolManager, which is added by the caller).
pub fn contract_addresses<'a>(pools: impl Iterator<Item = &'a PoolInfo>) -> Vec<Address> {
    pools
        .filter(|p| !matches!(p.kind, PoolKind::UniV4 { .. }))
        .map(|p| p.address)
        .collect()
}

pub fn describe<'a>(
    pools: impl Iterator<Item = &'a PoolInfo>,
    dexes: &[(String, String, Address)],
    v4_manager: Option<Address>,
    lending: Value,
) -> Value {
    let pools: Vec<Value> = pools
        .map(|p| {
            let (kind, fee) = kind_name(&p.kind);
            let v4_id = match &p.kind {
                PoolKind::UniV4 { id, .. } => Some(format!("{id:#x}")),
                _ => None,
            };
            json!({
                "address": format!("{:#x}", p.address),
                "dex": p.dex,
                "kind": kind,
                "fee": fee,
                "token0": format!("{:#x}", p.token0),
                "token1": format!("{:#x}", p.token1),
                "v4_id": v4_id,
            })
        })
        .collect();
    json!({
        "dexes": dexes.iter().map(|(n, k, f)| json!({ "name": n, "kind": k, "factory": format!("{f:#x}") })).collect::<Vec<_>>(),
        "v4_manager": v4_manager.map(|a| format!("{a:#x}")),
        "pools": pools,
        "lending": lending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;

    fn info(n: u8, kind: PoolKind) -> PoolInfo {
        PoolInfo {
            address: Address::with_last_byte(n),
            dex: format!("dex{n}"),
            kind,
            token0: Address::with_last_byte(100),
            token1: Address::with_last_byte(101),
        }
    }

    #[test]
    fn every_pool_kind_gets_a_name_a_fee_and_v4_pools_carry_their_id() {
        let v4 = PoolKind::UniV4 {
            fee: 500,
            tick_spacing: 10,
            native: false,
            flipped: false,
            id: B256::repeat_byte(7),
            manager: Address::with_last_byte(9),
        };
        let ps = [
            info(1, PoolKind::UniV2 { fee_bps: 30 }),
            info(2, PoolKind::CamelotV2),
            info(3, PoolKind::UniV3 { fee: 3000 }),
            info(4, PoolKind::PancakeV3 { fee: 2500 }),
            info(5, PoolKind::AlgebraV3),
            info(6, v4),
        ];
        let v = describe(
            ps.iter(),
            &[(
                "uniswap_v3".into(),
                "UniV3".into(),
                Address::with_last_byte(8),
            )],
            None,
            json!({}),
        );
        let kinds: Vec<_> = v["pools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            kinds,
            [
                "uniswap_v2",
                "camelot_v2",
                "uniswap_v3",
                "pancake_v3",
                "algebra_v3",
                "uniswap_v4"
            ]
        );
        assert_eq!(v["pools"][2]["fee"], 3000);
        assert!(v["pools"][1]["fee"].is_null());
        assert!(
            v["pools"][5]["v4_id"]
                .as_str()
                .unwrap()
                .starts_with("0x0707")
        );
        assert!(v["pools"][0]["v4_id"].is_null());
        assert_eq!(v["dexes"][0]["name"], "uniswap_v3");
    }

    #[test]
    fn v4_pools_are_not_contracts() {
        let v4 = PoolKind::UniV4 {
            fee: 1,
            tick_spacing: 1,
            native: false,
            flipped: false,
            id: B256::ZERO,
            manager: Address::ZERO,
        };
        let ps = [info(1, PoolKind::UniV3 { fee: 500 }), info(2, v4)];
        assert_eq!(
            contract_addresses(ps.iter()),
            vec![Address::with_last_byte(1)]
        );
    }
}
