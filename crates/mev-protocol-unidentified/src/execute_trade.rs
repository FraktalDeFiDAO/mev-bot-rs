//! Arbitrage bot `0x4f8f0bc0...`: `executeTrade(address[] routers, address[] path, uint24 fee, uint256 amount)`
//! (selector `0xb4b9c57b`): sells `amount` of `path[0]` for `path[1]` through the V3 pool of that fee and trades back
//! through the second router. Measured 2026-10-10 on 29 of 29 landed calls: `path[0]` and `amount` equal the first
//! token transfer, `path.len() == 2`, `fee` is a Uniswap V3 tier. The pool is named by its tokens and fee.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const BOT: Address = address!("4f8f0bc0331e3f49fe157b7d75ab1c435e608f79");
pub const SELECTOR: [u8; 4] = [0xb4, 0xb9, 0xc5, 0x7b];
const TIERS: [u32; 4] = [100, 500, 3000, 10_000];

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR {
        return None;
    }
    let path_at = usize::try_from(abi::uint(d, 1)?).ok()? / 32;
    if abi::uint(d, path_at)? != U256::from(2u8) {
        return None;
    }
    let (a, b) = (abi::addr(d, path_at + 1)?, abi::addr(d, path_at + 2)?);
    let fee = u32::try_from(abi::uint(d, 2)?)
        .ok()
        .filter(|f| TIERS.contains(f))?;
    let amount = abi::uint(d, 3)?;
    (a != b && !amount.is_zero()).then_some(SwapIntent {
        pool: PoolRef::Pair {
            token_a: a,
            token_b: b,
            fee,
        },
        token_in: a,
        amount_in: amount,
        sqrt_limit_x96: None,
        hops: 1,
        amount_out_exact: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn landed_calls_decode_to_pair_fee_token_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/execute_trade_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (sel, d) = abi::split(&input).unwrap();
            let s = decode(sel, d).unwrap_or_else(|| panic!("not decoded: {l}"));
            assert_eq!(s.token_in, f[1].parse::<Address>().unwrap());
            assert_eq!(
                s.pool,
                PoolRef::Pair {
                    token_a: f[1].parse().unwrap(),
                    token_b: f[2].parse().unwrap(),
                    fee: f[3].parse().unwrap()
                }
            );
            assert_eq!(s.amount_in, f[4].parse::<U256>().unwrap());
            n += 1;
        }
        assert!(n >= 25, "{n}");
    }

    #[test]
    fn other_shapes_decode_to_nothing() {
        assert!(decode(SELECTOR, &[0u8; 128]).is_none());
        assert!(decode([0; 4], &[0u8; 128]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
    }
}
