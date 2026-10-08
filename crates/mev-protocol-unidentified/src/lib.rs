//! Two contracts we have not identified (owner, deployer, source unknown). Their layouts were read from landed
//! transactions and checked against the `Swap` events they emitted (measured 2026-10-08, DECISIONS #89): facts about
//! contracts we do not control, which may change without notice. Kept apart so they can be renamed or dropped once
//! identified. Both run one exact-input swap on one Uniswap-V3-style pool with a price limit.

pub mod packed;
pub mod packed_table;

use alloy::primitives::{Address, address};
use mev_protocol::{Contract, Decoded, PoolRef, Protocol, SwapIntent, abi};

/// Selector `0xa00597a0`: `(amountIn, sqrtPriceLimitX96, tokenIn, flag, pool, deadline, ..)`.
pub const ROUTER_A: Address = address!("278d858f05b94576C1E6f73285886876ff6eF8D2");
/// Selector `0x000000e7`: `(pool, 0, amountIn, sqrtPriceLimitX96, tokenIn)`.
pub const ROUTER_B: Address = address!("96DaA0B8a5499EA9323421ED0Cda06B345caab73");
/// Router B's first word usually is the pool, but three wrapper contracts appear there instead; all three swap on the
/// same pool, `0x360e68fa...`. Measured on chain (2026-10-08): 49 successful calls, every one emitted its `Swap` on that
/// pool and on no other, none from the wrapper itself (`scripts/verify-decoder.py`). Resolved here so a decoded intent
/// names the pool whose state moves. If this mapping is ever wrong the verification script reports `pool wrong` for it.
pub const POOL_ALIASES: [(Address, Address); 3] = [
    (
        address!("2f5ea801cdb6ac9aa7fba3d288491def251b7f7e"),
        address!("360e68faccca8ca495c1b759fd9eee466db9fb32"),
    ),
    (
        address!("e8c4a103bdb3187b4f05577c115951f8826bfaaa"),
        address!("360e68faccca8ca495c1b759fd9eee466db9fb32"),
    ),
    (
        address!("af031ad1f20ad56dc9fbd83b20c0e7ed9dd57f26"),
        address!("360e68faccca8ca495c1b759fd9eee466db9fb32"),
    ),
];

/// The pool a first-word address stands for: itself, or the pool a known wrapper swaps on.
pub fn resolve_pool(a: Address) -> Address {
    POOL_ALIASES
        .iter()
        .find(|(w, _)| *w == a)
        .map_or(a, |(_, p)| *p)
}

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
                pool: PoolRef::Address(resolve_pool(abi::addr(d, 0)?)),
                amount_in: abi::uint(d, 2)?,
                sqrt_limit_x96: Some(abi::uint(d, 3)?),
                token_in: abi::addr(d, 4)?,
                hops: 1,
            }
        } else {
            return packed::decode(to, input).map(Decoded::Swap);
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
            Contract {
                address: packed::ROUTER_D,
                name: "unidentified packed-calldata bot (b2460c48, b42ebb63, b7f49f3f decoded; 6bfd6286 not)",
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
    fn a_known_wrapper_in_the_first_word_resolves_to_the_pool_it_swaps_on() {
        let (wrapper, pool) = POOL_ALIASES[0];
        let input = call(
            SELECTOR_B,
            vec![
                wa(wrapper),
                w(U256::from(1)),
                w(U256::from(0x7c122030e9042a5u64)),
                w(U256::from(5)),
                wa(WETH),
            ],
        );
        let s = swap(Unidentified.decode_call(ROUTER_B, &input));
        assert_eq!(s.pool, PoolRef::Address(pool));
        // An address that is not a known wrapper is left alone, and the table never maps a pool to itself.
        assert_eq!(resolve_pool(USDC), USDC);
        assert!(POOL_ALIASES.iter().all(|(w, p)| w != p));
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
