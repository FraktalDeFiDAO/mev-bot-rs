//! Camelot's Algebra (V3) `SwapRouter`, `exactInputSingle((tokenIn, tokenOut, recipient, deadline, amountIn,
//! amountOutMinimum, limitSqrtPrice))` (selector `0xbc651188`). Algebra has no fee tier (one pool per pair, fee
//! dynamic), so the pool is named by its tokens, `PoolRef::Tokens`. Measured 2026-10-10: tokens and amount exact on 40
//! of 40 landed calls against the transfer logs. `limitSqrtPrice` is reported as the price limit when non-zero.
//!
//! Not decoded: `exactInput` (path), exact-output calls, and `multicall` wrappers.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent, abi};

pub const ROUTER: Address = address!("1F721E2E82F6676FCE4eA07A5958cF098D339e18");
pub const SELECTOR_EXACT_INPUT_SINGLE: [u8; 4] = [0xbc, 0x65, 0x11, 0x88];

pub struct CamelotRouter;

impl Protocol for CamelotRouter {
    fn id(&self) -> &'static str {
        "camelot_router"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, d) = abi::split(input)?;
        if to != ROUTER || sel != SELECTOR_EXACT_INPUT_SINGLE {
            return None;
        }
        let (token_in, token_out) = (abi::addr(d, 0)?, abi::addr(d, 1)?);
        let amount_in = abi::uint(d, 4)?;
        if token_in == token_out || amount_in.is_zero() {
            return None;
        }
        Some(Decoded::Swap(SwapIntent {
            pool: PoolRef::Tokens {
                token_a: token_in,
                token_b: token_out,
            },
            token_in,
            amount_in,
            sqrt_limit_x96: Some(abi::uint(d, 6)?).filter(|l: &U256| !l.is_zero()),
            hops: 1,
            amount_out_exact: None,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: ROUTER,
            name: "Camelot V3 SwapRouter (exactInputSingle decoded)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn landed_calls_decode_to_their_tokens_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/exact_input_single.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let Some(Decoded::Swap(s)) = CamelotRouter.decode_call(ROUTER, &input) else {
                panic!("not decoded: {l}")
            };
            assert_eq!(s.token_in, f[1].parse::<Address>().unwrap());
            assert_eq!(
                s.pool,
                PoolRef::Tokens {
                    token_a: f[1].parse().unwrap(),
                    token_b: f[2].parse().unwrap()
                }
            );
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap());
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn other_contracts_selectors_and_short_calls_are_ignored() {
        let l = include_str!("../tests/fixtures/exact_input_single.csv")
            .lines()
            .nth(1)
            .unwrap();
        let input = hex::decode(l.split(',').next().unwrap().trim_start_matches("0x")).unwrap();
        assert!(
            CamelotRouter
                .decode_call(Address::with_last_byte(1), &input)
                .is_none()
        );
        assert!(CamelotRouter.decode_call(ROUTER, &input[..60]).is_none());
        let mut other = input;
        other[3] ^= 1;
        assert!(CamelotRouter.decode_call(ROUTER, &other).is_none());
    }
}
