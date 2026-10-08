//! Uniswap V3-style pools (also Sushi V3; Camelot V3/Algebra emits the same `Swap` shape and so shares the topic).
//! Events only: the routers that call these pools are in `mev-protocol-uniswap-router`.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, sqrt_price_update};

sol! {
    interface Pool {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }
    interface NftManager {
        event IncreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event DecreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
    }
}

pub struct UniswapV3;

impl Protocol for UniswapV3 {
    fn id(&self) -> &'static str {
        "uniswap_v3"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(Pool::Swap::SIGNATURE_HASH, "swap_v3", Class::Swap, p),
            EventKind::new(Pool::Mint::SIGNATURE_HASH, "mint_v3", Class::Liquidity, p),
            EventKind::new(Pool::Burn::SIGNATURE_HASH, "burn_v3", Class::Liquidity, p),
            EventKind::new(
                NftManager::IncreaseLiquidity::SIGNATURE_HASH,
                "increase_liquidity_nft",
                Class::Liquidity,
                p,
            ),
            EventKind::new(
                NftManager::DecreaseLiquidity::SIGNATURE_HASH,
                "decrease_liquidity_nft",
                Class::Liquidity,
                p,
            ),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "swap_v3")
            .then(|| sqrt_price_update(data))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    #[test]
    fn the_swap_topic_matches_the_known_mainnet_value() {
        let swap = UniswapV3
            .events()
            .into_iter()
            .find(|k| k.name == "swap_v3")
            .unwrap();
        assert_eq!(
            swap.topic0,
            b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67")
        );
    }

    #[test]
    fn a_swap_carries_a_price_and_a_burn_does_not() {
        let ev = UniswapV3.events();
        let swap = ev.iter().find(|k| k.name == "swap_v3").unwrap();
        let burn = ev.iter().find(|k| k.name == "burn_v3").unwrap();
        let mut data = vec![0u8; 32 * 5];
        data[32 * 2 + 31] = 7;
        assert!(matches!(
            UniswapV3.price_update(swap, &data),
            Some(PriceUpdate::SqrtPrice { .. })
        ));
        assert_eq!(UniswapV3.price_update(burn, &data), None);
    }
}
