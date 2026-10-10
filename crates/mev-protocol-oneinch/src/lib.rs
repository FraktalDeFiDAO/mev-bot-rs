//! 1inch AggregationRouter v6 `swap(executor, desc, data)` (selector `0x07ed2379`). The call names the tokens and the
//! amount; which pools carry the swap is decided by the executor from `data`, so the intent is `PoolRef::Tokens`
//! ("every pool we hold for these tokens"), not one pool. Layout measured 2026-10-10 on 60 of 60 landed calls against
//! the transfer events: word 1 = source token, word 2 = destination token, word 5 = amount, all exact.
//!
//! Not decoded: a native-ETH source (the amount is the transaction value, which `decode_call` is not given), and the
//! other entry points (`unoswap*`, `ethUnoswap*`, `clipperSwap*`), which carry packed pool addresses.

use alloy::primitives::{Address, address};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent, abi};

pub const ROUTER_V6: Address = address!("111111125421cA6dc452d289314280a0f8842A65");
pub const SELECTOR_SWAP: [u8; 4] = [0x07, 0xed, 0x23, 0x79];
/// 1inch's stand-in for the chain's native token.
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub struct OneInch;

impl Protocol for OneInch {
    fn id(&self) -> &'static str {
        "oneinch"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, d) = abi::split(input)?;
        if to != ROUTER_V6 || sel != SELECTOR_SWAP {
            return None;
        }
        let src = abi::addr(d, 1)?;
        let dst = abi::addr(d, 2)?;
        if src == NATIVE {
            return None;
        }
        let dst = if dst == NATIVE { WETH } else { dst };
        Some(Decoded::Swap(SwapIntent {
            pool: PoolRef::Tokens {
                token_a: src,
                token_b: dst,
            },
            token_in: src,
            amount_in: abi::uint(d, 5)?,
            sqrt_limit_x96: None,
            hops: 1,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: ROUTER_V6,
            name: "1inch AggregationRouter v6 (swap decoded; unoswap and clipper not)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
    const ARB: Address = address!("912CE59144191C1204E64559FE8253a0e49E6548");

    fn word(a: U256) -> [u8; 32] {
        a.to_be_bytes::<32>()
    }
    fn wa(a: Address) -> [u8; 32] {
        let mut o = [0u8; 32];
        o[12..].copy_from_slice(a.as_slice());
        o
    }
    fn call(src: Address, dst: Address, amount: u64) -> Vec<u8> {
        let mut o = SELECTOR_SWAP.to_vec();
        for w in [
            wa(Address::with_last_byte(9)),
            wa(src),
            wa(dst),
            wa(Address::with_last_byte(7)),
            wa(Address::with_last_byte(8)),
            word(U256::from(amount)),
            word(U256::from(1)),
            word(U256::ZERO),
            word(U256::from(0x120)),
        ] {
            o.extend_from_slice(&w);
        }
        o
    }

    #[test]
    fn swap_names_tokens_and_amount() {
        let Some(Decoded::Swap(s)) = OneInch.decode_call(ROUTER_V6, &call(USDC, ARB, 19_479_342))
        else {
            panic!("expected a swap")
        };
        assert_eq!(s.token_in, USDC);
        assert_eq!(s.amount_in, U256::from(19_479_342u64));
        assert_eq!(
            s.pool,
            PoolRef::Tokens {
                token_a: USDC,
                token_b: ARB
            }
        );
    }

    #[test]
    fn native_out_means_weth_and_native_in_is_not_decoded() {
        let Some(Decoded::Swap(s)) = OneInch.decode_call(ROUTER_V6, &call(USDC, NATIVE, 5)) else {
            panic!("expected a swap")
        };
        assert_eq!(
            s.pool,
            PoolRef::Tokens {
                token_a: USDC,
                token_b: WETH
            }
        );
        assert!(
            OneInch
                .decode_call(ROUTER_V6, &call(NATIVE, USDC, 5))
                .is_none()
        );
    }

    #[test]
    fn other_contracts_and_selectors_are_ignored() {
        let ok = call(USDC, ARB, 1);
        assert!(
            OneInch
                .decode_call(Address::with_last_byte(1), &ok)
                .is_none()
        );
        let mut other = ok.clone();
        other[0] ^= 1;
        assert!(OneInch.decode_call(ROUTER_V6, &other).is_none());
        assert!(OneInch.decode_call(ROUTER_V6, &ok[..40]).is_none());
    }
}
