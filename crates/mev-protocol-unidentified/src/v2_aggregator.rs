//! Aggregator `0x07964f13...`: `exactInputV2Swap((srcPool?, dstToken, flags, amountIn, minOut, w5, address[] path,
//! address[] pools, bytes, string) swap, uint256 deadline)` (selector `0x0dc4bdae`). The tuple is dynamic: word 0 is its
//! offset, word 1 the deadline, and inside the tuple `amountIn` is element 3, `path` (offset) element 6, `pools`
//! (offset) element 7. Measured 2026-10-10 on 38 of 38 landed calls against the transfer logs and emitters: `path[0]`
//! is the token sold, `amountIn` the amount moved, every `pools[i]` emitted a log, `path.len() == pools.len() + 1`.
//! The first pool is reported, with the number of pools as `hops`.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const ROUTER: Address = address!("07964f135f276412b3182a3b2407b8dd45000000");
pub const SELECTOR: [u8; 4] = [0x0d, 0xc4, 0xbd, 0xae];
/// `exactInputV3Swap((srcToken, dstToken, receiver, wrappedNative, amountIn, minOut, fee, deadline, uint256[] pools,
/// bytes, string))` on the same contract (selector `0xb9b5149b`). The tuple is dynamic. A zero `srcToken` means native
/// ETH in (the amount is then the transaction value); a zero `dstToken` means native out; element 3 is the wrapped native
/// token. Each `pools[i]` has flag bits on top and the pool address in its low 160 bits. Measured 2026-10-09 on 40 of 40
/// landed calls: tokens, amount (Transfer logs or the value) and every pool exact.
pub const SELECTOR_V3: [u8; 4] = [0xb9, 0xb5, 0x14, 0x9b];

/// Words of a length-prefixed `address[]` at word offset `at` (relative to `d`'s start).
fn addresses(d: &[u8], at: usize) -> Option<Vec<Address>> {
    let n = usize::try_from(abi::uint(d, at)?).ok()?;
    if n == 0 || n > 16 {
        return None;
    }
    (0..n).map(|i| abi::addr(d, at + 1 + i)).collect()
}

fn decode_v3(d: &[u8]) -> Option<SwapIntent> {
    let t = usize::try_from(abi::uint(d, 0)?).ok()? / 32;
    let word_at = |i: usize| -> Option<usize> { usize::try_from(abi::uint(d, t + i)?).ok() };
    let wrapped = abi::addr(d, t + 3)?;
    let pick = |a: Address| if a.is_zero() { wrapped } else { a };
    let (src, dst) = (pick(abi::addr(d, t)?), pick(abi::addr(d, t + 1)?));
    let amount = abi::uint(d, t + 4)?;
    let at = t + word_at(8)? / 32;
    let n = usize::try_from(abi::uint(d, at)?)
        .ok()
        .filter(|n| (1..=16).contains(n))?;
    // The pool is in the low 160 bits of the first element; the bits above are flags.
    let first = abi::uint(d, at + 1)?.to_be_bytes::<32>();
    let pool = Address::from_slice(&first[12..]);
    (src != dst && !amount.is_zero() && !wrapped.is_zero()).then_some(SwapIntent {
        pool: PoolRef::Address(pool),
        token_in: src,
        amount_in: amount,
        sqrt_limit_x96: None,
        hops: u8::try_from(n).ok()?,
        amount_out_exact: None,
    })
}

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel == SELECTOR_V3 {
        return decode_v3(d);
    }
    if sel != SELECTOR {
        return None;
    }
    let t = usize::try_from(abi::uint(d, 0)?).ok()? / 32;
    let word_at = |i: usize| -> Option<usize> { usize::try_from(abi::uint(d, t + i)?).ok() };
    let amount = abi::uint(d, t + 3)?;
    let path = addresses(d, t + word_at(6)? / 32)?;
    let pools = addresses(d, t + word_at(7)? / 32)?;
    if amount == U256::ZERO || path.len() != pools.len() + 1 {
        return None;
    }
    Some(SwapIntent {
        pool: PoolRef::Address(pools[0]),
        token_in: path[0],
        amount_in: amount,
        sqrt_limit_x96: None,
        hops: u8::try_from(pools.len()).ok()?,
        amount_out_exact: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn landed_calls_decode_to_token_amount_and_pools() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/v2_agg_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (sel, d) = abi::split(&input).unwrap();
            let s = decode(sel, d).unwrap_or_else(|| panic!("not decoded: {l}"));
            assert_eq!(s.token_in, f[1].parse::<Address>().unwrap());
            assert_eq!(s.amount_in, f[2].parse::<U256>().unwrap());
            let first: Address = f[3].split('|').next().unwrap().parse().unwrap();
            assert_eq!(s.pool, PoolRef::Address(first));
            assert_eq!(s.hops, f[4].parse::<u8>().unwrap());
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn landed_v3_calls_decode_to_token_amount_and_first_pool() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/v3_agg_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (sel, d) = abi::split(&input).unwrap();
            let s = decode(sel, d).unwrap_or_else(|| panic!("not decoded: {l}"));
            assert_eq!(s.token_in, f[1].parse::<Address>().unwrap(), "{l}");
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
            assert_eq!(s.pool, PoolRef::Address(f[4].parse().unwrap()), "{l}");
            assert_eq!(s.hops, f[5].parse::<u8>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn garbage_and_other_selectors_decode_to_nothing() {
        assert!(decode(SELECTOR, &[0u8; 64]).is_none());
        assert!(decode([0; 4], &[0u8; 64]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
        assert!(decode(SELECTOR_V3, &[0u8; 64]).is_none());
        assert!(decode(SELECTOR_V3, &[]).is_none());
    }
}
