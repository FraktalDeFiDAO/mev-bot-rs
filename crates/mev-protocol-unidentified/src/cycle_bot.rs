//! A packed cycle-arbitrage bot `0x5dbcbd26...` (selector `0xc1c0e9c4`). Fitted from 80 `debug_traceTransaction`
//! traces on 2026-10-09, verified on every landed call of the base layout (44 of 44):
//!   0 hop count (2 or 3) | 1 flags | 2..18 amountIn (uint128, right-aligned) | ... | then one 63-byte record per hop
//!   starting at 61: `pool(20) tokenIn(20) params(23)` in execution order | 34 trailing bytes.
//! The base layout has length `61 + 63 * hops + 34`. For it, the first record is the first pool the bot swaps on
//! (108 of 108 with a non-zero first record), `amountIn` is that swap's `amountSpecified`, and the bot pays exactly
//! that token and amount into the pool (108 of 108); 2 more calls have a zero placeholder first record and are not
//! decoded. Other lengths (19 of 129 landed calls) are variants not read: their first-hop fields hold on only 15 of 19. The pools are listed, so a
//! rival's whole cycle could be read; the first hop is reported with the hop count.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent};

pub const BOT: Address = address!("5dbcbd2669e44285547aa9428d16ebe26963d6c6");
pub const SELECTOR: [u8; 4] = [0xc1, 0xc0, 0xe9, 0xc4];

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR {
        return None;
    }
    let hops = *d.first()?;
    if !(2..=3).contains(&hops) || d.len() != 61 + 63 * usize::from(hops) + 34 {
        return None;
    }
    let amount = U256::from_be_slice(&d[2..18]);
    // A zero first record is a placeholder hop (2 of 110 landed base-layout calls): there is no first pool to name.
    let pool = Address::from_slice(&d[61..81]);
    (!amount.is_zero() && !pool.is_zero()).then(|| SwapIntent {
        pool: PoolRef::Address(pool),
        token_in: Address::from_slice(&d[81..101]),
        amount_in: amount,
        sqrt_limit_x96: None,
        hops,
        amount_out_exact: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn traced_calls_decode_to_the_first_pool_token_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/cycle_bot_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = decode(input[..4].try_into().unwrap(), &input[4..])
                .unwrap_or_else(|| panic!("not decoded: {l}"));
            assert_eq!(s.pool, PoolRef::Address(f[1].parse().unwrap()), "{l}");
            assert_eq!(s.token_in, f[2].parse::<Address>().unwrap(), "{l}");
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
            assert_eq!(s.hops, f[4].parse::<u8>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 40, "{n}");
    }

    #[test]
    fn other_lengths_hops_and_selectors_decode_to_nothing() {
        assert!(
            decode(SELECTOR, &[0u8; 221]).is_none(),
            "zero amount and hops"
        );
        let mut ok = vec![0u8; 61 + 63 * 2 + 34];
        ok[0] = 2;
        ok[17] = 1;
        ok[80] = 1; // a non-zero first pool
        assert!(decode(SELECTOR, &ok).is_some());
        assert!(decode(SELECTOR, &ok[..ok.len() - 1]).is_none());
        // A placeholder (all-zero) first record names no pool.
        let mut zero_pool = ok.clone();
        zero_pool[61..81].fill(0);
        assert!(decode(SELECTOR, &zero_pool).is_none());
        ok[0] = 4;
        assert!(decode(SELECTOR, &ok).is_none());
        assert!(decode([0; 4], &[0u8; 221]).is_none());
        assert!(decode(SELECTOR, &[]).is_none());
    }
}
