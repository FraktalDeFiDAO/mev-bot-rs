//! Morpho Blue on Arbitrum: liquidation event and the `liquidate` call. Runtime adapter: `mev-lending-morpho`.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, B256, address, keccak256},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{Class, Contract, Decoded, EventKind, LiquidationIntent, Protocol, abi};

sol! {
    struct MarketParams {
        address loanToken;
        address collateralToken;
        address oracle;
        address irm;
        uint256 lltv;
    }
    interface Morpho {
        event Liquidate(bytes32 indexed id, address indexed caller, address indexed borrower, uint256 repaidAssets, uint256 repaidShares, uint256 seizedAssets, uint256 badDebtAssets, uint256 badDebtShares);
        function liquidate(MarketParams marketParams, address borrower, uint256 seizedAssets, uint256 repaidShares, bytes data) external;
    }
}

/// Morpho Blue on Arbitrum One.
pub const MORPHO: Address = address!("6c247b1F6182318877311737BaC0844bAa518F5e");

/// A market's id: the hash of its parameters.
pub fn market_id(p: &MarketParams) -> B256 {
    let mut b = Vec::with_capacity(160);
    for a in [p.loanToken, p.collateralToken, p.oracle, p.irm] {
        b.extend_from_slice(&[0u8; 12]);
        b.extend_from_slice(a.as_slice());
    }
    b.extend_from_slice(&p.lltv.to_be_bytes::<32>());
    keccak256(b)
}

pub struct MorphoBlue;

impl Protocol for MorphoBlue {
    fn id(&self) -> &'static str {
        "morpho_blue"
    }

    fn events(&self) -> Vec<EventKind> {
        vec![EventKind::new(
            Morpho::Liquidate::SIGNATURE_HASH,
            "liquidation_morpho",
            Class::Liquidation,
            self.id(),
        )]
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, args) = abi::split(input)?;
        if to != MORPHO || sel != Morpho::liquidateCall::SELECTOR {
            return None;
        }
        let c = Morpho::liquidateCall::abi_decode_raw(args).ok()?;
        Some(Decoded::Liquidation(LiquidationIntent {
            protocol: self.id(),
            borrowers: vec![c.borrower],
            collateral: Some(c.marketParams.collateralToken),
            debt: Some(c.marketParams.loanToken),
            // Collateral to seize when stated; otherwise the call names debt shares and the amount is not in units we know.
            amount: (!c.seizedAssets.is_zero()).then_some(c.seizedAssets),
            market: Some(market_id(&c.marketParams)),
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: MORPHO,
            name: "Morpho Blue (liquidate decoded)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn params() -> MarketParams {
        MarketParams {
            loanToken: Address::with_last_byte(1),
            collateralToken: Address::with_last_byte(2),
            oracle: Address::with_last_byte(3),
            irm: Address::with_last_byte(4),
            lltv: U256::from(860_000_000_000_000_000u64),
        }
    }

    #[test]
    fn a_liquidate_call_gives_borrower_tokens_market_and_amount() {
        let call = Morpho::liquidateCall {
            marketParams: params(),
            borrower: Address::with_last_byte(9),
            seizedAssets: U256::from(500u64),
            repaidShares: U256::ZERO,
            data: Default::default(),
        };
        let input = call.abi_encode();
        let Some(Decoded::Liquidation(l)) = MorphoBlue.decode_call(MORPHO, &input) else {
            panic!("not decoded")
        };
        assert_eq!(l.borrowers, vec![Address::with_last_byte(9)]);
        assert_eq!(l.collateral, Some(Address::with_last_byte(2)));
        assert_eq!(l.debt, Some(Address::with_last_byte(1)));
        assert_eq!(l.amount, Some(U256::from(500u64)));
        assert_eq!(l.market, Some(market_id(&params())));
        assert!(
            MorphoBlue
                .decode_call(Address::with_last_byte(9), &input)
                .is_none()
        );
    }

    #[test]
    fn repaying_shares_gives_no_amount_and_the_market_id_depends_on_every_field() {
        let call = Morpho::liquidateCall {
            marketParams: params(),
            borrower: Address::with_last_byte(9),
            seizedAssets: U256::ZERO,
            repaidShares: U256::from(77u64),
            data: Default::default(),
        };
        let Some(Decoded::Liquidation(l)) = MorphoBlue.decode_call(MORPHO, &call.abi_encode())
        else {
            panic!()
        };
        assert_eq!(l.amount, None);
        let mut other = params();
        other.lltv += U256::from(1);
        assert_ne!(market_id(&params()), market_id(&other));
    }
}
