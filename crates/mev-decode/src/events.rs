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
}
