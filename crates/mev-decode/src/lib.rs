//! Standalone decoding of what Arbitrum transactions and events mean.
//!
//! Nothing here touches an RPC node, the engine, or pool state, so it can run alone (see the `mev-decode` command), be
//! used in analysis tools, or be taken in pieces:
//!
//!  - [`swap`]: a transaction's swap read from its calldata ([`SwapIntent`]): pool or pair, token in, amount, price limit.
//!  - [`events`]: the table of swap / liquidity / liquidation / state event shapes by topic0, their protocol, and the
//!    price a pool event leaves behind ([`events::price_update`]).
//!  - [`known_contract`]: which destinations we have identified, and whether their calls are decoded yet.
//!
//! What a feed transaction *does* is not knowable from its body: it may revert. Decoded calldata is an intent. Events
//! from block logs are what happened; prices for charts should come from them.

pub mod events;
pub mod swap;

use alloy::primitives::{Address, address};

pub use swap::{PoolRef, SwapIntent};

/// What a call's calldata means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Swap(SwapIntent),
}

/// Decode a call to `to` with `input`, if it is a contract and function we know.
pub fn decode_call(to: Address, input: &[u8]) -> Option<Decoded> {
    swap::decode(to, input).map(Decoded::Swap)
}

/// A destination we have identified.
#[derive(Debug, Clone, Copy)]
pub struct Contract {
    pub address: Address,
    pub name: &'static str,
    /// Whether [`decode_call`] reads calls to it.
    pub decoded: bool,
}

/// Destinations seen in the feed that move pools or lend, identified by address (and selector where noted). Used to
/// tell "known but not decoded yet" from "unknown" in coverage reports.
pub const CONTRACTS: &[Contract] = &[
    Contract {
        address: swap::SWAP_ROUTER_02,
        name: "Uniswap SwapRouter02 (exactInput decoded)",
        decoded: true,
    },
    Contract {
        address: swap::ROUTER_A,
        name: "unidentified, selector a00597a0",
        decoded: true,
    },
    Contract {
        address: swap::ROUTER_B,
        name: "unidentified, selector 000000e7",
        decoded: true,
    },
    Contract {
        address: address!("A51afAFe0263b40EdaEf0Df8781eA9aa03E381a3"),
        name: "Uniswap Universal Router",
        decoded: false,
    },
    Contract {
        address: address!("E592427A0AEce92De3Edee1F18E0157C05861564"),
        name: "Uniswap V3 SwapRouter (v1)",
        decoded: false,
    },
    Contract {
        address: address!("111111125421cA6dc452d289314280a0f8842A65"),
        name: "1inch AggregationRouter v6",
        decoded: false,
    },
    Contract {
        address: address!("4752ba5DBc23f44D87826276BF6Fd6b1C372aD24"),
        name: "Camelot Router (V2)",
        decoded: false,
    },
    Contract {
        address: address!("0000000071727De22E5E9d8BAf0edAc6f37da032"),
        name: "ERC-4337 EntryPoint v0.7 (wraps arbitrary calls)",
        decoded: false,
    },
];

/// The identified contract at `to`, if any.
pub fn known_contract(to: Address) -> Option<&'static Contract> {
    CONTRACTS.iter().find(|c| c.address == to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_flags_match_the_decoder() {
        // Every contract flagged `decoded` must really decode something, and no other contract may.
        for c in CONTRACTS {
            let reads = [
                swap::SELECTOR_A,
                swap::SELECTOR_B,
                swap::SELECTOR_EXACT_INPUT,
            ]
            .iter()
            .any(|sel| swap::decodes_selector(c.address, *sel));
            assert_eq!(reads, c.decoded, "{}", c.name);
        }
    }

    #[test]
    fn unknown_destinations_are_none() {
        assert!(known_contract(Address::with_last_byte(1)).is_none());
        assert!(known_contract(swap::SWAP_ROUTER_02).is_some());
    }
}
