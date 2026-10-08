//! Compound III on Arbitrum: positions are liquidated by `absorb`. Runtime adapter: `mev-lending-compound-v3`.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, address},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{Class, Contract, Decoded, EventKind, LiquidationIntent, Protocol, abi};

sol! {
    interface Comet {
        event AbsorbCollateral(address indexed absorber, address indexed borrower, address indexed asset, uint256 collateralAbsorbed, uint256 usdValue);
        event AbsorbDebt(address indexed absorber, address indexed borrower, uint256 basePaidOut, uint256 usdValue);
        function absorb(address absorber, address[] accounts) external;
    }
}

/// The comets configured in `config/arbitrum.toml` (`[[compound_v3.comets]]`).
pub const COMETS: [(Address, &str); 4] = [
    (
        address!("9c4ec768c28520B50860ea7a15bd7213a9fF58bf"),
        "Compound III cUSDCv3",
    ),
    (
        address!("d98Be00b5D27fc98112BdE293e487f8D4cA57d07"),
        "Compound III cUSDTv3 (USDT0)",
    ),
    (
        address!("6f7D514bbD4aFf3BcD1140B7344b32f063dEe486"),
        "Compound III cWETHv3",
    ),
    (
        address!("A5EDBDD9646f8dFF606d7448e414884C7d905dCA"),
        "Compound III cUSDCev3 (USDC.e)",
    ),
];

pub struct CompoundV3;

impl Protocol for CompoundV3 {
    fn id(&self) -> &'static str {
        "compound_v3"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(
                Comet::AbsorbCollateral::SIGNATURE_HASH,
                "absorb_collateral_compound",
                Class::Liquidation,
                p,
            ),
            EventKind::new(
                Comet::AbsorbDebt::SIGNATURE_HASH,
                "absorb_debt_compound",
                Class::Liquidation,
                p,
            ),
        ]
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, args) = abi::split(input)?;
        if sel != Comet::absorbCall::SELECTOR || !COMETS.iter().any(|(a, _)| *a == to) {
            return None;
        }
        let c = Comet::absorbCall::abi_decode_raw(args).ok()?;
        Some(Decoded::Liquidation(LiquidationIntent {
            protocol: self.id(),
            borrowers: c.accounts,
            collateral: None,
            debt: None,
            amount: None,
            market: None,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        COMETS
            .iter()
            .map(|(address, name)| Contract {
                address: *address,
                name,
                decoded: true,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_on_a_configured_comet_lists_every_account() {
        let call = Comet::absorbCall {
            absorber: Address::with_last_byte(1),
            accounts: vec![Address::with_last_byte(5), Address::with_last_byte(6)],
        };
        let input = call.abi_encode();
        let Some(Decoded::Liquidation(l)) = CompoundV3.decode_call(COMETS[0].0, &input) else {
            panic!("not decoded")
        };
        assert_eq!(
            l.borrowers,
            vec![Address::with_last_byte(5), Address::with_last_byte(6)]
        );
        assert_eq!(l.protocol, "compound_v3");
        assert!(
            CompoundV3
                .decode_call(Address::with_last_byte(9), &input)
                .is_none(),
            "not a comet"
        );
        assert_eq!(CompoundV3.contracts().len(), 4);
    }
}
