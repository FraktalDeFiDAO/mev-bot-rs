//! PancakeSwap V3: Uniswap V3 math, but the `Swap` event carries two extra protocol-fee fields, so it has its own topic.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, sqrt_price_update};

sol! {
    interface Pool {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint128 protocolFeesToken0, uint128 protocolFeesToken1);
    }
}

pub struct PancakeV3;

impl Protocol for PancakeV3 {
    fn id(&self) -> &'static str {
        "pancake_v3"
    }

    fn events(&self) -> Vec<EventKind> {
        vec![EventKind::new(
            Pool::Swap::SIGNATURE_HASH,
            "swap_v3_pancake",
            Class::Swap,
            self.id(),
        )]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "swap_v3_pancake")
            .then(|| sqrt_price_update(data))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pancake_swap_topic_differs_from_uniswaps() {
        // Uniswap V3 Swap topic0 (c42079f9...) is a different event from Pancake's; both can appear on one chain.
        let uniswap_topic = "c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67";
        let ev = PancakeV3.events();
        assert_ne!(format!("{:x}", ev[0].topic0), uniswap_topic);
        assert!(PancakeV3.price_update(&ev[0], &vec![0u8; 32 * 7]).is_some());
    }
}
