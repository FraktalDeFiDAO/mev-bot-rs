//! Read a feed transaction's swap straight from its calldata, without a node.
//!
//! Three non-public contracts account for about 94% of the feed transactions that move a watched pool
//! (measured 2026-10-08, DECISIONS #89). Each runs a single exact-input swap on one Uniswap-V3-style pool with the
//! amount, token and (for two of them) a sqrt-price limit in fixed calldata words. Layouts were read from
//! landed transactions and checked against their `Swap` events (the router address is a measured fact, not a
//! documented interface: if a layout changes, `decode` returns `None` or the shadow comparison reports it).

use alloy::primitives::{Address, U256, address};

/// Contract with selector `0xa00597a0`: `(amountIn, sqrtPriceLimitX96, tokenIn, flag, pool, deadline, ..)`.
pub const ROUTER_A: Address = address!("278d858f05b94576C1E6f73285886876ff6eF8D2");
/// Contract with selector `0x000000e7`: `(pool, 0, amountIn, sqrtPriceLimitX96, tokenIn)`.
pub const ROUTER_B: Address = address!("96DaA0B8a5499EA9323421ED0Cda06B345caab73");
/// Contract with selector `0xb858183f`: `(0x20, 0x80, recipient, amountIn, amountOutMin, 0x2b, path)` with
/// `path = tokenIn(20) fee(3) tokenOut(20)`; the pool is found by tokens and fee.
pub const ROUTER_C: Address = address!("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45");

/// Which pool a swap is aimed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolRef {
    Address(Address),
    Pair {
        token_a: Address,
        token_b: Address,
        fee: u32,
    },
}

/// One exact-input swap on one pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapIntent {
    pub pool: PoolRef,
    pub token_in: Address,
    pub amount_in: U256,
    /// `sqrtPriceLimitX96`: the swap stops here with input left over. `None` when the contract has none.
    pub sqrt_limit_x96: Option<U256>,
}

fn word(d: &[u8], i: usize) -> Option<&[u8]> {
    d.get(32 * i..32 * i + 32)
}

fn addr(d: &[u8], i: usize) -> Option<Address> {
    let w = word(d, i)?;
    w[..12]
        .iter()
        .all(|b| *b == 0)
        .then(|| Address::from_slice(&w[12..]))
}

fn uint(d: &[u8], i: usize) -> Option<U256> {
    word(d, i).map(U256::from_be_slice)
}

/// The swap a call to `to` with `input` performs, if it is one of the contracts above and the layout matches.
pub fn decode(to: Address, input: &[u8]) -> Option<SwapIntent> {
    let (sel, d) = (input.get(..4)?, input.get(4..)?);
    if to == ROUTER_A && sel == [0xa0, 0x05, 0x97, 0xa0] {
        return Some(SwapIntent {
            amount_in: uint(d, 0)?,
            sqrt_limit_x96: Some(uint(d, 1)?),
            token_in: addr(d, 2)?,
            pool: PoolRef::Address(addr(d, 4)?),
        });
    }
    if to == ROUTER_B && sel == [0x00, 0x00, 0x00, 0xe7] {
        return Some(SwapIntent {
            pool: PoolRef::Address(addr(d, 0)?),
            amount_in: uint(d, 2)?,
            sqrt_limit_x96: Some(uint(d, 3)?),
            token_in: addr(d, 4)?,
        });
    }
    if to == ROUTER_C && sel == [0xb8, 0x58, 0x18, 0x3f] {
        if uint(d, 5)? != U256::from(43u8) {
            return None; // a multi-hop path or another layout
        }
        let path = d.get(32 * 6..32 * 6 + 43)?;
        let token_in = Address::from_slice(&path[..20]);
        let fee = u32::from_be_bytes([0, path[20], path[21], path[22]]);
        let token_out = Address::from_slice(&path[23..43]);
        return Some(SwapIntent {
            pool: PoolRef::Pair {
                token_a: token_in,
                token_b: token_out,
                fee,
            },
            token_in,
            amount_in: uint(d, 3)?,
            sqrt_limit_x96: None,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");

    fn w(v: U256) -> Vec<u8> {
        v.to_be_bytes::<32>().to_vec()
    }
    fn wa(a: Address) -> Vec<u8> {
        let mut o = vec![0u8; 12];
        o.extend_from_slice(a.as_slice());
        o
    }
    fn call(sel: [u8; 4], words: Vec<Vec<u8>>) -> Vec<u8> {
        let mut o = sel.to_vec();
        for x in words {
            o.extend(x);
        }
        o
    }

    #[test]
    fn router_a_reads_amount_limit_token_and_pool() {
        let pool = address!("0eb43b37539a4261aa1860f9476094bb2c00a894");
        let input = call(
            [0xa0, 0x05, 0x97, 0xa0],
            vec![
                w(U256::from(0x2678e8e1de7d99u64)),
                w(U256::from(7_777u64)),
                wa(WETH),
                w(U256::from(1)),
                wa(pool),
                w(U256::from(0x6ac79943u64)),
                w(U256::from(15)),
            ],
        );
        let s = decode(ROUTER_A, &input).unwrap();
        assert_eq!(s.pool, PoolRef::Address(pool));
        assert_eq!(s.token_in, WETH);
        assert_eq!(s.amount_in, U256::from(0x2678e8e1de7d99u64));
        assert_eq!(s.sqrt_limit_x96, Some(U256::from(7_777u64)));
    }

    #[test]
    fn router_b_reads_pool_amount_limit_and_token() {
        let pool = address!("c6962004f452be9203591991d15f6b388e09e8d0");
        let input = call(
            [0, 0, 0, 0xe7],
            vec![
                wa(pool),
                w(U256::ZERO),
                w(U256::from(0xe277db25u64)),
                w(U256::from(99u64)),
                wa(USDC),
            ],
        );
        let s = decode(ROUTER_B, &input).unwrap();
        assert_eq!(s.pool, PoolRef::Address(pool));
        assert_eq!(s.token_in, USDC);
        assert_eq!(s.amount_in, U256::from(0xe277db25u64));
        assert_eq!(s.sqrt_limit_x96, Some(U256::from(99u64)));
    }

    #[test]
    fn router_c_reads_a_single_hop_path() {
        let arb = address!("912CE59144191C1204E64559FE8253a0e49E6548");
        let mut path = Vec::new();
        path.extend_from_slice(WETH.as_slice());
        path.extend_from_slice(&[0, 0, 100]);
        path.extend_from_slice(arb.as_slice());
        path.resize(64, 0);
        let input = call(
            [0xb8, 0x58, 0x18, 0x3f],
            vec![
                w(U256::from(0x20)),
                w(U256::from(0x80)),
                wa(Address::with_last_byte(9)),
                w(U256::from(0x11f2b19548fb5bcu64)),
                w(U256::from(5)),
                w(U256::from(43)),
                path,
            ],
        );
        let s = decode(ROUTER_C, &input).unwrap();
        assert_eq!(s.token_in, WETH);
        assert_eq!(s.amount_in, U256::from(0x11f2b19548fb5bcu64));
        assert_eq!(
            s.pool,
            PoolRef::Pair {
                token_a: WETH,
                token_b: arb,
                fee: 100
            }
        );
        assert_eq!(s.sqrt_limit_x96, None);
    }

    #[test]
    fn anything_else_is_not_decoded() {
        let ok = call(
            [0, 0, 0, 0xe7],
            vec![
                wa(Address::ZERO),
                w(U256::ZERO),
                w(U256::from(1)),
                w(U256::from(1)),
                wa(USDC),
            ],
        );
        assert!(decode(ROUTER_B, &ok).is_some());
        assert!(
            decode(Address::with_last_byte(1), &ok).is_none(),
            "another contract"
        );
        assert!(decode(ROUTER_A, &ok).is_none(), "another selector");
        assert!(decode(ROUTER_B, &ok[..40]).is_none(), "truncated");
        let mut dirty = ok.clone();
        dirty[4] = 1; // pool word with garbage in the high bytes
        assert!(decode(ROUTER_B, &dirty).is_none());
        // Router C with a multi-hop path length is skipped.
        let multi = call(
            [0xb8, 0x58, 0x18, 0x3f],
            vec![
                vec![0; 32],
                vec![0; 32],
                vec![0; 32],
                vec![0; 32],
                vec![0; 32],
                w(U256::from(66)),
            ],
        );
        assert!(decode(ROUTER_C, &multi).is_none());
    }
}
