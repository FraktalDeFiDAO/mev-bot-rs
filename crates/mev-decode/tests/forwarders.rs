//! The two swap forwarders unwrap to the router call they carry, which the protocol crates then read.

use alloy::primitives::{Address, U256, hex};
use mev_decode::{Decoded, calls};

#[test]
fn landed_forwarder_calls_decode_to_the_swap_inside_them() {
    let (mut n, mut via_forwarder) = (0, 0);
    for l in include_str!("fixtures/forwarders.csv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = l.split(',').collect();
        let to: Address = f[0].parse().unwrap();
        let input = hex::decode(f[1].trim_start_matches("0x")).unwrap();
        let swaps: Vec<_> = calls::actions_in(to, &input)
            .into_iter()
            .filter_map(|a| match a.decoded {
                Some(Decoded::Swap(s)) => Some((s, a.leaf.via)),
                _ => None,
            })
            .collect();
        let (s, via) = swaps.first().unwrap_or_else(|| panic!("no swap: {l}"));
        assert_eq!(s.token_in, f[2].parse::<Address>().unwrap(), "{l}");
        assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
        via_forwarder += usize::from(via.iter().any(|v| v.starts_with("swap_")));
        n += 1;
    }
    assert!(n >= 45, "{n}");
    assert_eq!(n, via_forwarder, "every swap was found through a forwarder");
}

#[test]
fn malformed_forwarder_calls_stay_leaves() {
    let to: Address = "0x09fc9b7545020f6a51d113e495e0a451597969d3"
        .parse()
        .unwrap();
    let junk = [0x70, 0x89, 0x09, 0xad, 1, 2, 3];
    let acts = calls::actions_in(to, &junk);
    assert_eq!(acts.len(), 1);
    assert!(acts[0].decoded.is_none() && acts[0].leaf.via.is_empty());
}
