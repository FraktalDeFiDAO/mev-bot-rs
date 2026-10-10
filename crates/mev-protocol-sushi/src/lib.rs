//! Sushi's multi-swap router `snwapMultiple((tokenIn, amountIn, recipient)[] inputs, (tokenOut, recipient,
//! minAmount)[] outputs, (target, value, data)[] executors)` (selector `0xd33721a5`). Inputs and outputs name the
//! tokens and amounts; the executors (arbitrary calls) pick the pools, so each (input, output) pair is one intent on
//! `PoolRef::Tokens`. Measured 2026-10-10: input tokens, input amounts and output tokens exact on 40 of 40 landed
//! calls against the transfer logs. Native ETH (`0xEeee..`) as an input has no amount in the call and is skipped; as an
//! output it means WETH.

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::SolCall,
};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent};

sol! {
    struct InputToken { address token; uint256 amountIn; address recipient; }
    struct OutputToken { address token; address recipient; uint256 amountOutMin; }
    struct Executor { address target; uint256 value; bytes data; }
    function snwapMultiple(InputToken[] inputTokens, OutputToken[] outputTokens, Executor[] executors) external payable;
}

pub const ROUTER: Address = address!("ac4c6e212a361c968f1725b4d055b47e63f80b75");
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub struct SushiRouter;

impl Protocol for SushiRouter {
    fn id(&self) -> &'static str {
        "sushi_router"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        self.decode_calls(to, input).into_iter().next()
    }

    fn decode_calls(&self, to: Address, input: &[u8]) -> Vec<Decoded> {
        if to != ROUTER {
            return Vec::new();
        }
        let Ok(c) = snwapMultipleCall::abi_decode(input) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in &c.inputTokens {
            if i.token == NATIVE || i.amountIn == U256::ZERO {
                continue;
            }
            for o in &c.outputTokens {
                let dst = if o.token == NATIVE { WETH } else { o.token };
                if dst != i.token {
                    out.push(Decoded::Swap(SwapIntent {
                        pool: PoolRef::Tokens {
                            token_a: i.token,
                            token_b: dst,
                        },
                        token_in: i.token,
                        amount_in: i.amountIn,
                        sqrt_limit_x96: None,
                        hops: 1,
                    }));
                }
            }
        }
        out
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: ROUTER,
            name: "Sushi router (snwapMultiple: one token-pair intent per input and output)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn landed_calls_decode_to_the_token_pairs_and_amounts_the_chain_confirmed() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/snwap_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, expected) = l.split_once(',').unwrap();
            let input = hex::decode(input.trim_start_matches("0x")).unwrap();
            let got: Vec<String> = SushiRouter
                .decode_calls(ROUTER, &input)
                .into_iter()
                .map(|d| match d {
                    Decoded::Swap(s) => {
                        let PoolRef::Tokens { token_b, .. } = s.pool else {
                            panic!()
                        };
                        format!("{:#x}/{:#x}/{}", s.token_in, token_b, s.amount_in)
                    }
                    other => panic!("{other:?}"),
                })
                .collect();
            let want: Vec<&str> = expected.split(';').filter(|e| !e.is_empty()).collect();
            assert_eq!(got, want, "{l}");
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn other_contracts_and_garbage_decode_to_nothing() {
        assert!(
            SushiRouter
                .decode_calls(Address::with_last_byte(1), &[0xd3, 0x37])
                .is_empty()
        );
        assert!(
            SushiRouter
                .decode_calls(ROUTER, &[0xd3, 0x37, 0x21, 0xa5, 1])
                .is_empty()
        );
        assert!(SushiRouter.decode_call(ROUTER, b"").is_none());
    }
}
