//! Two contracts we have not identified (owner, deployer, source unknown). Their layouts were read from landed
//! transactions and checked against the `Swap` events they emitted (measured 2026-10-08, DECISIONS #89): facts about
//! contracts we do not control, which may change without notice. Kept apart so they can be renamed or dropped once
//! identified. Both run one exact-input swap on one Uniswap-V3-style pool with a price limit.

use alloy::primitives::{Address, address};
use mev_protocol::{Contract, Decoded, PoolRef, Protocol, SwapIntent, abi};

/// Selector `0xa00597a0`: `(amountIn, sqrtPriceLimitX96, tokenIn, flag, pool, deadline, ..)`.
pub const ROUTER_A: Address = address!("278d858f05b94576C1E6f73285886876ff6eF8D2");
/// Selector `0x000000e7`: `(pool, 0, amountIn, sqrtPriceLimitX96, tokenIn)`.
pub const ROUTER_B: Address = address!("96DaA0B8a5499EA9323421ED0Cda06B345caab73");
pub const SELECTOR_A: [u8; 4] = [0xa0, 0x05, 0x97, 0xa0];
pub const SELECTOR_B: [u8; 4] = [0x00, 0x00, 0x00, 0xe7];

pub struct Unidentified;

impl Protocol for Unidentified {
    fn id(&self) -> &'static str {
        "unidentified"
    }

    fn events(&self) -> Vec<mev_protocol::EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, d) = abi::split(input)?;
        let swap = if to == ROUTER_A && sel == SELECTOR_A {
            SwapIntent {
                amount_in: abi::uint(d, 0)?,
                sqrt_limit_x96: Some(abi::uint(d, 1)?),
                token_in: abi::addr(d, 2)?,
                pool: PoolRef::Address(abi::addr(d, 4)?),
                hops: 1,
            }
        } else if to == ROUTER_B && sel == SELECTOR_B {
            SwapIntent {
                pool: PoolRef::Address(abi::addr(d, 0)?),
                amount_in: abi::uint(d, 2)?,
                sqrt_limit_x96: Some(abi::uint(d, 3)?),
                token_in: abi::addr(d, 4)?,
                hops: 1,
            }
        } else {
            return None;
        };
        Some(Decoded::Swap(swap))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![
            Contract {
                address: ROUTER_A,
                name: "unidentified, selector a00597a0",
                decoded: true,
            },
            Contract {
                address: ROUTER_B,
                name: "unidentified, selector 000000e7",
                decoded: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");

    fn w(v: U256) -> Vec<u8> {
        v.to_be_bytes::<32>().to_vec()
    }
    fn wa(a: Address) -> Vec<u8> {
        let mut o = vec![0u8; 12];
        o.extend_from_slice(a.as_slice());
        o
    }
    fn call(sel: [u8; 4], words: Vec<Vec<u8>>) -> Vec<u8> {
        let mut o = sel.to_vec();
        words.into_iter().for_each(|x| o.extend(x));
        o
    }
    fn swap(d: Option<Decoded>) -> SwapIntent {
        match d {
            Some(Decoded::Swap(s)) => s,
            other => panic!("expected a swap, got {other:?}"),
        }
    }

    #[test]
    fn router_a_reads_amount_limit_token_and_pool() {
        let pool = address!("0eb43b37539a4261aa1860f9476094bb2c00a894");
        let input = call(
            SELECTOR_A,
            vec![
                w(U256::from(0x2678e8e1de7d99u64)),
                w(U256::from(7_777u64)),
                wa(WETH),
                w(U256::from(1)),
                wa(pool),
                w(U256::from(0x6ac79943u64)),
                w(U256::from(15)),
            ],
        );
        let s = swap(Unidentified.decode_call(ROUTER_A, &input));
        assert_eq!(s.pool, PoolRef::Address(pool));
        assert_eq!(s.token_in, WETH);
        assert_eq!(s.amount_in, U256::from(0x2678e8e1de7d99u64));
        assert_eq!(s.sqrt_limit_x96, Some(U256::from(7_777u64)));
    }

    #[test]
    fn router_b_reads_pool_amount_limit_and_token() {
        let pool = address!("c6962004f452be9203591991d15f6b388e09e8d0");
        let input = call(
            SELECTOR_B,
            vec![
                wa(pool),
                w(U256::ZERO),
                w(U256::from(0xe277db25u64)),
                w(U256::from(99u64)),
                wa(USDC),
            ],
        );
        let s = swap(Unidentified.decode_call(ROUTER_B, &input));
        assert_eq!(
            (s.token_in, s.amount_in, s.sqrt_limit_x96),
            (USDC, U256::from(0xe277db25u64), Some(U256::from(99u64)))
        );
    }

    #[test]
    fn other_contracts_selectors_and_dirty_words_are_not_decoded() {
        let ok = call(
            SELECTOR_B,
            vec![
                wa(Address::ZERO),
                w(U256::ZERO),
                w(U256::from(1)),
                w(U256::from(1)),
                wa(USDC),
            ],
        );
        assert!(Unidentified.decode_call(ROUTER_B, &ok).is_some());
        assert!(
            Unidentified
                .decode_call(Address::with_last_byte(1), &ok)
                .is_none()
        );
        assert!(Unidentified.decode_call(ROUTER_A, &ok).is_none());
        assert!(Unidentified.decode_call(ROUTER_B, &ok[..40]).is_none());
        let mut dirty = ok;
        dirty[4] = 1;
        assert!(Unidentified.decode_call(ROUTER_B, &dirty).is_none());
    }
}
