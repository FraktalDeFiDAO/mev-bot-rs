//! A packed MEV bot `0xaa0000dc...` (selector `0x7ef88937`): a variable-length program (193-954 bytes) that starts with
//! the constant 16-byte header `00e2002c6df8000500b4553900b4ba87`, then `0000`. Its view phase declares every pool it
//! will trade as `pool(20) 04 <selector>` with `3850c7bd` (`slot0`, V3), `0902f1ac` (`getReserves`, V2) or `e76c01e4`;
//! after a token's first mention that pattern never occurs (460 of 461 first mentions of a pool, 0 of the tokens').
//! So the declared pools are the pools it trades on. Measured on 166 landed calls (2026-10-09, 320 traces): 97.2% of
//! the declared pools were swapped and 95.4% of the swapped pools were declared. The sold token, direction and the
//! per-hop amounts are NOT recoverable from the calldata (DECISIONS #153-#156), so this is a pool set, not a swap intent.

use alloy::primitives::{Address, address};

pub const BOT: Address = address!("aa0000dc97008b87383600006100003b6b3c2020");
pub const SELECTOR: [u8; 4] = [0x7e, 0xf8, 0x89, 0x37];
const HEADER: [u8; 16] = [
    0x00, 0xe2, 0x00, 0x2c, 0x6d, 0xf8, 0x00, 0x05, 0x00, 0xb4, 0x55, 0x39, 0x00, 0xb4, 0xba, 0x87,
];
const VIEW: [[u8; 4]; 3] = [
    [0x38, 0x50, 0xc7, 0xbd],
    [0x09, 0x02, 0xf1, 0xac],
    [0xe7, 0x6c, 0x01, 0xe4],
];

/// The pools the program declares, in order of first declaration.
pub fn pools(sel: [u8; 4], d: &[u8]) -> Option<Vec<Address>> {
    if sel != SELECTOR || d.get(..16)? != HEADER || d.get(16..18)? != [0, 0] {
        return None;
    }
    let mut out: Vec<Address> = Vec::new();
    for i in 18..d.len().saturating_sub(25) {
        if d[i + 20] == 0x04 && VIEW.iter().any(|v| d[i + 21..i + 25] == *v) {
            let a = Address::from_slice(&d[i..i + 20]);
            if !a.is_zero() && !out.contains(&a) {
                out.push(a);
            }
        }
    }
    (!out.is_empty() && out.len() <= 8).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;

    #[test]
    fn the_declared_pools_are_the_swapped_pools_to_the_measured_precision_and_recall() {
        let (mut named, mut swapped, mut both, mut calls) = (0usize, 0usize, 0usize, 0usize);
        for l in include_str!("../tests/fixtures/aa_bot_pools.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, sw) = l.split_once(',').unwrap();
            let input = hex::decode(input.trim_start_matches("0x")).unwrap();
            let want: Vec<Address> = sw.split('|').map(|a| a.parse().unwrap()).collect();
            let Some(got) = pools(input[..4].try_into().unwrap(), &input[4..]) else {
                continue; // a program that declares none: not read
            };
            calls += 1;
            named += got.len();
            swapped += want.len();
            both += got.iter().filter(|a| want.contains(a)).count();
        }
        // Recorded in DECISIONS #161: precision 97.2% and recall 95.4% on the 166-call sample.
        assert!(
            both as f64 / named as f64 > 0.93,
            "precision {both}/{named}"
        );
        assert!(
            both as f64 / swapped as f64 > 0.90,
            "recall {both}/{swapped}"
        );
        assert!(calls >= 100, "{calls}");
    }

    #[test]
    fn wrong_header_selector_and_garbage_name_no_pools() {
        assert!(pools(SELECTOR, &[0u8; 64]).is_none());
        assert!(pools([0; 4], &HEADER).is_none());
        assert!(pools(SELECTOR, &[]).is_none());
        let mut d = HEADER.to_vec();
        d.extend([0, 0, 0, 0]);
        d.extend([0xab; 20]);
        d.extend([0x04, 0x38, 0x50, 0xc7, 0xbd, 0, 0, 0, 0]);
        assert_eq!(
            pools(SELECTOR, &d),
            Some(vec![Address::from_slice(&[0xab; 20])])
        );
    }
}
