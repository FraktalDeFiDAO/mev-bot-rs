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

/// The pools the program declares, in order of first declaration. The program is `7ef88937` followed by the constant
/// header, either at the start of the call (selector `7ef88937`) or behind a 3-byte salt (`4a01xx` + `7ef88937`, a
/// second family on the same contract: 502 of 2,078 feed transactions on 2026-10-10); both are read the same way.
pub fn pools(input: &[u8]) -> Option<Vec<Address>> {
    let head = input.get(..40)?;
    let at = (0..=8).find(|&p| {
        head.get(p..p + 4) == Some(&SELECTOR[..]) && input.get(p + 4..p + 20) == Some(&HEADER[..])
    })?;
    let body = input.get(at + 4..)?;
    if body.get(16..18)? != [0, 0] {
        return None;
    }
    let mut out: Vec<Address> = Vec::new();
    for i in 18..body.len().saturating_sub(25) {
        if body[i + 20] == 0x04 && VIEW.iter().any(|v| body[i + 21..i + 25] == *v) {
            let a = Address::from_slice(&body[i..i + 20]);
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
            let Some(got) = pools(&input) else {
                continue; // a program that declares none: not read
            };
            calls += 1;
            named += got.len();
            swapped += want.len();
            both += got.iter().filter(|a| want.contains(a)).count();
        }
        // DECISIONS #165: precision 97.5% and recall 95.6% on 325 landed calls of both families; 248 of them declare
        // pools (the other 77 are not read).
        assert!(
            both as f64 / named as f64 > 0.93,
            "precision {both}/{named}"
        );
        assert!(
            both as f64 / swapped as f64 > 0.92,
            "recall {both}/{swapped}"
        );
        assert!(calls >= 230, "{calls}");
    }

    #[test]
    fn wrong_header_selector_and_garbage_name_no_pools() {
        assert!(pools(&[0u8; 64]).is_none());
        assert!(pools(&[]).is_none());
        let mut d = SELECTOR.to_vec();
        d.extend(HEADER);
        d.extend([0, 0, 0, 0]);
        d.extend([0xab; 20]);
        d.extend([0x04, 0x38, 0x50, 0xc7, 0xbd, 0, 0, 0, 0]);
        let want = Some(vec![Address::from_slice(&[0xab; 20])]);
        assert_eq!(pools(&d), want);
        // The same program behind a 3-byte salt (the second family on this contract).
        let mut salted = vec![0x4a, 0x01, 0x0d];
        salted.extend(&d);
        assert_eq!(pools(&salted), want);
        // Another selector in front of the header is not this program.
        let mut other = vec![0xde, 0xad, 0xbe, 0xef];
        other.extend(HEADER);
        assert!(pools(&other).is_none());
    }
}
