//! The table of every event shape the bot recognises, collected from the protocol crates.
//!
//! Chain logs are the source of truth for prices and for whether a transaction did anything: a feed transaction that
//! reverts emits none. Adding a protocol means adding its crate to `crate::protocols()`; nothing here names a protocol.

pub use mev_protocol::{Class, PriceUpdate};

/// One recognised event shape (see `mev_protocol::EventKind`).
pub type Kind = mev_protocol::EventKind;

/// Every event the bot recognises, in registry order.
pub fn kinds() -> &'static [Kind] {
    static T: std::sync::OnceLock<Vec<Kind>> = std::sync::OnceLock::new();
    T.get_or_init(|| crate::protocols().iter().flat_map(|p| p.events()).collect())
}

/// Topics of every event a *pool* (or the V4 PoolManager) emits that the table knows: swaps, liquidity changes, state
/// restatements, fee collection, flash loans, pool creation and donations. Not the position manager's own events
/// (their liquidity is also in the pool's `Mint` / `Burn`), and not lending or Balancer events (other emitters).
pub fn pool_event_topics() -> Vec<alloy::primitives::B256> {
    const POOL_PROTOCOLS: [&str; 4] = ["uniswap_v2", "uniswap_v3", "pancake_v3", "uniswap_v4"];
    kinds()
        .iter()
        .filter(|k| POOL_PROTOCOLS.contains(&k.protocol) && !k.name.ends_with("_nft"))
        .map(|k| k.topic0)
        .collect()
}

/// The recognised kind of an event with this `topic0`.
pub fn classify(topic0: &alloy::primitives::B256) -> Option<&'static Kind> {
    kinds().iter().find(|k| &k.topic0 == topic0)
}

/// The price after the event, for the events that carry one (`Swap` on V3-style pools and V4, `Sync` on V2). A
/// reverted transaction emits no events, so a price taken from logs is always a price that happened.
pub fn price_update(kind: &Kind, data: &[u8]) -> Option<PriceUpdate> {
    crate::protocols()
        .iter()
        .find(|p| p.id() == kind.protocol)
        .and_then(|p| p.price_update(kind, data))
}

/// Every event the table knows, as named fields (numbers in decimal, addresses and ids in hex), read from the log's
/// topics and data. Pool and lending events of every protocol in the registry; an unknown kind gives no fields.
/// Signed amounts keep their sign (`amount0` of a V3 swap is negative when the pool pays it out).
pub fn describe(
    kind: &Kind,
    topics: &[alloy::primitives::B256],
    data: &[u8],
) -> Vec<(&'static str, String)> {
    use alloy::primitives::{I256, U256};
    let w = |i: usize| -> Option<U256> { data.get(32 * i..32 * i + 32).map(U256::from_be_slice) };
    let s = |i: usize| w(i).map(|v| I256::from_raw(v).to_string());
    let u = |i: usize| w(i).map(|v| v.to_string());
    let a = |i: usize| {
        w(i).map(|v| {
            format!(
                "{:#x}",
                alloy::primitives::Address::from_word(v.to_be_bytes::<32>().into())
            )
        })
    };
    let ta = |i: usize| {
        topics
            .get(i)
            .map(|t| format!("{:#x}", alloy::primitives::Address::from_word(*t)))
    };
    let th = |i: usize| topics.get(i).map(|t| format!("{t:#x}"));
    // An indexed int24 is sign-extended into its 32-byte topic.
    let tint = |i: usize| {
        topics
            .get(i)
            .map(|t| i32::from_be_bytes([t[28], t[29], t[30], t[31]]).to_string())
    };
    let wint = |i: usize| {
        w(i).map(|v| {
            i32::from_be_bytes([
                v.to_be_bytes::<32>()[28],
                v.to_be_bytes::<32>()[29],
                v.to_be_bytes::<32>()[30],
                v.to_be_bytes::<32>()[31],
            ])
            .to_string()
        })
    };
    let rows: Vec<(&'static str, Option<String>)> = match kind.name {
        "swap_v2" => vec![
            ("sender", ta(1)),
            ("to", ta(2)),
            ("amount0_in", u(0)),
            ("amount1_in", u(1)),
            ("amount0_out", u(2)),
            ("amount1_out", u(3)),
        ],
        "sync_v2" => vec![("reserve0", u(0)), ("reserve1", u(1))],
        "mint_v2" => vec![("sender", ta(1)), ("amount0", u(0)), ("amount1", u(1))],
        "burn_v2" => vec![
            ("sender", ta(1)),
            ("to", ta(2)),
            ("amount0", u(0)),
            ("amount1", u(1)),
        ],
        "swap_v3" | "swap_v3_pancake" => vec![
            ("sender", ta(1)),
            ("recipient", ta(2)),
            ("amount0", s(0)),
            ("amount1", s(1)),
            ("sqrt_price_x96", u(2)),
            ("liquidity", u(3)),
            ("tick", wint(4)),
            (
                "protocol_fees_token0",
                if kind.name == "swap_v3_pancake" {
                    u(5)
                } else {
                    None
                },
            ),
            (
                "protocol_fees_token1",
                if kind.name == "swap_v3_pancake" {
                    u(6)
                } else {
                    None
                },
            ),
        ],
        "mint_v3" => vec![
            ("owner", ta(1)),
            ("tick_lower", tint(2)),
            ("tick_upper", tint(3)),
            ("sender", a(0)),
            ("liquidity", u(1)),
            ("amount0", u(2)),
            ("amount1", u(3)),
        ],
        "burn_v3" => vec![
            ("owner", ta(1)),
            ("tick_lower", tint(2)),
            ("tick_upper", tint(3)),
            ("liquidity", u(0)),
            ("amount0", u(1)),
            ("amount1", u(2)),
        ],
        "collect_v3" => vec![
            ("owner", ta(1)),
            ("tick_lower", tint(2)),
            ("tick_upper", tint(3)),
            ("recipient", a(0)),
            ("amount0", u(1)),
            ("amount1", u(2)),
        ],
        "flash_v3" => vec![
            ("sender", ta(1)),
            ("recipient", ta(2)),
            ("amount0", u(0)),
            ("amount1", u(1)),
            ("paid0", u(2)),
            ("paid1", u(3)),
        ],
        "increase_liquidity_nft" | "decrease_liquidity_nft" => vec![
            ("token_id", th(1)),
            ("liquidity", u(0)),
            ("amount0", u(1)),
            ("amount1", u(2)),
        ],
        "swap_v4" => vec![
            ("pool_id", th(1)),
            ("sender", ta(2)),
            ("amount0", s(0)),
            ("amount1", s(1)),
            ("sqrt_price_x96", u(2)),
            ("liquidity", u(3)),
            ("tick", wint(4)),
            ("fee", u(5)),
        ],
        "modify_liquidity_v4" => vec![
            ("pool_id", th(1)),
            ("sender", ta(2)),
            ("tick_lower", wint(0)),
            ("tick_upper", wint(1)),
            ("liquidity_delta", s(2)),
            ("salt", w(3).map(|v| format!("{v:#x}"))),
        ],
        "initialize_v4" => vec![
            ("pool_id", th(1)),
            ("currency0", ta(2)),
            ("currency1", ta(3)),
            ("fee", u(0)),
            ("tick_spacing", wint(1)),
            ("hooks", a(2)),
            ("sqrt_price_x96", u(3)),
            ("tick", wint(4)),
        ],
        "donate_v4" => vec![
            ("pool_id", th(1)),
            ("sender", ta(2)),
            ("amount0", u(0)),
            ("amount1", u(1)),
        ],
        // Camelot V3 notification before each swap: two values (250 and 250 seen), most likely the fee in effect. Kept unlabelled.
        "fee_algebra_observed" => vec![("value0", u(0)), ("value1", u(1))],
        "swap_balancer" => vec![
            ("pool_id", th(1)),
            ("token_in", ta(2)),
            ("token_out", ta(3)),
            ("amount_in", u(0)),
            ("amount_out", u(1)),
        ],
        "liquidation_aave" => vec![
            ("collateral", ta(1)),
            ("debt", ta(2)),
            ("user", ta(3)),
            ("debt_to_cover", u(0)),
            ("collateral_liquidated", u(1)),
            ("liquidator", a(2)),
        ],
        "liquidation_morpho" => vec![
            ("market_id", th(1)),
            ("caller", ta(2)),
            ("borrower", ta(3)),
            ("repaid_assets", u(0)),
            ("repaid_shares", u(1)),
            ("seized_assets", u(2)),
            ("bad_debt_assets", u(3)),
            ("bad_debt_shares", u(4)),
        ],
        "absorb_collateral_compound" => vec![
            ("absorber", ta(1)),
            ("borrower", ta(2)),
            ("asset", ta(3)),
            ("collateral_absorbed", u(0)),
            ("usd_value", u(1)),
        ],
        "absorb_debt_compound" => vec![
            ("absorber", ta(1)),
            ("borrower", ta(2)),
            ("base_paid_out", u(0)),
            ("usd_value", u(1)),
        ],
        "liquidation_fluid" => vec![
            ("liquidator", a(0)),
            ("collateral_amount", u(1)),
            ("debt_amount", u(2)),
            ("to", a(3)),
        ],
        "absorb_fluid" => vec![("collateral_amount", u(0)), ("debt_amount", u(1))],
        _ => Vec::new(),
    };
    rows.into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use alloy::sol_types::SolEvent;

    #[test]
    fn every_kind_has_a_distinct_topic_and_name() {
        let mut topics = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for k in kinds() {
            assert!(topics.insert(k.topic0), "duplicate topic for {}", k.name);
            assert!(names.insert(k.name), "duplicate name {}", k.name);
        }
        assert!(kinds().len() >= 17, "{}", kinds().len());
    }

    #[test]
    fn the_definitions_agree_with_the_bot_bindings() {
        // The bot's own bindings define the same events; if they ever drift apart this catches it.
        let by = |n: &str| kinds().iter().find(|k| k.name == n).unwrap().topic0;
        assert_eq!(
            by("swap_v3"),
            mev_bindings::IUniswapV3Pool::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            by("mint_v3"),
            mev_bindings::IUniswapV3Pool::Mint::SIGNATURE_HASH
        );
        assert_eq!(
            by("burn_v3"),
            mev_bindings::IUniswapV3Pool::Burn::SIGNATURE_HASH
        );
        assert_eq!(
            by("swap_v3_pancake"),
            mev_bindings::IPancakeV3Pool::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            by("sync_v2"),
            mev_bindings::IUniswapV2Pair::Sync::SIGNATURE_HASH
        );
        assert_eq!(
            by("swap_v4"),
            mev_bindings::IPoolManager::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            by("modify_liquidity_v4"),
            mev_bindings::IPoolManager::ModifyLiquidity::SIGNATURE_HASH
        );
    }

    #[test]
    fn prices_are_routed_to_the_owning_protocol() {
        let swap = kinds().iter().find(|k| k.name == "swap_v3").unwrap();
        let mut data = vec![0u8; 32 * 5];
        data[32 * 2 + 31] = 7;
        assert!(matches!(
            price_update(swap, &data),
            Some(PriceUpdate::SqrtPrice { sqrt_price_x96, .. }) if sqrt_price_x96 == U256::from(7)
        ));
        let burn = kinds().iter().find(|k| k.name == "burn_v3").unwrap();
        assert_eq!(price_update(burn, &data), None);
    }

    fn word(v: U256) -> Vec<u8> {
        v.to_be_bytes::<32>().to_vec()
    }
    fn addr_topic(a: u8) -> alloy::primitives::B256 {
        let mut t = [0u8; 32];
        t[31] = a;
        t.into()
    }
    fn field<'a>(f: &'a [(&'static str, String)], k: &str) -> &'a str {
        f.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.as_str())
            .unwrap_or("<missing>")
    }

    #[test]
    fn a_v3_swap_keeps_the_sign_of_its_amounts_and_reads_the_tick() {
        use alloy::primitives::I256;
        let kind = kinds().iter().find(|k| k.name == "swap_v3").unwrap();
        let mut data = Vec::new();
        data.extend(word(I256::try_from(-1_500i64).unwrap().into_raw())); // pool pays token0 out
        data.extend(word(U256::from(2_000u64)));
        data.extend(word(U256::from(79_228_162_514_264_337_593_543_950_336u128)));
        data.extend(word(U256::from(12_345u64)));
        data.extend(word(I256::try_from(-60i64).unwrap().into_raw())); // tick -60, sign-extended
        let f = describe(kind, &[kind.topic0, addr_topic(7), addr_topic(8)], &data);
        assert_eq!(field(&f, "amount0"), "-1500");
        assert_eq!(field(&f, "amount1"), "2000");
        assert_eq!(field(&f, "tick"), "-60");
        assert_eq!(field(&f, "liquidity"), "12345");
        assert_eq!(
            field(&f, "sender"),
            "0x0000000000000000000000000000000000000007"
        );
        assert!(
            f.iter().all(|(k, _)| !k.starts_with("protocol_fees")),
            "only Pancake has those"
        );
    }

    #[test]
    fn a_v3_mint_reads_the_owner_the_ticks_and_the_amounts() {
        use alloy::primitives::I256;
        let kind = kinds().iter().find(|k| k.name == "mint_v3").unwrap();
        let neg = |v: i64| {
            alloy::primitives::B256::from(I256::try_from(v).unwrap().into_raw().to_be_bytes::<32>())
        };
        let mut data = Vec::new();
        data.extend(word(U256::from(9u8))); // sender
        data.extend(word(U256::from(1_000_000u64))); // liquidity
        data.extend(word(U256::from(11u8)));
        data.extend(word(U256::from(22u8)));
        let f = describe(
            kind,
            &[kind.topic0, addr_topic(5), neg(-887_220), neg(887_220)],
            &data,
        );
        assert_eq!(
            (field(&f, "tick_lower"), field(&f, "tick_upper")),
            ("-887220", "887220")
        );
        assert_eq!(
            (
                field(&f, "liquidity"),
                field(&f, "amount0"),
                field(&f, "amount1")
            ),
            ("1000000", "11", "22")
        );
        assert_eq!(
            field(&f, "owner"),
            "0x0000000000000000000000000000000000000005"
        );
    }

    #[test]
    fn every_event_in_the_table_is_described_or_deliberately_has_no_fields() {
        // A kind added to the table without a `describe` arm would silently journal as an empty record.
        let empty_data = vec![0u8; 32 * 8];
        let topics = [addr_topic(1), addr_topic(2), addr_topic(3), addr_topic(4)];
        for k in kinds() {
            let mut t = topics.to_vec();
            t[0] = k.topic0;
            assert!(
                !describe(k, &t, &empty_data).is_empty(),
                "no fields for {}",
                k.name
            );
        }
    }

    #[test]
    fn unknown_kinds_and_short_data_give_no_invented_fields() {
        let k = kinds().iter().find(|k| k.name == "swap_v3").unwrap();
        let f = describe(k, &[k.topic0], &[1u8; 10]);
        assert!(
            f.iter()
                .all(|(n, _)| !matches!(*n, "amount0" | "amount1" | "tick")),
            "short data must not yield amounts: {f:?}"
        );
        let fake = Kind {
            topic0: alloy::primitives::B256::ZERO,
            name: "nope",
            class: Class::State,
            protocol: "x",
        };
        assert!(describe(&fake, &[], &[0u8; 64]).is_empty());
    }

    #[test]
    fn the_pool_subscription_topics_cover_every_pool_event_and_no_other_emitter() {
        let topics = pool_event_topics();
        let has = |n: &str| topics.contains(&kinds().iter().find(|k| k.name == n).unwrap().topic0);
        for n in [
            "swap_v2",
            "mint_v2",
            "burn_v2",
            "sync_v2",
            "swap_v3",
            "mint_v3",
            "burn_v3",
            "collect_v3",
            "flash_v3",
            "swap_v3_pancake",
            "swap_v4",
            "modify_liquidity_v4",
            "initialize_v4",
            "donate_v4",
            "fee_algebra_observed",
        ] {
            assert!(has(n), "{n} must be subscribed");
        }
        for n in [
            "increase_liquidity_nft",
            "decrease_liquidity_nft",
            "swap_balancer",
            "liquidation_aave",
            "liquidation_fluid",
        ] {
            assert!(!has(n), "{n} is not emitted by a pool");
        }
        let unique: std::collections::HashSet<_> = topics.iter().collect();
        assert_eq!(unique.len(), topics.len());
    }
}
