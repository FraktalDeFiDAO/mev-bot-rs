//! Fluid vaults on Arbitrum. A vault liquidates by price range, not per account: anyone calls
//! `vault.liquidate(debtAmt, colPerUnitDebt, to, absorb)` and buys discounted collateral with debt. So a decoded
//! liquidation names the vault (as `market`), not a borrower. Runtime adapter: `mev-lending-fluid`.
//!
//! Event shapes were checked against the two configured vaults' own history (2026-10-09, about a month of blocks):
//! `LogLiquidate` 4 data words (72 times), `LogAbsorb` 2 words (3 times), `LogOperate` 5 words (not a liquidation).

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, B256, U256, address},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{Class, Contract, Decoded, EventKind, LiquidationIntent, Protocol, abi};

sol! {
    interface Vault {
        event LogLiquidate(address liquidator, uint256 colAmt, uint256 debtAmt, address to);
        event LogAbsorb(uint256 colAmount, uint256 debtAmount);
        function liquidate(uint256 debtAmt, uint256 colPerUnitDebt, address to, bool absorb) external payable returns (uint256, uint256);
    }

    /// A Fluid DEX pool (`DexT1`): `Swap` carries no indexed fields (verified on mainnet pools, 2026-10-12).
    interface Dex {
        event Swap(bool swap0to1, uint256 amountIn, uint256 amountOut, address to);
    }
}

/// The vaults configured in `config/arbitrum.toml` (`[fluid] vaults`).
pub const VAULTS: [(Address, &str); 2] = [
    (
        address!("f55B8e9F0c51Ace009f4b41d03321675d4C643b3"),
        "Fluid vault (ETH collateral / ARB debt)",
    ),
    (
        address!("82b27fa821419f5689381b565a8b0786aa2548de"),
        "Fluid vault (WBTC collateral / USDT0 debt)",
    ),
];

/// A vault address as a 32-byte market id (left-padded), the way `LiquidationIntent::market` is typed.
pub fn market_id(vault: Address) -> B256 {
    let mut b = [0u8; 32];
    b[12..].copy_from_slice(vault.as_slice());
    B256::from(b)
}

pub struct Fluid;

impl Protocol for Fluid {
    fn id(&self) -> &'static str {
        "fluid_vaults"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(
                Vault::LogLiquidate::SIGNATURE_HASH,
                "liquidation_fluid",
                Class::Liquidation,
                p,
            ),
            EventKind::new(
                Vault::LogAbsorb::SIGNATURE_HASH,
                "absorb_fluid",
                Class::Liquidation,
                p,
            ),
            EventKind::new(Dex::Swap::SIGNATURE_HASH, "swap_fluid_dex", Class::Swap, p),
        ]
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, args) = abi::split(input)?;
        // Any contract answering to this exact selector: the vault list is ~100 and grows, so it is not hard-coded.
        if sel != Vault::liquidateCall::SELECTOR {
            return None;
        }
        let c = Vault::liquidateCall::abi_decode_raw(args).ok()?;
        Some(Decoded::Liquidation(LiquidationIntent {
            protocol: self.id(),
            borrowers: Vec::new(),
            collateral: None,
            debt: None,
            // Debt the caller offers to repay; `U256::MAX` style values mean "as much as the range holds".
            amount: Some(c.debtAmt).filter(|a| *a != U256::ZERO),
            market: Some(market_id(to)),
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        VAULTS
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
    fn liquidate_on_a_configured_vault_names_the_vault_and_the_debt_offered() {
        let c = Vault::liquidateCall {
            debtAmt: U256::from(1_000_000u64),
            colPerUnitDebt: U256::from(7u8),
            to: Address::with_last_byte(9),
            absorb: true,
        };
        let input = c.abi_encode();
        let Some(Decoded::Liquidation(l)) = Fluid.decode_call(VAULTS[0].0, &input) else {
            panic!("not decoded")
        };
        assert_eq!(l.protocol, "fluid_vaults");
        assert_eq!(l.amount, Some(U256::from(1_000_000u64)));
        assert_eq!(l.market, Some(market_id(VAULTS[0].0)));
        assert!(
            l.borrowers.is_empty(),
            "Fluid liquidates a price range, not an account"
        );
        assert!(
            Fluid
                .decode_call(Address::with_last_byte(5), &input[..4])
                .is_none(),
            "selector alone, no arguments"
        );
        assert!(
            Fluid.decode_call(VAULTS[1].0, &input[..30]).is_none(),
            "truncated"
        );
        let zero = Vault::liquidateCall {
            debtAmt: U256::ZERO,
            colPerUnitDebt: U256::ZERO,
            to: Address::ZERO,
            absorb: false,
        };
        let Some(Decoded::Liquidation(z)) = Fluid.decode_call(VAULTS[1].0, &zero.abi_encode())
        else {
            panic!()
        };
        assert_eq!(z.amount, None, "a zero amount is not an amount");
    }

    #[test]
    fn the_event_topics_are_the_ones_the_vaults_emit() {
        // Topic prefixes seen on chain in the two vaults' logs (LogLiquidate with 4 data words, LogAbsorb with 2).
        let ev = Fluid.events();
        let topic = |n: &str| format!("{:x}", ev.iter().find(|k| k.name == n).unwrap().topic0);
        assert!(topic("liquidation_fluid").starts_with("80fd9cc6b1"));
        assert!(topic("absorb_fluid").starts_with("115609402b"));
        assert!(
            ev.iter()
                .filter(|k| k.name != "swap_fluid_dex")
                .all(|k| k.class == Class::Liquidation)
        );
        assert_eq!(
            ev.iter()
                .find(|k| k.name == "swap_fluid_dex")
                .unwrap()
                .class,
            Class::Swap
        );
    }

    #[test]
    fn the_selector_is_fluids_liquidate() {
        let c = Vault::liquidateCall {
            debtAmt: U256::from(1u8),
            colPerUnitDebt: U256::ZERO,
            to: Address::ZERO,
            absorb: false,
        };
        assert_eq!(c.abi_encode()[..4], Vault::liquidateCall::SELECTOR);
    }
}
