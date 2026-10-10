//! A transaction's swap read from its calldata. Compatibility surface: the decoders live in the protocol crates
//! (`mev-protocol-uniswap-router`, `mev-protocol-unidentified`); this module keeps the names callers already use.

use alloy::primitives::Address;
use mev_protocol::Decoded;

pub use mev_protocol::{PoolRef, SwapIntent};
pub use mev_protocol_unidentified::{ROUTER_A, ROUTER_B, SELECTOR_A, SELECTOR_B};
pub use mev_protocol_uniswap_router::SWAP_ROUTER_02;

/// Former name of [`SWAP_ROUTER_02`], kept for the shadow-comparison labels.
pub const ROUTER_C: Address = SWAP_ROUTER_02;

/// The swap a call to `to` with `input` performs, if any protocol crate reads it.
pub fn decode(to: Address, input: &[u8]) -> Option<SwapIntent> {
    match crate::decode_call(to, input)? {
        Decoded::Swap(s) => Some(s),
        Decoded::Liquidity(_) | Decoded::Liquidation(_) | Decoded::Pools(_) => None,
    }
}
