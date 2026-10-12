//! `0xc6dd6c02...`: `swap((bytes32 id, uint256 amountIn, address tokenIn, uint256 bound, address tokenOut, address,
//! address, bytes data))` (selector `0xf3efe779`), a quote-based swap (large stablecoin amounts). The struct is
//! dynamic: word 0 is its offset, then element 0 is the id, 1 the amount sold, 2 the token sold, 3 a price bound (not
//! the output: it equals no transfer), 4 the token bought. Measured 2026-10-09 on 40 of 40 landed calls: token in,
//! token out and amount in equal the Transfer logs. The liquidity source is in `data`, so the intent is `PoolRef::Tokens`.

use alloy::primitives::{Address, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const ROUTER: Address = address!("c6dd6c021cfbb284f0c0d788547b51fc90fcf663");
pub const SELECTOR: [u8; 4] = [0xf3, 0xef, 0xe7, 0x79];

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR {
        return None;
    }
    let t = usize::try_from(abi::uint(d, 0)?).ok()? / 32;
    let (tin, tout) = (abi::addr(d, t + 2)?, abi::addr(d, t + 4)?);
    let amount = abi::uint(d, t + 1)?;
    (tin != tout && !amount.is_zero()).then_some(SwapIntent {
        pool: PoolRef::Tokens {
            token_a: tin,
            token_b: tout,
        },
        token_in: tin,
        amount_in: amount,
        sqrt_limit_x96: None,
        hops: 1,
        amount_out_exact: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, hex};

    #[test]
    fn landed_calls_decode_to_tokens_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/rfq_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (sel, d) = abi::split(&input).unwrap();
            let s = decode(sel, d).unwrap_or_else(|| panic!("not decoded: {l}"));
            assert_eq!(s.token_in, f[1].parse::<Address>().unwrap(), "{l}");
            assert_eq!(
                s.pool,
                PoolRef::Tokens {
                    token_a: f[1].parse().unwrap(),
                    token_b: f[2].parse().unwrap()
                },
                "{l}"
            );
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn garbage_and_other_selectors_decode_to_nothing() {
        assert!(decode(SELECTOR, &[0u8; 64]).is_none());
        assert!(decode([0; 4], &[0u8; 320]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
    }
}
