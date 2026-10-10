//! Two contracts we have not identified (owner, deployer, source unknown). Their layouts were read from landed
//! transactions and checked against the `Swap` events they emitted (measured 2026-10-08, DECISIONS #89): facts about
//! contracts we do not control, which may change without notice. Kept apart so they can be renamed or dropped once
//! identified. Both run one exact-input swap on one Uniswap-V3-style pool with a price limit.

pub mod aa_bot;
pub mod cycle_bot;
pub mod dag_router;
pub mod execute_trade;
pub mod meta_swap;
pub mod packed;
pub mod packed_table;
pub mod rfq_swap;
pub mod router_f;
pub mod router_h;
pub mod v2_aggregator;
pub mod v4_bots;

use alloy::primitives::{Address, address};
use mev_protocol::{Contract, Decoded, PoolRef, PoolSet, Protocol, SwapIntent, abi};

/// Selector `0xa00597a0`: `(amountIn, sqrtPriceLimitX96, tokenIn, flag, pool, deadline, ..)`.
pub const ROUTER_A: Address = address!("278d858f05b94576C1E6f73285886876ff6eF8D2");
/// Selector `0x000000e7`: `(pool, 0, amountIn, sqrtPriceLimitX96, tokenIn)`.
pub const ROUTER_B: Address = address!("96DaA0B8a5499EA9323421ED0Cda06B345caab73");
/// Router B's first word usually is a V3-style pool, but four wrapper contracts appear there instead. Each swaps on one
/// Uniswap V4 pool (the PoolManager is `0x360e68fa...`, which emitted the swap); mapped here to that pool's stand-in
/// address (the last 20 bytes of its id), which is how pools without a contract are named elsewhere. Measured on chain
/// (2026-10-08): every successful call through a wrapper emitted its V4 `Swap` for that id and no other
/// (61 + 9 + 1 + 3 calls, `scripts/verify-decoder.py`).
pub const POOL_ALIASES: [(Address, Address); 4] = [
    (
        address!("2f5ea801cdb6ac9aa7fba3d288491def251b7f7e"),
        address!("2308da953ff125b0f95015adbf89aaf579e903a8"),
    ),
    (
        address!("e8c4a103bdb3187b4f05577c115951f8826bfaaa"),
        address!("b8df2ccefad94553ec18f1c29143f0399c78f500"),
    ),
    (
        address!("af031ad1f20ad56dc9fbd83b20c0e7ed9dd57f26"),
        address!("3ea89c31b361a009f9c36a1de5c8f0afcc45bde8"),
    ),
    (
        address!("056ba06ff14a87f7bb819c284a40fb0d5f038233"),
        address!("9cc505ce20b36afffd297aef2dc17daafce3b140"),
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
/// Router A's Uniswap-V2-pair variant `0x95435ac9`: `(amountIn, w1, tokenIn, token0In, 99700, pair, deadline)` plus a
/// trailing byte. Measured 2026-10-10 on 40 of 40 landed calls against the pair's own `Swap` event: pair, input token
/// and direction exact (`token0In` = 1 when token0 goes in); `amountIn` is within 1e-11 of the amount the pair received
/// (the contract trims a few wei); `w1` is not a price limit (V2 has none), so none is reported.
pub const SELECTOR_A_V2: [u8; 4] = [0x95, 0x43, 0x5a, 0xc9];

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
                amount_out_exact: None,
            }
        } else if to == ROUTER_A && sel == SELECTOR_A_V2 {
            SwapIntent {
                amount_in: abi::uint(d, 0)?,
                sqrt_limit_x96: None,
                token_in: abi::addr(d, 2)?,
                pool: PoolRef::Address(abi::addr(d, 5)?),
                hops: 1,
                amount_out_exact: None,
            }
        } else if to == ROUTER_B && sel == SELECTOR_B {
            SwapIntent {
                pool: PoolRef::Address(resolve_pool(abi::addr(d, 0)?)),
                amount_in: abi::uint(d, 2)?,
                sqrt_limit_x96: Some(abi::uint(d, 3)?),
                token_in: abi::addr(d, 4)?,
                hops: 1,
                amount_out_exact: None,
            }
        } else if to == ROUTER_A && sel == v4_bots::SELECTOR_A_V4 {
            v4_bots::decode_a_v4(d)?
        } else if to == v4_bots::ROUTER_E {
            v4_bots::decode_e(sel, d)?
        } else if to == dag_router::ROUTER_G {
            dag_router::decode(sel, d)?
        } else if to == cycle_bot::BOT {
            cycle_bot::decode(sel, d)?
        } else if to == meta_swap::ROUTER {
            meta_swap::decode(sel, d)?
        } else if to == rfq_swap::ROUTER {
            rfq_swap::decode(sel, d)?
        } else if to == execute_trade::BOT {
            execute_trade::decode(sel, d)?
        } else if to == v2_aggregator::ROUTER {
            v2_aggregator::decode(sel, d)?
        } else if to == router_h::ROUTER_H {
            router_h::decode(sel, d)?
        } else if to == router_f::ROUTER_F {
            router_f::decode(sel, d)?
        } else {
            return packed::decode(to, input).map(Decoded::Swap);
        };
        Some(Decoded::Swap(swap))
    }

    fn decode_calls(&self, to: Address, input: &[u8]) -> Vec<Decoded> {
        let mut out: Vec<Decoded> = self.decode_call(to, input).into_iter().collect();
        // Bots that name every pool of a cycle: the exact first hop (where known) plus the set, which a rival's pending
        // transaction would move. The sets are not exact (see each module for the measured precision and recall).
        if let Some((sel, d)) = abi::split(input) {
            let set = if to == cycle_bot::BOT {
                cycle_bot::pools(sel, d)
            } else if to == aa_bot::BOT {
                aa_bot::pools(sel, d)
            } else if to == router_f::ROUTER_F {
                router_f::pools(sel, d)
            } else {
                None
            };
            if let Some(pools) = set {
                out.push(Decoded::Pools(PoolSet {
                    protocol: "unidentified",
                    pools,
                }));
            }
        }
        out
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![
            Contract {
                address: ROUTER_A,
                name: "unidentified, selectors a00597a0 (V3), 95435ac9 (V2) and 669dc5b6 (V4)",
                decoded: true,
            },
            Contract {
                address: ROUTER_B,
                name: "unidentified, selector 000000e7",
                decoded: true,
            },
            Contract {
                address: v4_bots::ROUTER_E,
                name: "unidentified, selectors 755f317c (V3) and 4fd7c9b7 (V4)",
                decoded: true,
            },
            Contract {
                address: dag_router::ROUTER_G,
                name: "unidentified aggregator, dagSwapByOrderId (f2c42696) decoded to its tokens",
                decoded: true,
            },
            Contract {
                address: aa_bot::BOT,
                name: "unidentified packed MEV bot (7ef88937): the pools it declares; no exact swap",
                decoded: true,
            },
            Contract {
                address: cycle_bot::BOT,
                name: "unidentified cycle-arbitrage bot (c1c0e9c4): first pool, token and amount of the base layout",
                decoded: true,
            },
            Contract {
                address: meta_swap::ROUTER,
                name: "meta-aggregator router (5f575529): tokens and amount; the aggregator's own call is not read",
                decoded: true,
            },
            Contract {
                address: rfq_swap::ROUTER,
                name: "unidentified quote-based swap router (f3efe779): tokens and amount",
                decoded: true,
            },
            Contract {
                address: execute_trade::BOT,
                name: "unidentified arbitrage bot, executeTrade (b4b9c57b): pair, fee, token and amount",
                decoded: true,
            },
            Contract {
                address: v2_aggregator::ROUTER,
                name: "unidentified aggregator, exactInputV2Swap (0dc4bdae) and exactInputV3Swap (b9b5149b): token, amount and first pool",
                decoded: true,
            },
            Contract {
                address: router_h::ROUTER_H,
                name: "unidentified packed bot, selector 00000001 (one pool, token and amount decoded; other legs not)",
                decoded: true,
            },
            Contract {
                address: router_f::ROUTER_F,
                name: "unidentified packed bot (00000002 V3, 00000011 V4, 00000001 V2 legs decoded; other 00000001 shapes not)",
                decoded: true,
            },
            Contract {
                address: packed::ROUTER_D,
                name: "unidentified packed-calldata bot (b2460c48, b42ebb63, b7f49f3f on V3 pools; 6bfd6286 on V4 pools)",
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
    fn router_a_v2_variant_names_the_pair_token_and_amount() {
        // Landed call 0x7f20c845...: 15.2 token swapped on a V2 pair; the pair's Swap event agreed on all three.
        let pair = address!("b7e50106a5bd3cf21af210a755f9c8740890a8c9");
        let tok = address!("539bde0d7dbd336b79148aa742883198bbf60342");
        let input = call(
            SOME_V2,
            vec![
                w(U256::from(0xd25bb5c30d8a9b560u128)),
                w(U256::from(0x1cc9f0a45aaebabffa7d16au128)),
                wa(tok),
                w(U256::from(1)),
                w(U256::from(99_700u64)),
                wa(pair),
                w(U256::from(0x6ac980b9u64)),
                vec![0x0f],
            ],
        );
        let s = swap(Unidentified.decode_call(ROUTER_A, &input));
        assert_eq!(s.pool, PoolRef::Address(pair));
        assert_eq!(s.token_in, tok);
        assert_eq!(s.amount_in, U256::from(0xd25bb5c30d8a9b560u128));
        assert_eq!(s.sqrt_limit_x96, None);
        // Only router A answers to this selector.
        assert!(Unidentified.decode_call(ROUTER_B, &input).is_none());
    }

    const SOME_V2: [u8; 4] = SELECTOR_A_V2;

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
