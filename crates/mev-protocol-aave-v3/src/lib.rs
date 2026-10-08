//! Aave V3 on Arbitrum: liquidation event and the `liquidationCall` call on the Pool.
//! The runtime adapter (indexing, health sweeps, planning) is `mev-lending-aave-v3`.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, address},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{Class, Contract, Decoded, EventKind, LiquidationIntent, Protocol, abi};

sol! {
    interface Pool {
        event LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken);
        function liquidationCall(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken) external;
    }
}

/// Aave V3 Pool proxy on Arbitrum One.
pub const POOL: Address = address!("794a61358D6845594F94dc1DB02A252b5b4814aD");

pub struct AaveV3;

impl Protocol for AaveV3 {
    fn id(&self) -> &'static str {
        "aave_v3"
    }

    fn events(&self) -> Vec<EventKind> {
        vec![EventKind::new(
            Pool::LiquidationCall::SIGNATURE_HASH,
            "liquidation_aave",
            Class::Liquidation,
            self.id(),
        )]
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, args) = abi::split(input)?;
        if to != POOL || sel != Pool::liquidationCallCall::SELECTOR {
            return None;
        }
        let c = Pool::liquidationCallCall::abi_decode_raw(args).ok()?;
        Some(Decoded::Liquidation(LiquidationIntent {
            protocol: self.id(),
            borrowers: vec![c.user],
            collateral: Some(c.collateralAsset),
            debt: Some(c.debtAsset),
            // u256::MAX means "all of the debt" in Aave.
            amount: Some(c.debtToCover),
            market: None,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: POOL,
            name: "Aave V3 Pool (liquidationCall decoded)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, b256};

    #[test]
    fn the_event_topic_matches_the_known_mainnet_value() {
        let k = &AaveV3.events()[0];
        assert_eq!(
            k.topic0,
            b256!("e413a321e8681d831f4dbccbca790d2952b56f977908e45be37335533e005286")
        );
    }

    #[test]
    fn a_liquidation_call_to_the_pool_is_decoded() {
        let call = Pool::liquidationCallCall {
            collateralAsset: Address::with_last_byte(1),
            debtAsset: Address::with_last_byte(2),
            user: Address::with_last_byte(3),
            debtToCover: U256::from(1_000u64),
            receiveAToken: false,
        };
        let input = call.abi_encode();
        assert_eq!(
            &input[..4],
            [0x00, 0xa7, 0x18, 0xa9],
            "liquidationCall selector"
        );
        let Some(Decoded::Liquidation(l)) = AaveV3.decode_call(POOL, &input) else {
            panic!("not decoded")
        };
        assert_eq!(l.borrowers, vec![Address::with_last_byte(3)]);
        assert_eq!(l.collateral, Some(Address::with_last_byte(1)));
        assert_eq!(l.debt, Some(Address::with_last_byte(2)));
        assert_eq!(l.amount, Some(U256::from(1_000u64)));
        // Another contract, another selector, or garbage arguments: nothing.
        assert!(
            AaveV3
                .decode_call(Address::with_last_byte(9), &input)
                .is_none()
        );
        assert!(AaveV3.decode_call(POOL, &input[..20]).is_none());
        assert!(
            AaveV3
                .decode_call(POOL, &[0xde, 0xad, 0xbe, 0xef])
                .is_none()
        );
    }
}
