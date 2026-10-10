//! A meta-aggregator router `0x9dda6ef3...`: `swap(string aggregatorId, address tokenFrom, uint256 amount, bytes data)`
//! (selector `0x5f575529`). `data` is `abi.encode(tokenFrom, tokenTo, amountAfterFee, ..)` followed by the chosen
//! aggregator's own calldata (1inch, OpenOcean, Uniswap with Permit2, Mayan), so the call names both tokens and the
//! amount but not the pools: the intent is `PoolRef::Tokens`. Measured 2026-10-09 on 40 of 40 landed calls:
//! `data[0]` is the call's `tokenFrom`, `data[1]` a token that moved (or native), the call's `amount` equals the
//! Transfer logs, or their sum, or the transaction value (the amount inside `data` is net of a fee on 12 of 40, so the
//! call's own is used). Native ETH (zero or `0xEeee..`) is WETH for pool purposes; a native to WETH swap decodes to nothing.

use alloy::primitives::{Address, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const ROUTER: Address = address!("9dda6ef3d919c9bc8885d5560999a3640431e8e6");
pub const SELECTOR: [u8; 4] = [0x5f, 0x57, 0x55, 0x29];
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR {
        return None;
    }
    let wrap = |a: Address| {
        if a.is_zero() || a == NATIVE { WETH } else { a }
    };
    let amount = abi::uint(d, 2)?;
    let off = usize::try_from(abi::uint(d, 3)?).ok()? / 32;
    let len = usize::try_from(abi::uint(d, off)?).ok()?;
    let data = d.get(32 * (off + 1)..32 * (off + 1) + len)?;
    let (from, to) = (wrap(abi::addr(data, 0)?), wrap(abi::addr(data, 1)?));
    // The call's own tokenFrom must agree with the encoded one.
    if wrap(abi::addr(d, 1)?) != from || from == to || amount.is_zero() {
        return None;
    }
    Some(SwapIntent {
        pool: PoolRef::Tokens {
            token_a: from,
            token_b: to,
        },
        token_in: from,
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
        for l in include_str!("../tests/fixtures/meta_swap_rows.csv")
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
        assert!(n >= 30, "{n}");
    }

    #[test]
    fn garbage_and_other_selectors_decode_to_nothing() {
        assert!(decode(SELECTOR, &[0u8; 96]).is_none());
        assert!(decode([0; 4], &[0u8; 320]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
    }
}
