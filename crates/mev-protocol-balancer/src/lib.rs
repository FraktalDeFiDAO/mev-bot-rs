//! Balancer V2 Vault. Classified for coverage; no Balancer pool is modelled for pricing yet.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{sol, sol_types::SolEvent};
use mev_protocol::{Class, EventKind, Protocol};

sol! {
    interface Vault {
        event Swap(bytes32 indexed poolId, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut);
    }
}

pub struct Balancer;

impl Protocol for Balancer {
    fn id(&self) -> &'static str {
        "balancer"
    }

    fn events(&self) -> Vec<EventKind> {
        vec![EventKind::new(
            Vault::Swap::SIGNATURE_HASH,
            "swap_balancer",
            Class::Swap,
            self.id(),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_swap_event_of_the_swap_class() {
        let ev = Balancer.events();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].class, Class::Swap);
    }
}
