//! Router F (`0xe6b82c6e...`): a bot with packed calldata. Layouts fitted from `debug_traceTransaction` traces and
//! verified on every traced transaction (2026-10-08); fixtures in `tests/fixtures/f*_rows.csv`.
//!
//! Selector `00000002` (V3 pool), arguments after the selector, byte offsets:
//!   0..16 amountIn (uint128) | 17..37 pool | 37..57 tokenIn | 57..77 tokenOut | 121..141 sqrtPriceLimitX96.
//!   `zeroForOne` is `tokenIn < tokenOut`.
//! Selector `00000011` (V4 pool), 32-byte words: `(0x20, amountIn, zeroForOne, .., sqrtLimit at 6, .., currency0 at 12,
//!   currency1 at 13, fee at 14, tickSpacing at 15, hooks at 16)`.
//! Selector `00000001` (V2 pair legs), arguments after the selector: `[legs][wIn][wOut][amountIn: wIn bytes][minOut:
//!   wOut bytes]` (wIn + wOut = 16) `[tokenOut: 20][deadline: 8][pool: 20][fee: 3][tokenIn: 20]..`, the first leg's fields.
//!   Decoded for widths (9,7) with one or two legs and (7,9) with one leg; verified on 56 of 56 successful traced calls
//!   (first pair swap = pool, first transfer = tokenIn and amountIn, 2026-10-10, `tests/fixtures/f1_rows.csv`). Other
//!   width pairs lead with a V3 leg or order their legs differently and are not decoded.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi, v4};

use crate::v4_bots::{Key, non_zero, small, v4_swap};

pub const ROUTER_F: Address = address!("e6b82c6eccdfb2d6bc55bb11a61bae5ab2c3f807");
pub const SELECTOR_V3: [u8; 4] = [0, 0, 0, 2];
pub const SELECTOR_V4: [u8; 4] = [0, 0, 0, 0x11];
pub const SELECTOR_V2: [u8; 4] = [0, 0, 0, 1];

fn address_at(b: &[u8], off: usize) -> Option<Address> {
    b.get(off..off + 20).map(Address::from_slice)
}

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel == SELECTOR_V3 {
        // Fixed-size argument; refuse anything else rather than read the wrong offsets.
        if d.len() != 217 {
            return None;
        }
        let amount = U256::from_be_slice(d.get(0..16)?);
        let (token_in, token_out) = (address_at(d, 37)?, address_at(d, 57)?);
        if amount.is_zero() || token_in == token_out {
            return None;
        }
        return Some(SwapIntent {
            pool: PoolRef::Address(address_at(d, 17)?),
            token_in,
            amount_in: amount,
            sqrt_limit_x96: non_zero(U256::from_be_slice(d.get(121..141)?)),
            hops: 1,
        });
    }
    if sel == SELECTOR_V2 {
        let (legs, w_in, w_out) = (
            *d.first()?,
            usize::from(*d.get(1)?),
            usize::from(*d.get(2)?),
        );
        let known = matches!((legs, w_in, w_out), (1 | 2, 9, 7) | (1, 7, 9));
        // One leg is 93 bytes; a second leg adds 46.
        if !known || d.len() != 93 + 46 * usize::from(legs - 1) {
            return None;
        }
        let amount = U256::from_be_slice(d.get(3..3 + w_in)?);
        if amount.is_zero() {
            return None;
        }
        return Some(SwapIntent {
            pool: PoolRef::Address(address_at(d, 47)?),
            token_in: address_at(d, 70)?,
            amount_in: amount,
            sqrt_limit_x96: None,
            hops: legs,
        });
    }
    if sel == SELECTOR_V4 {
        if d.len() != 576 || abi::uint(d, 0)? != U256::from(0x20u8) {
            return None;
        }
        let zero_for_one = match abi::uint(d, 2)? {
            v if v.is_zero() => false,
            v if v == U256::from(1u8) => true,
            _ => return None,
        };
        let key = Key {
            currency0: abi::addr(d, 12)?,
            currency1: abi::addr(d, 13)?,
            fee: small(d, 14)?,
            tick_spacing: small(d, 15)?,
            hooks: abi::addr(d, 16)?,
        };
        let _ = v4::MANAGER; // the manager is not in this calldata; the key alone names the pool
        return v4_swap(key, zero_for_one, abi::uint(d, 1)?, abi::uint(d, 6)?);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    fn rows(csv: &'static str) -> impl Iterator<Item = Vec<&'static str>> {
        csv.lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|l| l.split(',').collect())
    }

    #[test]
    fn v3_swaps_decode_to_the_traced_pool_direction_amount_and_limit() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/f2_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s =
                decode(SELECTOR_V3, &input[4..]).unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            assert_eq!(s.pool, PoolRef::Address(f[1].parse().unwrap()), "{}", f[0]);
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{}", f[0]);
            assert_eq!(
                s.sqrt_limit_x96,
                Some(f[4].parse::<U256>().unwrap()),
                "{}",
                f[0]
            );
            // Direction: the input token sorts below the output token exactly when the swap is zeroForOne.
            let d = &input[4..];
            assert_eq!(
                address_at(d, 37) < address_at(d, 57),
                f[2] == "1",
                "{}",
                f[0]
            );
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn v2_legs_decode_to_the_traced_pool_token_and_amount() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/f1_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s =
                decode(SELECTOR_V2, &input[4..]).unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            assert_eq!(s.pool, PoolRef::Address(f[1].parse().unwrap()), "{}", f[0]);
            assert_eq!(s.token_in, f[2].parse::<Address>().unwrap(), "{}", f[0]);
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{}", f[0]);
            assert_eq!(s.hops, f[4].parse::<u8>().unwrap(), "{}", f[0]);
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn v2_legs_of_other_shapes_are_not_decoded() {
        let ok = hex::decode(include_str!("../tests/fixtures/f1_one_leg.hex").trim()).unwrap();
        assert!(decode(SELECTOR_V2, &ok).is_some());
        let mut v3_first = ok.clone();
        v3_first[1] = 4; // width pair (4, 7): not a known shape
        assert!(decode(SELECTOR_V2, &v3_first).is_none());
        assert!(decode(SELECTOR_V2, &ok[..92]).is_none());
        let mut zero = ok;
        zero[3..12].fill(0);
        assert!(decode(SELECTOR_V2, &zero).is_none());
    }

    #[test]
    fn v4_swaps_decode_to_the_traced_pool_key_direction_amount_and_limit() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/f11_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s =
                decode(SELECTOR_V4, &input[4..]).unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            let (c0, c1): (Address, Address) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            let id = v4::pool_id(
                c0,
                c1,
                f[3].parse().unwrap(),
                f[4].parse().unwrap(),
                f[5].parse().unwrap(),
            );
            assert_eq!(s.pool, PoolRef::Address(v4::stand_in(id)), "{}", f[0]);
            assert_eq!(s.token_in, if f[6] == "1" { c0 } else { c1 }, "{}", f[0]);
            assert_eq!(s.amount_in, f[7].parse::<U256>().unwrap(), "{}", f[0]);
            assert_eq!(
                s.sqrt_limit_x96,
                Some(f[8].parse::<U256>().unwrap()),
                "{}",
                f[0]
            );
            n += 1;
        }
        assert!(n >= 20, "{n}");
    }

    #[test]
    fn other_shapes_are_not_decoded() {
        let f = rows(include_str!("../tests/fixtures/f2_rows.csv"))
            .next()
            .unwrap();
        let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
        let d = &input[4..];
        assert!(decode(SELECTOR_V3, d).is_some());
        assert!(decode(SELECTOR_V3, &d[..216]).is_none(), "truncated");
        assert!(
            decode(SELECTOR_V4, d).is_none(),
            "the V4 shape needs its own length"
        );
        assert!(decode([0, 0, 0, 1], d).is_none(), "the multi-leg selector");
        let mut zero = d.to_vec();
        zero[..16].fill(0);
        assert!(decode(SELECTOR_V3, &zero).is_none(), "a zero amount");
        let g = rows(include_str!("../tests/fixtures/f11_rows.csv"))
            .next()
            .unwrap();
        let gi = hex::decode(g[0].trim_start_matches("0x")).unwrap();
        let mut bad_flag = gi[4..].to_vec();
        bad_flag[2 * 32 + 31] = 7;
        assert!(
            decode(SELECTOR_V4, &bad_flag).is_none(),
            "direction must be 0 or 1"
        );
    }
}
