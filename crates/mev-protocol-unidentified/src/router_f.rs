//! Router F (`0xe6b82c6e...`): a bot with packed calldata. Layouts fitted from `debug_traceTransaction` traces and
//! verified on every traced transaction (2026-10-08); fixtures in `tests/fixtures/f*_rows.csv`.
//!
//! Selector `00000002` (V3 pool), arguments after the selector, byte offsets:
//!   0..16 amountIn (uint128) | 17..37 pool | 37..57 tokenIn | 57..77 tokenOut | 121..141 sqrtPriceLimitX96.
//!   `zeroForOne` is `tokenIn < tokenOut`.
//! Selector `00000011` (V4 pool), 32-byte words: `(0x20, amountIn, zeroForOne, .., sqrtLimit at 6, .., currency0 at 12,
//!   currency1 at 13, fee at 14, tickSpacing at 15, hooks at 16)`.
//! Selector `00000001` (swap legs), arguments after the selector: `[legs][wIn][wOut][amountIn: wIn bytes][minOut: wOut
//!   bytes][tokenOut: 20][deadline: 8][pool: 20][fee: 3][tokenIn: 20]..`, the first leg's fields, so for any width pair
//!   the pool is at byte `31 + wIn + wOut` and the token in at `54 + wIn + wOut`. Decoded for one or two legs with widths
//!   3..=10 and length `77 + wIn + wOut + 46 * (legs - 1)`: on 84 of 84 successful traced calls (2026-10-10, fixture
//!   `tests/fixtures/f1_rows.csv`) the bot pays exactly this token and amount into this pool (the V3 `amountSpecified`
//!   or the token transfer); the pool is also the first swapped in 82 of 84, the other two run their second leg first.
//!   Other lengths (one 330-byte call) and more legs are not decoded.

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

/// Both pools of a two-leg `00000001` call (the first leg's, at `31 + wIn + wOut`, and the second 46 bytes on), for
/// the short shape only. Measured on 33 landed calls (2026-10-10): every swapped pool was named (recall 100%) and 86.4%
/// of the named pools were swapped (the second leg is sometimes not executed), so a conservative set for holding pools
/// back, not an exact list.
pub fn pools(sel: [u8; 4], d: &[u8]) -> Option<Vec<Address>> {
    if sel != SELECTOR_V2 {
        return None;
    }
    let (legs, w_in, w_out) = (
        *d.first()?,
        usize::from(*d.get(1)?),
        usize::from(*d.get(2)?),
    );
    let known = legs == 2 && (3..=10).contains(&w_in) && (3..=10).contains(&w_out);
    if !known || d.len() != 77 + w_in + w_out + 46 {
        return None;
    }
    let first = 31 + w_in + w_out;
    Some(vec![address_at(d, first)?, address_at(d, first + 46)?])
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
            amount_out_exact: None,
        });
    }
    if sel == SELECTOR_V2 {
        let (legs, w_in, w_out) = (
            *d.first()?,
            usize::from(*d.get(1)?),
            usize::from(*d.get(2)?),
        );
        // One or two legs, field widths 3..=10; the length is the fixed head plus both fields, 46 more per extra leg.
        let known =
            (1..=2).contains(&legs) && (3..=10).contains(&w_in) && (3..=10).contains(&w_out);
        if !known || d.len() != 77 + w_in + w_out + 46 * usize::from(legs - 1) {
            return None;
        }
        let amount = U256::from_be_slice(d.get(3..3 + w_in)?);
        if amount.is_zero() {
            return None;
        }
        return Some(SwapIntent {
            pool: PoolRef::Address(address_at(d, 31 + w_in + w_out)?),
            token_in: address_at(d, 54 + w_in + w_out)?,
            amount_in: amount,
            sqrt_limit_x96: None,
            hops: legs,
            amount_out_exact: None,
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
    fn the_named_pools_of_two_leg_calls_cover_every_swapped_pool() {
        let (mut named, mut swapped, mut both, mut calls) = (0usize, 0usize, 0usize, 0usize);
        for l in include_str!("../tests/fixtures/f1_pools.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, sw) = l.split_once(',').unwrap();
            let input = hex::decode(input.trim_start_matches("0x")).unwrap();
            let got = pools(SELECTOR_V2, &input[4..]).unwrap_or_else(|| panic!("no pools: {l}"));
            let want: Vec<Address> = sw.split('|').map(|a| a.parse().unwrap()).collect();
            calls += 1;
            named += got.len();
            swapped += want.len();
            both += got.iter().filter(|a| want.contains(a)).count();
        }
        // DECISIONS #162: precision 86.4% and recall 100% on the 33-call sample.
        assert!(
            both as f64 / named as f64 > 0.80,
            "precision {both}/{named}"
        );
        assert!(
            both as f64 / swapped as f64 > 0.97,
            "recall {both}/{swapped}"
        );
        assert!(calls >= 30, "{calls}");
    }

    #[test]
    fn v2_legs_of_other_shapes_are_not_decoded() {
        let ok = hex::decode(include_str!("../tests/fixtures/f1_one_leg.hex").trim()).unwrap();
        assert!(decode(SELECTOR_V2, &ok).is_some());
        // A width outside 3..=10, a wrong length for the widths, and more than two legs are not read.
        let mut wide = ok.clone();
        wide[1] = 11;
        assert!(decode(SELECTOR_V2, &wide).is_none());
        let mut narrow = ok.clone();
        narrow[1] = 4; // widths (4, 7) but the length of (9, 7): not a known shape
        assert!(decode(SELECTOR_V2, &narrow).is_none());
        let mut three_legs = ok.clone();
        three_legs[0] = 3;
        assert!(decode(SELECTOR_V2, &three_legs).is_none());
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
