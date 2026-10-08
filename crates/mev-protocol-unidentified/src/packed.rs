//! Router D: a swap bot whose calldata is packed into one 32-byte argument (about a quarter of swap transactions on
//! 2026-10-08, none of them naming a pool or token address). Fitted from transaction traces and verified exactly on
//! 505 of 505 traced transactions (amount, direction and price limit), see `scripts/learn-packed-table.py`.
//!
//! Argument layout (after the 4-byte selector; the selector picks the pool table):
//!   bytes 0-1   pool index
//!   byte  2     left shift, in bits, applied to the amount
//!   bytes 3-10  amount mantissa (uint64)
//!   bytes 11-30 sqrtPriceLimitX96 (uint160)
//!   byte  31    direction, bit 0 = zeroForOne
//! Selector `6bfd6286` (a 64-byte argument, no direct pool swap) is a different shape and is not decoded.

use alloy::primitives::{Address, U256, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

use crate::packed_table::lookup;

pub const ROUTER_D: Address = address!("93c30e6e09f698cfea99b09b5166ed90cfaedd17");
/// The selectors whose argument has the layout above.
pub const SELECTORS: [[u8; 4]; 3] = [
    [0xb2, 0x46, 0x0c, 0x48],
    [0xb4, 0x2e, 0xbb, 0x63],
    [0xb7, 0xf4, 0x9f, 0x3f],
];

pub fn decode(to: Address, input: &[u8]) -> Option<SwapIntent> {
    let (sel, arg) = abi::split(input)?;
    if to != ROUTER_D || !SELECTORS.contains(&sel) || arg.len() != 32 {
        return None;
    }
    let (pool, token0, token1) = lookup(sel, u16::from_be_bytes([arg[0], arg[1]]))?;
    let shift = u32::from(arg[2]);
    let mantissa = u64::from_be_bytes(arg[3..11].try_into().ok()?);
    // Refuse an amount that does not fit in 256 bits rather than wrap it.
    if mantissa != 0 && 64 - mantissa.leading_zeros() + shift > 256 {
        return None;
    }
    let limit = U256::from_be_slice(&arg[11..31]);
    let zero_for_one = arg[31] & 1 == 1;
    Some(SwapIntent {
        pool: PoolRef::Address(pool),
        token_in: if zero_for_one { token0 } else { token1 },
        amount_in: U256::from(mantissa) << shift,
        sqrt_limit_x96: (!limit.is_zero()).then_some(limit),
        hops: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, hex};

    /// Real transactions with trace ground truth: (calldata, pool that swapped, zeroForOne, amount, price limit).
    fn rows() -> Vec<[&'static str; 5]> {
        include_str!("../tests/fixtures/packed_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|l| {
                let f: Vec<&str> = l.split(',').collect();
                [f[0], f[1], f[2], f[3], f[4]]
            })
            .collect()
    }

    #[test]
    fn real_transactions_decode_to_what_their_traces_show() {
        for [input, pool, zfo, amount, limit] in rows() {
            let bytes = hex::decode(input.trim_start_matches("0x")).unwrap();
            let s = decode(ROUTER_D, &bytes).unwrap_or_else(|| panic!("not decoded: {input}"));
            assert_eq!(s.pool, PoolRef::Address(pool.parse().unwrap()), "{input}");
            assert_eq!(s.amount_in, amount.parse::<U256>().unwrap(), "{input}");
            assert_eq!(
                s.sqrt_limit_x96,
                Some(limit.parse::<U256>().unwrap()),
                "{input}"
            );
            let (_, t0, t1) = lookup(
                bytes[..4].try_into().unwrap(),
                u16::from_be_bytes([bytes[4], bytes[5]]),
            )
            .unwrap();
            assert_eq!(s.token_in, if zfo == "1" { t0 } else { t1 }, "{input}");
        }
    }

    #[test]
    fn every_traced_transaction_in_the_fixture_file_decodes_to_its_trace() {
        let rows = include_str!("../tests/fixtures/packed_rows.csv");
        let mut n = 0;
        for line in rows
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = line.split(',').collect();
            let bytes = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = decode(ROUTER_D, &bytes).unwrap_or_else(|| panic!("not decoded: {line}"));
            assert_eq!(s.pool, PoolRef::Address(f[1].parse().unwrap()), "{line}");
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{line}");
            assert_eq!(
                s.sqrt_limit_x96,
                Some(f[4].parse::<U256>().unwrap()),
                "{line}"
            );
            let (_, t0, t1) = lookup(
                bytes[..4].try_into().unwrap(),
                u16::from_be_bytes([bytes[4], bytes[5]]),
            )
            .unwrap();
            assert_eq!(s.token_in, if f[2] == "1" { t0 } else { t1 }, "{line}");
            n += 1;
        }
        assert!(n >= 100, "only {n} fixture rows");
    }

    #[test]
    fn other_contracts_selectors_lengths_and_unknown_indices_are_not_decoded() {
        let input = rows()[0][0];
        let bytes = hex::decode(input.trim_start_matches("0x")).unwrap();
        assert!(decode(ROUTER_D, &bytes).is_some());
        assert!(
            decode(Address::with_last_byte(1), &bytes).is_none(),
            "another contract"
        );
        assert!(decode(ROUTER_D, &bytes[..35]).is_none(), "truncated");
        let mut long = bytes.clone();
        long.push(0);
        assert!(decode(ROUTER_D, &long).is_none(), "extra byte");
        let mut other_sel = bytes.clone();
        other_sel[..4].copy_from_slice(&[0x6b, 0xfd, 0x62, 0x86]);
        assert!(
            decode(ROUTER_D, &other_sel).is_none(),
            "the 64-byte shape is not this layout"
        );
        let mut unknown = bytes.clone();
        unknown[4] = 0xff;
        unknown[5] = 0xff;
        assert!(
            decode(ROUTER_D, &unknown).is_none(),
            "an index not in the table"
        );
    }

    #[test]
    fn the_amount_shift_is_applied_and_an_overflowing_one_is_refused() {
        let input = rows()[0][0];
        let mut bytes = hex::decode(input.trim_start_matches("0x")).unwrap();
        bytes[4 + 2] = 4; // shift 4 bits
        bytes[4 + 3..4 + 11].copy_from_slice(&1u64.to_be_bytes());
        assert_eq!(decode(ROUTER_D, &bytes).unwrap().amount_in, U256::from(16));
        bytes[4 + 2] = 255; // 1 << 255 fits, 1 << 256 would not; a mantissa with 64 bits at shift 255 overflows
        bytes[4 + 3..4 + 11].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode(ROUTER_D, &bytes).is_none());
    }

    #[test]
    fn the_table_is_one_to_one_per_selector() {
        let mut seen = std::collections::HashSet::new();
        for r in crate::packed_table::TABLE {
            assert!(
                seen.insert((r.0, r.1)),
                "duplicate index {:#06x} for selector {:02x?}",
                r.1,
                r.0
            );
            assert!(
                SELECTORS.contains(&r.0),
                "selector {:02x?} is not decoded",
                r.0
            );
        }
        assert!(crate::packed_table::TABLE.len() >= 29);
    }
}
