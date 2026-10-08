//! Uniswap V4: every pool lives inside the singleton PoolManager; events name the pool by id.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, sqrt_price_update};

sol! {
    interface PoolManager {
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
    }
}

pub struct UniswapV4;

impl Protocol for UniswapV4 {
    fn id(&self) -> &'static str {
        "uniswap_v4"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(PoolManager::Swap::SIGNATURE_HASH, "swap_v4", Class::Swap, p),
            EventKind::new(
                PoolManager::ModifyLiquidity::SIGNATURE_HASH,
                "modify_liquidity_v4",
                Class::Liquidity,
                p,
            ),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "swap_v4")
            .then(|| sqrt_price_update(data))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_events_are_distinct_and_only_swap_has_a_price() {
        let ev = UniswapV4.events();
        assert_eq!(ev.len(), 2);
        assert_ne!(ev[0].topic0, ev[1].topic0);
        let data = vec![0u8; 32 * 6];
        assert!(UniswapV4.price_update(&ev[0], &data).is_some());
        assert_eq!(UniswapV4.price_update(&ev[1], &data), None);
    }
}
