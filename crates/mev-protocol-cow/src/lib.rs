//! CoW Protocol `GPv2Settlement.settle(tokens, clearingPrices, trades, interactions)` (selector `0x13d79a0b`).
//! A batch auction: each trade sells one listed token for another at a uniform clearing price, matched against the
//! other trades or against pools through the `interactions` (arbitrary calls, not read here). So each trade is one
//! intent on `PoolRef::Tokens`, an over-approximation of the pools that settle. Measured 2026-10-10: the settlement's
//! token list was a subset of the tokens that moved in 38 of 39 landed calls.
//!
//! `amount_in` is the trade's executed amount for a sell order and its sell limit otherwise: a bound, not the fill.

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::SolCall,
};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent};

sol! {
    struct Trade {
        uint256 sellTokenIndex; uint256 buyTokenIndex; address receiver; uint256 sellAmount; uint256 buyAmount;
        uint32 validTo; bytes32 appData; uint256 feeAmount; uint256 flags; uint256 executedAmount; bytes signature;
    }
    struct Interaction { address target; uint256 value; bytes callData; }
    function settle(address[] tokens, uint256[] clearingPrices, Trade[] trades, Interaction[][3] interactions) external;
}

pub const SETTLEMENT: Address = address!("9008D19f58AAbD9eD0D60971565AA8510560ab41");

pub struct Cow;

fn intents(input: &[u8]) -> Vec<SwapIntent> {
    let Ok(call) = settleCall::abi_decode(input) else {
        return Vec::new();
    };
    call.trades
        .iter()
        .filter_map(|t| {
            let sell = *call.tokens.get(usize::try_from(t.sellTokenIndex).ok()?)?;
            let buy = *call.tokens.get(usize::try_from(t.buyTokenIndex).ok()?)?;
            // flags bit 0: 0 = sell order (executedAmount is the sold amount), 1 = buy order.
            let sell_order = t.flags & U256::from(1) == U256::ZERO;
            let amount_in = if sell_order && t.executedAmount > U256::ZERO {
                t.executedAmount
            } else {
                t.sellAmount
            };
            (sell != buy && amount_in > U256::ZERO).then_some(SwapIntent {
                pool: PoolRef::Tokens {
                    token_a: sell,
                    token_b: buy,
                },
                token_in: sell,
                amount_in,
                sqrt_limit_x96: None,
                hops: 1,
            })
        })
        .collect()
}

impl Protocol for Cow {
    fn id(&self) -> &'static str {
        "cow"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        self.decode_calls(to, input).into_iter().next()
    }

    fn decode_calls(&self, to: Address, input: &[u8]) -> Vec<Decoded> {
        if to != SETTLEMENT {
            return Vec::new();
        }
        intents(input).into_iter().map(Decoded::Swap).collect()
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: SETTLEMENT,
            name: "CoW Protocol settlement (settle: one token-pair intent per trade; interactions not read)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLE: &str = include_str!("../tests/fixtures/settle.hex");

    #[test]
    fn a_landed_settlement_yields_one_intent_per_trade() {
        // 0xdc1376a0...: USDC -> 0xcb8b... (31.382679 USDC) and 0xcb8b... -> USDC (218.236... units).
        let input = alloy::hex::decode(SETTLE.trim()).unwrap();
        let got = Cow.decode_calls(SETTLEMENT, &input);
        assert_eq!(got.len(), 2);
        let Decoded::Swap(a) = &got[0] else { panic!() };
        let usdc = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
        let other = address!("cb8b5cd20bdcaea9a010ac1f8d835824f5c87a04");
        assert_eq!(a.token_in, usdc);
        assert_eq!(a.amount_in, U256::from(31_382_679u64));
        assert_eq!(
            a.pool,
            PoolRef::Tokens {
                token_a: usdc,
                token_b: other
            }
        );
        let Decoded::Swap(b) = &got[1] else { panic!() };
        assert_eq!(b.token_in, other);
        assert_eq!(b.amount_in, U256::from(218_236_199_960_686_542_050u128));
    }

    #[test]
    fn other_contracts_and_garbage_decode_to_nothing() {
        let input = alloy::hex::decode(SETTLE.trim()).unwrap();
        assert!(
            Cow.decode_calls(Address::with_last_byte(1), &input)
                .is_empty()
        );
        assert!(Cow.decode_calls(SETTLEMENT, &input[..100]).is_empty());
        assert!(Cow.decode_call(SETTLEMENT, b"").is_none());
    }
}
