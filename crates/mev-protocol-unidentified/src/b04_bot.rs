//! A packed MEV bot `0xb04e59e1...`: every call starts with a different 4-byte "selector" (1,720 distinct values in
//! 1,726 feed transactions: a salt) followed by a program. The program lists its pools as literal 20-byte addresses,
//! each preceded by a two-byte tag (`20c0`, `20c1`, `02e0`, `02e1`, `29c1`, `27c1`, `0ae0`, `0ae1`); tokens are not.
//! Tags fitted on 47 landed calls of 2026-10-10 and then checked on a different day without changing them: on 43
//! landed calls of 2026-10-09 the named pools were swapped with 99.0% precision and covered 99.0% of the swapped pools.
//! The sold token, direction and amounts are not read; this is a pool set, never an exact swap.

use alloy::primitives::{Address, address};

pub const BOT: Address = address!("b04e59e15387367df98d6f4ff2852dc33c1585f5");
const TAGS: [[u8; 2]; 8] = [
    [0x20, 0xc0],
    [0x20, 0xc1],
    [0x02, 0xe0],
    [0x02, 0xe1],
    [0x29, 0xc1],
    [0x27, 0xc1],
    [0x0a, 0xe0],
    [0x0a, 0xe1],
];

/// The pools the program names, in order of first mention (the first four bytes are the salt and are skipped).
pub fn pools(input: &[u8]) -> Option<Vec<Address>> {
    if input.len() < 4 + 22 {
        return None;
    }
    let mut out: Vec<Address> = Vec::new();
    for i in 4..input.len().saturating_sub(21) {
        if TAGS.iter().any(|t| input[i..i + 2] == *t) {
            let a = Address::from_slice(&input[i + 2..i + 22]);
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
    fn the_named_pools_are_the_swapped_pools_on_two_days() {
        let (mut named, mut swapped, mut both, mut calls) = (0usize, 0usize, 0usize, 0usize);
        for l in include_str!("../tests/fixtures/b04_pools.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, sw) = l.split_once(',').unwrap();
            let input = hex::decode(input.trim_start_matches("0x")).unwrap();
            let want: Vec<Address> = sw.split('|').map(|a| a.parse().unwrap()).collect();
            let Some(got) = pools(&input) else { continue };
            calls += 1;
            named += got.len();
            swapped += want.len();
            both += got.iter().filter(|a| want.contains(a)).count();
        }
        // DECISIONS #164: 95% / 97% in-sample and 99% / 99% on the held-out day.
        assert!(
            both as f64 / named as f64 > 0.93,
            "precision {both}/{named}"
        );
        assert!(
            both as f64 / swapped as f64 > 0.94,
            "recall {both}/{swapped}"
        );
        assert!(calls >= 80, "{calls}");
    }

    #[test]
    fn short_input_and_untagged_addresses_name_no_pools() {
        assert!(pools(&[0u8; 10]).is_none());
        // A token-like address with no tag in front of it is not a pool.
        let mut d = vec![1u8, 2, 3, 4, 0x5c, 0xc8];
        d.extend([0xcd; 20]);
        assert!(pools(&d).is_none());
        let mut tagged = vec![1u8, 2, 3, 4, 0x20, 0xc0];
        tagged.extend([0xcd; 20]);
        assert_eq!(pools(&tagged), Some(vec![Address::from_slice(&[0xcd; 20])]));
    }
}
