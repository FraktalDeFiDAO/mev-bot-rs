//! Router H (`0x69bb29e2...`): a packed bot of V2 / V3 legs. Selector `00000001` takes one `bytes` argument (ABI
//! offset 0x20, length, data). Fitted from 100 `debug_traceTransaction` traces, verified on the 74 successful ones
//! (2026-10-10, fixture `tests/fixtures/h_rows.csv`), offsets into the `bytes`:
//!   0..8 `0x8000000000000000` | 12..16 `0x03100140` | 16..32 amountIn (uint128) | 109..129 tokenIn |
//!   pool records of 32 bytes, the first at 237 (another at 269, 301): a 20-byte pool address.
//! Exact on all 74: amountIn and tokenIn (the first token transfer). The pool is a pool the transaction swaps on in all
//! 74, and the first one executed in 69 of 74 (a multi-leg cycle can run its legs in another order than it lists
//! them), so it is reported as the pool, not as "the first hop". The slot at 237 begins with four zero bytes in three
//! 340-byte calls; there the pool is at 301. Other lengths and headers are not decoded.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const ROUTER_H: Address = address!("69bb29e2bf617bf74098d95054f72aee7999e4e3");
pub const SELECTOR: [u8; 4] = [0, 0, 0, 1];
/// Lengths of the `bytes` argument seen on landed calls.
const LENGTHS: [usize; 5] = [258, 276, 308, 340, 363];

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR || abi::uint(d, 0)? != U256::from(0x20u8) {
        return None;
    }
    let len = usize::try_from(abi::uint(d, 1)?).ok()?;
    let b = d.get(64..64 + len)?;
    if !LENGTHS.contains(&len)
        || b[..8] != [0x80, 0, 0, 0, 0, 0, 0, 0]
        || b[12..16] != [3, 0x10, 1, 0x40]
    {
        return None;
    }
    let amount = U256::from_be_slice(&b[16..32]);
    let slot = |off: usize| -> Option<Address> {
        let a = b.get(off..off + 20)?;
        a[..4]
            .iter()
            .any(|x| *x != 0)
            .then(|| Address::from_slice(a))
    };
    let pool = slot(237).or_else(|| if len == 340 { slot(301) } else { None })?;
    (!amount.is_zero()).then_some(SwapIntent {
        pool: PoolRef::Address(pool),
        token_in: Address::from_slice(&b[109..129]),
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
    fn traced_calls_decode_to_a_swapped_pool_the_token_and_the_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/h_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (sel, d) = abi::split(&input).unwrap();
            let s = decode(sel, d).unwrap_or_else(|| panic!("not decoded: {}", f[0]));
            let PoolRef::Address(pool) = s.pool else {
                panic!()
            };
            assert!(
                f[1].split('|')
                    .any(|p| p.parse::<Address>().unwrap() == pool),
                "{pool} not among {}",
                f[1]
            );
            assert_eq!(s.token_in, f[2].parse::<Address>().unwrap(), "{}", f[0]);
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{}", f[0]);
            n += 1;
        }
        assert!(n >= 70, "{n}");
    }

    #[test]
    fn other_selectors_lengths_and_headers_are_not_decoded() {
        let l = include_str!("../tests/fixtures/h_rows.csv")
            .lines()
            .find(|l| !l.starts_with('#'))
            .unwrap();
        let input = hex::decode(l.split(',').next().unwrap().trim_start_matches("0x")).unwrap();
        let (sel, d) = abi::split(&input).unwrap();
        assert!(decode(sel, d).is_some());
        assert!(decode([0, 0, 0, 2], d).is_none());
        let mut bad = d.to_vec();
        bad[64] = 0; // header byte
        assert!(decode(sel, &bad).is_none());
        assert!(decode(sel, &d[..200]).is_none());
    }
}
