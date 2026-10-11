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

/// Every pool the program names (one 63-byte record per hop from byte 61, placeholders skipped), for two to four hops
/// and any tail length. Measured on 123 landed calls of 2026-10-09 and 100 of 2026-10-10 (a different day): 95.9% of the
/// named pools were swapped and 98.6% of the swapped pools were named, so this is a conservative set for holding pools
/// back, not an exact list. (The exact first hop below still needs the base layout.)
pub fn pools(sel: [u8; 4], d: &[u8]) -> Option<Vec<Address>> {
    if sel != SELECTOR {
        return None;
    }
    let hops = usize::from(*d.first()?);
    if !(2..=4).contains(&hops) || d.len() < 61 + 63 * hops + 20 {
        return None;
    }
    let named: Vec<Address> = (0..hops)
        .map(|k| Address::from_slice(&d[61 + 63 * k..81 + 63 * k]))
        .filter(|a| !a.is_zero())
        .collect();
    (!named.is_empty()).then_some(named)
}

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
    fn the_named_pools_are_the_swapped_pools_to_the_measured_precision_and_recall() {
        let (mut named, mut swapped, mut both) = (0usize, 0usize, 0usize);
        for l in include_str!("../tests/fixtures/cycle_bot_pools.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, sw) = l.split_once(',').unwrap();
            let input = hex::decode(input.trim_start_matches("0x")).unwrap();
            let got = pools(input[..4].try_into().unwrap(), &input[4..])
                .unwrap_or_else(|| panic!("no pools: {l}"));
            let want: Vec<Address> = sw.split('|').map(|a| a.parse().unwrap()).collect();
            named += got.len();
            swapped += want.len();
            both += got.iter().filter(|a| want.contains(a)).count();
        }
        // Recorded in DECISIONS #161: precision 91.2% and recall 99.6% on the 110-call sample.
        assert!(
            both as f64 / named as f64 > 0.88,
            "precision {both}/{named}"
        );
        assert!(
            both as f64 / swapped as f64 > 0.97,
            "recall {both}/{swapped}"
        );
        assert!(named >= 200, "{named}");
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
