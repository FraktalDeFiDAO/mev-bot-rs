//! Aerodrome V2 pools (Solidly-style, Base): their events. Reserves are `uint256`, so `Sync` and `Swap` have
//! different topics from Uniswap V2's even though the reserves sit in the same two data words. Router calldata
//! (`Route[]` paths) is not decoded yet. `Mint(address,uint256,uint256)` and the LP token's `Transfer` / `Approval`
//! have Uniswap V2's topics and are recognised there.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, PriceUpdate, Protocol, reserves_update};

sol! {
    interface Pool {
        event Swap(address indexed sender, address indexed to, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out);
        event Sync(uint256 reserve0, uint256 reserve1);
        event Burn(address indexed sender, address indexed to, uint256 amount0, uint256 amount1);
        event Fees(address indexed sender, uint256 amount0, uint256 amount1);
        event Claim(address indexed sender, address indexed recipient, uint256 amount0, uint256 amount1);
    }
}

pub struct Aerodrome;

impl Protocol for Aerodrome {
    fn id(&self) -> &'static str {
        "aerodrome"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(Pool::Swap::SIGNATURE_HASH, "swap_solidly", Class::Swap, p),
            EventKind::new(Pool::Sync::SIGNATURE_HASH, "sync_solidly", Class::State, p),
            EventKind::new(
                Pool::Burn::SIGNATURE_HASH,
                "burn_solidly",
                Class::Liquidity,
                p,
            ),
            // Swap fees moved to the pool's fee holder, and LPs claiming them: no reserve change.
            EventKind::new(Pool::Fees::SIGNATURE_HASH, "fees_solidly", Class::State, p),
            EventKind::new(
                Pool::Claim::SIGNATURE_HASH,
                "claim_solidly",
                Class::State,
                p,
            ),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "sync_solidly")
            .then(|| reserves_update(data))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, b256};

    #[test]
    fn topics_are_the_ones_aerodrome_pools_emit() {
        // Computed with `cast sig-event` and seen on Aerodrome's WETH/USDC pool (0xcDAC…5C43) on 2026-10-20.
        assert_eq!(
            Pool::Sync::SIGNATURE_HASH,
            b256!("cf2aa50876cdfbb541206f89af0ee78d44a2abf8d328e37fa4917f982149848a")
        );
        assert_eq!(
            Pool::Swap::SIGNATURE_HASH,
            b256!("b3e2773606abfd36b5bd91394b3a54d1398336c65005baf7bf7a05efeffaf75b")
        );
    }

    #[test]
    fn sync_carries_the_new_reserves() {
        let mut data = vec![0u8; 64];
        data[31] = 7;
        data[63] = 9;
        let kind = Aerodrome
            .events()
            .into_iter()
            .find(|k| k.name == "sync_solidly")
            .unwrap();
        assert_eq!(
            Aerodrome.price_update(&kind, &data),
            Some(PriceUpdate::Reserves {
                reserve0: U256::from(7),
                reserve1: U256::from(9),
            })
        );
    }
}
