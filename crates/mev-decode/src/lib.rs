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
    static P: [&dyn Protocol; 11] = [
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
        &mev_protocol_unidentified::Unidentified,
    ];
    &P
}

/// Decode a call to `to` with `input`, if it is a contract and function a protocol crate knows.
pub fn decode_call(to: Address, input: &[u8]) -> Option<Decoded> {
    protocols().iter().find_map(|p| p.decode_call(to, input))
}

/// Destinations seen in the feed that are identified but whose calls no protocol crate decodes yet.
const UNDECODED: [Contract; 3] = [
    Contract {
        address: address!("4752ba5DBc23f44D87826276BF6Fd6b1C372aD24"),
        name: "Camelot Router (V2)",
        decoded: false,
    },
    Contract {
        address: address!("111111125421cA6dc452d289314280a0f8842A65"),
        name: "1inch AggregationRouter v6",
        decoded: false,
    },
    Contract {
        address: address!("0000000071727De22E5E9d8BAf0edAc6f37da032"),
        name: "ERC-4337 EntryPoint v0.7 (wraps arbitrary calls)",
        decoded: false,
    },
];

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

/// The identified contract at `to`, if any.
pub fn known_contract(to: Address) -> Option<&'static Contract> {
    contracts().iter().find(|c| c.address == to)
}

#[cfg(test)]
mod tests {
    use super::*;

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
