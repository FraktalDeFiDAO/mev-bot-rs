//! Dolomite (Arbitrum): a single `DolomiteMargin` contract holds every account `(owner, number)`. A position is
//! liquidated by an `operate` action; the margin contract then emits `LogLiquidate` (or `LogVaporize` when the
//! account's debt exceeds its collateral). Events only: there is no capture path yet (runtime adapter
//! `mev-lending-dolomite` is audit-pending). Event shapes are from dolomite-margin `Events.sol`; the topics are
//! checked against the contract's own history by the ignored live test in `mev-engine` (journal tests).

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, address},
    sol,
    sol_types::SolEvent,
};
use mev_protocol::{Class, Contract, EventKind, Protocol};

sol! {
    struct Wei { bool sign; uint256 value; }
    struct Par { bool sign; uint128 value; }
    struct BalanceUpdate { Wei deltaWei; Par newPar; }

    interface Margin {
        event LogLiquidate(
            address indexed solidAccountOwner,
            uint256 solidAccountNumber,
            address indexed liquidAccountOwner,
            uint256 liquidAccountNumber,
            uint256 heldMarket,
            uint256 owedMarket,
            BalanceUpdate solidHeldUpdate,
            BalanceUpdate solidOwedUpdate,
            BalanceUpdate liquidHeldUpdate,
            BalanceUpdate liquidOwedUpdate
        );
        event LogVaporize(
            address indexed solidAccountOwner,
            uint256 solidAccountNumber,
            address indexed vaporAccountOwner,
            uint256 vaporAccountNumber,
            uint256 heldMarket,
            uint256 owedMarket,
            BalanceUpdate solidHeldUpdate,
            BalanceUpdate solidOwedUpdate,
            BalanceUpdate vaporOwedUpdate
        );
    }
}

/// `DolomiteMargin` on Arbitrum One.
pub const MARGIN: Address = address!("6Bd780E7fDf01D77e4d475c821f1e7AE05409072");

pub struct Dolomite;

impl Protocol for Dolomite {
    fn id(&self) -> &'static str {
        "dolomite"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(
                Margin::LogLiquidate::SIGNATURE_HASH,
                "liquidation_dolomite",
                Class::Liquidation,
                p,
            ),
            EventKind::new(
                Margin::LogVaporize::SIGNATURE_HASH,
                "vaporize_dolomite",
                Class::Liquidation,
                p,
            ),
        ]
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: MARGIN,
            name: "DolomiteMargin",
            decoded: false,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topics_follow_the_nested_struct_signature() {
        // Solidity hashes the flattened tuple form of struct arguments.
        assert_eq!(
            Margin::LogLiquidate::SIGNATURE,
            "LogLiquidate(address,uint256,address,uint256,uint256,uint256,((bool,uint256),(bool,uint128)),((bool,uint256),(bool,uint128)),((bool,uint256),(bool,uint128)),((bool,uint256),(bool,uint128)))"
        );
        let kinds = Dolomite.events();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.iter().all(|k| k.class == Class::Liquidation));
    }
}
