//! Uniswap V4: every pool lives inside the singleton PoolManager; events name the pool by id.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, sqrt_price_update};

sol! {
    interface PoolManager {
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, uint24 fee, int24 tickSpacing, address hooks, uint160 sqrtPriceX96, int24 tick);
        event Donate(bytes32 indexed id, address indexed sender, uint256 amount0, uint256 amount1);
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
            // A new pool, and a donation to a pool's LPs: no swap or liquidity change, but part of the full record.
            EventKind::new(
                PoolManager::Initialize::SIGNATURE_HASH,
                "initialize_v4",
                Class::State,
                p,
            ),
            EventKind::new(
                PoolManager::Donate::SIGNATURE_HASH,
                "donate_v4",
                Class::State,
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
    fn the_four_events_are_distinct_and_only_swap_has_a_price() {
        let ev = UniswapV4.events();
        assert_eq!(ev.len(), 4);
        let topics: std::collections::HashSet<_> = ev.iter().map(|k| k.topic0).collect();
        assert_eq!(topics.len(), 4);
        let data = vec![0u8; 32 * 6];
        for k in &ev {
            assert_eq!(
                UniswapV4.price_update(k, &data).is_some(),
                k.name == "swap_v4",
                "{}",
                k.name
            );
        }
        // Only a swap or a liquidity change is of those classes; a new pool and a donation are state.
        let class = |n: &str| ev.iter().find(|k| k.name == n).unwrap().class;
        assert_eq!(class("swap_v4"), Class::Swap);
        assert_eq!(class("modify_liquidity_v4"), Class::Liquidity);
        assert_eq!(
            (class("initialize_v4"), class("donate_v4")),
            (Class::State, Class::State)
        );
    }
}
