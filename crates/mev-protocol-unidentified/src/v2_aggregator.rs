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

/// Words of a length-prefixed `address[]` at word offset `at` (relative to `d`'s start).
fn addresses(d: &[u8], at: usize) -> Option<Vec<Address>> {
    let n = usize::try_from(abi::uint(d, at)?).ok()?;
    if n == 0 || n > 16 {
        return None;
    }
    (0..n).map(|i| abi::addr(d, at + 1 + i)).collect()
}

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
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
    fn garbage_and_other_selectors_decode_to_nothing() {
        assert!(decode(SELECTOR, &[0u8; 64]).is_none());
        assert!(decode([0; 4], &[0u8; 64]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
    }
}
