//! Paraswap Augustus v5 (`0xdef171fe...`). Each entry point takes one dynamic struct (ABI word 0 is its offset)
//! that starts with `fromToken`, `toToken`; the sold amount is element 2 for `simpleSwap(0x54e3f31b)` and element 3
//! for `directUniV3Swap(0xa6886da9)`. The route is in the struct's callee data, so the intent is `PoolRef::Tokens`.
//! Measured 2026-10-10: tokens and amount exact on 40 of 40 `simpleSwap` and 30 of 30 `directUniV3Swap` landed calls
//! (the transfer logs, or the transaction value when the source is native). Native ETH (`0xEeee..`) maps to WETH.
//!
//! Not decoded: `multiSwap`, `megaSwap`, `directCurveV1Swap` and the other direct swaps, whose structs differ.

use alloy::primitives::{Address, address};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent, abi};

pub const AUGUSTUS: Address = address!("def171fe48cf0115b1d80b88dc8eab59176fee57");
/// Augustus V6.2: `swapExactAmountIn(executor, (srcToken, destToken, fromAmount, toAmount, quotedAmount, metadata,
/// beneficiary), partnerAndFee, permit, executorData)`; the data tuple is static, so its fields are words 1..8. Measured
/// 2026-10-09 on 37 of 37 landed calls: tokens and `fromAmount` exact (Transfer logs, or the value for native).
pub const AUGUSTUS_V6: Address = address!("6a000f20005980200259b80c5102003040001068");
pub const SWAP_EXACT_AMOUNT_IN: [u8; 4] = [0xe3, 0xea, 0xd5, 0x9e];
pub const SIMPLE_SWAP: [u8; 4] = [0x54, 0xe3, 0xf3, 0x1b];
pub const DIRECT_UNI_V3_SWAP: [u8; 4] = [0xa6, 0x88, 0x6d, 0xa9];
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub struct Paraswap;

impl Protocol for Paraswap {
    fn id(&self) -> &'static str {
        "paraswap"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let (sel, d) = abi::split(input)?;
        if to == AUGUSTUS_V6 && sel == SWAP_EXACT_AMOUNT_IN {
            let wrap = |a: Address| if a == NATIVE { WETH } else { a };
            let (src, dst) = (wrap(abi::addr(d, 1)?), wrap(abi::addr(d, 2)?));
            let amount = abi::uint(d, 3)?;
            return (src != dst && !amount.is_zero()).then_some(Decoded::Swap(SwapIntent {
                pool: PoolRef::Tokens {
                    token_a: src,
                    token_b: dst,
                },
                token_in: src,
                amount_in: amount,
                sqrt_limit_x96: None,
                hops: 1,
                amount_out_exact: None,
            }));
        }
        if to != AUGUSTUS {
            return None;
        }
        let amount_element = if sel == SIMPLE_SWAP {
            2
        } else if sel == DIRECT_UNI_V3_SWAP {
            3
        } else {
            return None;
        };
        let t = usize::try_from(abi::uint(d, 0)?).ok()? / 32;
        let wrap = |a: Address| if a == NATIVE { WETH } else { a };
        let (src, dst) = (wrap(abi::addr(d, t)?), wrap(abi::addr(d, t + 1)?));
        let amount = abi::uint(d, t + amount_element)?;
        (src != dst && !amount.is_zero()).then_some(Decoded::Swap(SwapIntent {
            pool: PoolRef::Tokens {
                token_a: src,
                token_b: dst,
            },
            token_in: src,
            amount_in: amount,
            sqrt_limit_x96: None,
            hops: 1,
            amount_out_exact: None,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![
            Contract {
                address: AUGUSTUS,
                name: "Paraswap Augustus v5 (simpleSwap and directUniV3Swap decoded; multiSwap and others not)",
                decoded: true,
            },
            Contract {
                address: AUGUSTUS_V6,
                name: "Paraswap Augustus v6.2 (swapExactAmountIn decoded)",
                decoded: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, hex};

    #[test]
    fn landed_calls_decode_to_their_tokens_and_amount() {
        let (mut n, mut native, mut wraps) = (0, 0, 0);
        for l in include_str!("../tests/fixtures/swap_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[1].trim_start_matches("0x")).unwrap();
            let src: Address = f[2].parse().unwrap();
            let want_in = if src == NATIVE { WETH } else { src };
            let dst: Address = f[3].parse().unwrap();
            if want_in == if dst == NATIVE { WETH } else { dst } {
                // ETH <-> WETH is a wrap, not a swap: no pool is touched.
                assert!(Paraswap.decode_call(AUGUSTUS, &input).is_none(), "{l}");
                wraps += 1;
                continue;
            }
            let Some(Decoded::Swap(s)) = Paraswap.decode_call(AUGUSTUS, &input) else {
                panic!("not decoded: {l}")
            };
            assert_eq!(s.token_in, want_in, "{l}");
            assert_eq!(s.amount_in, f[4].parse::<U256>().unwrap(), "{l}");
            native += usize::from(src == NATIVE);
            n += 1;
        }
        assert!(n >= 55 && native >= 8, "{n} {native} {wraps}");
    }

    #[test]
    fn landed_v6_calls_decode_to_their_tokens_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/v6_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let wrap = |a: Address| if a == NATIVE { WETH } else { a };
            let (src, dst): (Address, Address) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            if wrap(src) == wrap(dst) {
                assert!(Paraswap.decode_call(AUGUSTUS_V6, &input).is_none(), "{l}");
                continue;
            }
            let Some(Decoded::Swap(s)) = Paraswap.decode_call(AUGUSTUS_V6, &input) else {
                panic!("not decoded: {l}")
            };
            assert_eq!(s.token_in, wrap(src), "{l}");
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 30, "{n}");
    }

    #[test]
    fn other_contracts_selectors_and_garbage_are_ignored() {
        let l = include_str!("../tests/fixtures/swap_rows.csv")
            .lines()
            .nth(1)
            .unwrap();
        let input = hex::decode(l.split(',').nth(1).unwrap().trim_start_matches("0x")).unwrap();
        assert!(
            Paraswap
                .decode_call(Address::with_last_byte(1), &input)
                .is_none()
        );
        let mut other = input.clone();
        other[3] ^= 1;
        assert!(Paraswap.decode_call(AUGUSTUS, &other).is_none());
        assert!(Paraswap.decode_call(AUGUSTUS, &input[..40]).is_none());
        assert!(Paraswap.decode_call(AUGUSTUS, b"").is_none());
    }
}
