//! Swaps these bots send to the Uniswap V4 PoolManager, and one more V3 bot with plain ABI words. Layouts were
//! fitted from `debug_traceTransaction` traces against the PoolManager's own `swap` call (and the pool's, for V3) and
//! verified on every traced transaction (2026-10-08); see the fixtures in `tests/fixtures`.
//!
//! V4 pools have no contract of their own, so a decoded V4 swap names its pool by the stand-in address derived from
//! its id (`mev_protocol::v4::stand_in`), the same address the pool store uses. Native ETH is the zero address.
//!
//! Router A, selector `669dc5b6`: `(currency0, currency1, fee, tickSpacing, hooks, zeroForOne, -amountIn, sqrtLimit,
//! manager, deadline, ..)`: the key is explicit and sorted; the amount is negative (exact input).
//!
//! Router E (`0xea845931...`):
//!  - `755f317c` (V3 pool): `(deadline, pool, tokenIn, tokenOut, amountIn, minOut, sqrtLimit, ..)`.
//!  - `4fd7c9b7` (V4 pool): `(deadline, tokenIn, tokenOut, variant, fee, tickSpacing, hooks, amountIn, minOut,
//!    sqrtLimit, ..)`. Tokens are not sorted. `variant` 2 keeps WETH as WETH; 3 and 50 use native ETH for WETH in the
//!    pool key. Other variants are not decoded.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi, v4};

pub const ROUTER_E: Address = address!("ea84593154d5834ef39b832bf9745126771861a2");
pub const SELECTOR_A_V4: [u8; 4] = [0x66, 0x9d, 0xc5, 0xb6];
pub const SELECTOR_E_V3: [u8; 4] = [0x75, 0x5f, 0x31, 0x7c];
pub const SELECTOR_E_V4: [u8; 4] = [0x4f, 0xd7, 0xc9, 0xb7];
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub(crate) fn small(d: &[u8], i: usize) -> Option<u32> {
    u32::try_from(abi::uint(d, i)?).ok()
}

pub(crate) fn non_zero(v: U256) -> Option<U256> {
    (!v.is_zero()).then_some(v)
}

/// A V4 pool key.
pub(crate) struct Key {
    pub(crate) currency0: Address,
    pub(crate) currency1: Address,
    pub(crate) fee: u32,
    pub(crate) tick_spacing: u32,
    pub(crate) hooks: Address,
}

pub(crate) fn v4_swap(
    k: Key,
    zero_for_one: bool,
    amount_in: U256,
    limit: U256,
) -> Option<SwapIntent> {
    if k.currency0 >= k.currency1
        || k.fee > 0xff_ffff
        || k.tick_spacing == 0
        || k.tick_spacing > 0x7fff
        || amount_in.is_zero()
    {
        return None;
    }
    let id = v4::pool_id(
        k.currency0,
        k.currency1,
        k.fee,
        i32::try_from(k.tick_spacing).ok()?,
        k.hooks,
    );
    Some(SwapIntent {
        pool: PoolRef::Address(v4::stand_in(id)),
        token_in: if zero_for_one {
            k.currency0
        } else {
            k.currency1
        },
        amount_in,
        sqrt_limit_x96: non_zero(limit),
        hops: 1,
    })
}

/// Router A's V4 swap (`669dc5b6`); `d` is the calldata after the selector.
pub fn decode_a_v4(d: &[u8]) -> Option<SwapIntent> {
    if abi::addr(d, 8)? != v4::MANAGER {
        return None;
    }
    let zero_for_one = match abi::uint(d, 5)? {
        v if v.is_zero() => false,
        v if v == U256::from(1u8) => true,
        _ => return None,
    };
    let signed = abi::uint(d, 6)?;
    // Exact input is a negative amount (two's complement); anything else is not an exact-input swap.
    if !signed.bit(255) {
        return None;
    }
    let key = Key {
        currency0: abi::addr(d, 0)?,
        currency1: abi::addr(d, 1)?,
        fee: small(d, 2)?,
        tick_spacing: small(d, 3)?,
        hooks: abi::addr(d, 4)?,
    };
    v4_swap(
        key,
        zero_for_one,
        U256::ZERO.wrapping_sub(signed),
        abi::uint(d, 7)?,
    )
}

/// Router E's swaps (`755f317c` on a V3 pool, `4fd7c9b7` on a V4 pool).
pub fn decode_e(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel == SELECTOR_E_V3 {
        return Some(SwapIntent {
            pool: PoolRef::Address(abi::addr(d, 1)?),
            token_in: abi::addr(d, 2)?,
            amount_in: abi::uint(d, 4)?,
            sqrt_limit_x96: non_zero(abi::uint(d, 6)?),
            hops: 1,
        });
    }
    if sel != SELECTOR_E_V4 {
        return None;
    }
    let native = match abi::uint(d, 3)? {
        v if v == U256::from(2u8) => false,
        v if v == U256::from(3u8) || v == U256::from(50u8) => true,
        _ => return None,
    };
    let map = |t: Address| {
        if native && t == WETH {
            Address::ZERO
        } else {
            t
        }
    };
    let (token_in, token_out) = (map(abi::addr(d, 1)?), map(abi::addr(d, 2)?));
    if token_in == token_out {
        return None;
    }
    let (c0, c1) = if token_in < token_out {
        (token_in, token_out)
    } else {
        (token_out, token_in)
    };
    let key = Key {
        currency0: c0,
        currency1: c1,
        fee: small(d, 4)?,
        tick_spacing: small(d, 5)?,
        hooks: abi::addr(d, 6)?,
    };
    v4_swap(key, token_in == c0, abi::uint(d, 7)?, abi::uint(d, 9)?)
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

    fn check_v4(f: &[&str], s: &SwapIntent) {
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
    }

    #[test]
    fn router_a_v4_swaps_decode_to_the_traced_pool_key_direction_amount_and_limit() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/v4_a_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = decode_a_v4(&input[4..]).unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            check_v4(&f, &s);
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn router_e_v4_swaps_decode_to_the_traced_pool_key_direction_amount_and_limit() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/v4_e_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = decode_e(SELECTOR_E_V4, &input[4..])
                .unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            check_v4(&f, &s);
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn router_e_v3_swaps_decode_to_the_traced_pool_token_amount_and_limit() {
        let mut n = 0;
        for f in rows(include_str!("../tests/fixtures/e_v3_rows.csv")) {
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = decode_e(SELECTOR_E_V3, &input[4..])
                .unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            assert_eq!(s.pool, PoolRef::Address(f[1].parse().unwrap()), "{}", f[0]);
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{}", f[0]);
            assert_eq!(
                s.sqrt_limit_x96,
                Some(f[4].parse::<U256>().unwrap()),
                "{}",
                f[0]
            );
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn malformed_or_unknown_v4_calls_are_not_decoded() {
        let f = rows(include_str!("../tests/fixtures/v4_a_rows.csv"))
            .next()
            .unwrap();
        let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
        let d = &input[4..];
        assert!(decode_a_v4(d).is_some());
        let mut wrong_manager = d.to_vec();
        wrong_manager[8 * 32 + 31] ^= 1;
        assert!(decode_a_v4(&wrong_manager).is_none(), "a different manager");
        let mut positive = d.to_vec();
        positive[6 * 32..7 * 32].copy_from_slice(&U256::from(5u8).to_be_bytes::<32>());
        assert!(
            decode_a_v4(&positive).is_none(),
            "a positive amount is exact-output"
        );
        let mut bad_flag = d.to_vec();
        bad_flag[5 * 32 + 31] = 2;
        assert!(decode_a_v4(&bad_flag).is_none(), "direction must be 0 or 1");
        assert!(decode_a_v4(&d[..8 * 32]).is_none(), "truncated");
        let e = rows(include_str!("../tests/fixtures/v4_e_rows.csv"))
            .next()
            .unwrap();
        let ei = hex::decode(e[0].trim_start_matches("0x")).unwrap();
        let mut variant = ei[4..].to_vec();
        variant[3 * 32 + 31] = 9;
        assert!(
            decode_e(SELECTOR_E_V4, &variant).is_none(),
            "an unknown pool variant"
        );
        assert!(
            decode_e([0, 0, 0, 0], &ei[4..]).is_none(),
            "another selector"
        );
    }
}
