//! Standalone decoding of what Arbitrum transactions and events mean.
//!
//! Each protocol is its own crate (`mev-protocol-<name>`, see `docs/design/protocol-crates.md`); this crate is the
//! registry that collects them, plus the `mev-decode` command. Nothing here touches an RPC node, the engine, or pool
//! state, so it can run alone, be used in analysis tools, or be taken in pieces:
//!
//!  - [`decode_call`]: what a call's calldata intends ([`Decoded`]): a swap ([`SwapIntent`]) or a liquidation.
//!  - [`calls::decode_all`] / [`calls::actions_in`]: every action in a transaction, through multicall, Multicall3,
//!    smart-account and ERC-4337 wrappers.
//!  - [`events`]: the table of swap / liquidity / liquidation / state event shapes by topic0 and the price a pool
//!    event leaves behind ([`events::price_update`]).
//!  - [`known_contract`]: which destinations are identified, and whether their calls are decoded yet.
//!
//! What a feed transaction *does* is not knowable from its body: it may revert. Decoded calldata is an intent. Events
//! from block logs are what happened; prices for charts should come from them.

pub mod calls;
pub mod events;
pub mod swap;

use alloy::primitives::{Address, address};

pub use mev_protocol::{
    Class, Contract, Decoded, LiquidationIntent, LiquidityAction, LiquidityIntent, PoolRef,
    PriceUpdate, Protocol, SwapIntent,
};

/// Every protocol crate, in a fixed order. Adding a protocol means adding it here.
pub fn protocols() -> &'static [&'static dyn Protocol] {
    static P: [&dyn Protocol; 17] = [
        &mev_protocol_uniswap_v2::UniswapV2,
        &mev_protocol_uniswap_v3::UniswapV3,
        &mev_protocol_uniswap_v4::UniswapV4,
        &mev_protocol_uniswap_router::UniswapRouter,
        &mev_protocol_pancake_v3::PancakeV3,
        &mev_protocol_balancer::Balancer,
        &mev_protocol_aave_v3::AaveV3,
        &mev_protocol_morpho_blue::MorphoBlue,
        &mev_protocol_compound_v3::CompoundV3,
        &mev_protocol_fluid::Fluid,
        &mev_protocol_oneinch::OneInch,
        &mev_protocol_cow::Cow,
        &mev_protocol_camelot::CamelotRouter,
        &mev_protocol_sushi::SushiRouter,
        &mev_protocol_paraswap::Paraswap,
        &mev_protocol_kyber::Kyber,
        &mev_protocol_unidentified::Unidentified,
    ];
    &P
}

/// Decode a call to `to` with `input`, if it is a contract and function a protocol crate knows.
pub fn decode_call(to: Address, input: &[u8]) -> Option<Decoded> {
    protocols().iter().find_map(|p| p.decode_call(to, input))
}

/// Like [`decode_call`], for a top-level transaction that also carries native ETH: the functions whose amount is the
/// transaction value (`swapExactETHForTokens`) decode here. Calls nested in wrappers have no value of their own.
pub fn decode_call_with_value(
    to: Address,
    input: &[u8],
    value: alloy::primitives::U256,
) -> Option<Decoded> {
    decode_call(to, input).or_else(|| {
        (!value.is_zero())
            .then(|| {
                protocols()
                    .iter()
                    .find_map(|p| p.decode_payable(to, input, value))
            })
            .flatten()
    })
}

/// Every intent in a call to `to` (several for a call that carries several, such as a Universal Router `execute` with
/// a swap per command); empty when no protocol crate reads it. The first intent equals [`decode_call`].
pub fn decode_calls(to: Address, input: &[u8]) -> Vec<Decoded> {
    protocols()
        .iter()
        .map(|p| p.decode_calls(to, input))
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Destinations seen in the feed that are identified but whose calls no protocol crate decodes yet.
const UNDECODED: [Contract; 1] = [Contract {
    address: address!("0000000071727De22E5E9d8BAf0edAc6f37da032"),
    name: "ERC-4337 EntryPoint v0.7 (wraps arbitrary calls)",
    decoded: false,
}];

/// Every identified destination: the protocol crates' contracts, then the undecoded ones.
pub fn contracts() -> &'static [Contract] {
    static C: std::sync::OnceLock<Vec<Contract>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        protocols()
            .iter()
            .flat_map(|p| p.contracts())
            .chain(UNDECODED)
            .collect()
    })
}

/// Destinations whose calldata carries the addresses of the pools it trades on literally (an aggregator's route data,
/// a bot's program), without the calldata giving one exact swap. The pools a pending call to one of them is about to
/// move are the watched pools that appear in its calldata; only the engine knows which pools are watched, so it does the
/// scan (see `Engine`'s arrival path). Measured as recall / precision of "watched pools in the calldata" against the pools
/// that swapped, on 2026-10-10 (the blind transactions) and, unchanged, on 2026-10-09 (all landed transactions with a
/// tracked swap): OKX-style router `0x09f94b5f` 100/81 and 100/77; 1inch v6 100/100 and 100/100; `0x20cce25c` 100/100
/// and 100/100; KyberSwap 100/73 and 100/84; CoW settlement 73/86 and 82/96; `0x00000000b4ec` 100/100 and 100/100;
/// `0xe6b82c6e` 100/100 and 100/100; `0x69bb29e2` 100/100 and 100/100; `0xaa0000dc` 96/100 and 98/100; `0xc6dd6c02`
/// 100/100 and 99/79; ERC-4337 EntryPoint v0.7 47/89 and 33/100; `0xdb9b1e94` 50/100 and 66/98. Paraswap v6 (recall 1-27%)
/// and the pull-and-call router (3%) do not qualify and are not listed. DECISIONS #167-#168.
pub const POOL_SCAN_DESTINATIONS: [Address; 18] = [
    mev_protocol_unidentified::dag_router::ROUTER_G,
    mev_protocol_oneinch::ROUTER_V6,
    address!("20cce25c88506e37d2812961802ce16a66ac5428"),
    mev_protocol_kyber::ROUTER,
    mev_protocol_cow::SETTLEMENT,
    address!("00000000b4ec312b95e8b1d4fd4eb0d1d2220975"),
    mev_protocol_unidentified::router_f::ROUTER_F,
    mev_protocol_unidentified::router_h::ROUTER_H,
    mev_protocol_unidentified::aa_bot::BOT,
    address!("c6dd6c021cfbb284f0c0d788547b51fc90fcf663"),
    calls::ENTRYPOINT_V7,
    address!("db9b1e94b5b69df7e401ddbede43491141047db3"),
    // Added in DECISIONS #170 (recall / precision on 2026-10-09 and 2026-10-10): 0x0b62f5f5 100/100 and 100/100;
    // 0x1231deb6 100/87 and 100/98; 0x22821c05 100/93 (10-10 only); 0x877af3fe 100/100 and 100/100; 0x8a1ba3d5 87/100
    // and 100/100; 0xe32d1dae 100/84 (10-10 only).
    address!("0b62f5f573d928115e73a5da0957a4b7cb94fee4"),
    address!("1231deb6f5749ef6ce6943a275a1d3e7486f4eae"),
    address!("22821c055d2fa0ff79266287fae610e8d81fcb21"),
    address!("877af3fe40bcd805edc64df96b3eaba2448a5ae7"),
    address!("8a1ba3d5b7864621a6214627a85a3f252b2e6180"),
    address!("e32d1dae75dcafa4a0270feafc27f5ac6f01dab8"),
];

/// Destinations whose calldata lists the tokens of its cycle and the fee tiers between them, but derives its pools (no pool
/// address appears): `0xfeacf3f2`. The pools a pending call to one of them can move are the watched pools of any pair of
/// the listed tokens whose fee tier (for fee-tier pools) appears in the calldata. Measured on 28 landed calls of 2026-10-10:
/// every swapped pool is in that set (recall 100%) at 12% precision, about 13 pools per call; it is used only because the
/// calls are rare (about 1,000-2,000 a day, each pending for a 2 s window). DECISIONS #169.
pub const PAIR_SCAN_DESTINATIONS: [Address; 1] =
    [address!("feacf3f2a6525b52661161f0d5ea6f62f5be6274")];

/// Whether the pools a call to `to` moves are found from the token pairs and fee tiers in its calldata.
pub fn scans_for_pairs(to: Address) -> bool {
    PAIR_SCAN_DESTINATIONS.contains(&to)
}

/// Whether the pools a call to `to` moves are found by scanning its calldata for watched pool addresses.
pub fn scans_for_pools(to: Address) -> bool {
    POOL_SCAN_DESTINATIONS.contains(&to)
}

/// The identified contract at `to`, if any.
pub fn known_contract(to: Address) -> Option<&'static Contract> {
    contracts().iter().find(|c| c.address == to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_measured_aggregators_are_scanned_for_pools() {
        assert!(scans_for_pools(mev_protocol_kyber::ROUTER));
        assert!(scans_for_pools(mev_protocol_cow::SETTLEMENT));
        assert!(!scans_for_pools(Address::with_last_byte(1)));
        let unique: std::collections::HashSet<_> = POOL_SCAN_DESTINATIONS.iter().collect();
        assert_eq!(unique.len(), POOL_SCAN_DESTINATIONS.len());
    }

    #[test]
    fn protocol_ids_and_contract_addresses_are_unique() {
        let mut ids = std::collections::HashSet::new();
        for p in protocols() {
            assert!(ids.insert(p.id()), "duplicate protocol id {}", p.id());
        }
        let mut addrs = std::collections::HashSet::new();
        for c in contracts() {
            assert!(addrs.insert(c.address), "duplicate contract {}", c.name);
        }
    }

    #[test]
    fn every_event_belongs_to_a_registered_protocol() {
        for k in events::kinds() {
            assert!(
                protocols().iter().any(|p| p.id() == k.protocol),
                "{} names unknown protocol {}",
                k.name,
                k.protocol
            );
        }
    }

    #[test]
    fn the_three_original_contracts_still_decode_through_the_registry() {
        // Layout details are tested in the protocol crates; this proves the registry reaches them.
        assert!(known_contract(swap::ROUTER_A).is_some_and(|c| c.decoded));
        assert!(known_contract(swap::ROUTER_B).is_some_and(|c| c.decoded));
        assert!(known_contract(swap::SWAP_ROUTER_02).is_some_and(|c| c.decoded));
        assert!(known_contract(Address::with_last_byte(1)).is_none());
    }

    #[test]
    fn calls_nobody_reads_decode_to_nothing() {
        assert!(decode_call(swap::ROUTER_A, &[0u8; 3]).is_none());
        assert!(decode_call(Address::with_last_byte(1), &[0xde, 0xad, 0xbe, 0xef]).is_none());
    }
}
