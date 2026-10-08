//! Uniswap V2-style constant-product pairs (also Sushi V2 and Camelot V2). Events only: routers are separate crates.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, reserves_update};

sol! {
    interface Pair {
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Sync(uint112 reserve0, uint112 reserve1);
        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    }
}

pub struct UniswapV2;

impl Protocol for UniswapV2 {
    fn id(&self) -> &'static str {
        "uniswap_v2"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(Pair::Swap::SIGNATURE_HASH, "swap_v2", Class::Swap, p),
            EventKind::new(Pair::Sync::SIGNATURE_HASH, "sync_v2", Class::State, p),
            EventKind::new(Pair::Mint::SIGNATURE_HASH, "mint_v2", Class::Liquidity, p),
            EventKind::new(Pair::Burn::SIGNATURE_HASH, "burn_v2", Class::Liquidity, p),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "sync_v2")
            .then(|| reserves_update(data))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    #[test]
    fn topics_match_the_known_mainnet_values() {
        let by = |n: &str| {
            UniswapV2
                .events()
                .into_iter()
                .find(|k| k.name == n)
                .unwrap()
                .topic0
        };
        assert_eq!(
            by("swap_v2"),
            b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822")
        );
        assert_eq!(
            by("sync_v2"),
            b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1")
        );
    }

    #[test]
    fn only_sync_carries_a_price() {
        let ev = UniswapV2.events();
        let sync = ev.iter().find(|k| k.name == "sync_v2").unwrap();
        let swap = ev.iter().find(|k| k.name == "swap_v2").unwrap();
        let mut d = vec![0u8; 64];
        d[31] = 3;
        d[63] = 4;
        assert!(matches!(
            UniswapV2.price_update(sync, &d),
            Some(PriceUpdate::Reserves { .. })
        ));
        assert_eq!(UniswapV2.price_update(swap, &d), None);
    }
}
