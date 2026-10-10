//! Router G (`0x09f94b5f...`): an aggregator whose `dagSwapByOrderId(uint256,(uint256,address,uint256,uint256,uint256),
//! (address[],address[],uint256[],bytes[],uint256)[])` (selector `0xf2c42696`) arrives with the order inline:
//! `(orderId, srcToken, dstToken, amountIn, minReturn, deadline, offset, ..)`. Measured 2026-10-10 on 21 of 21 landed
//! calls against the transfer events: source and destination token exact; `amountIn` equals the total moved in (when
//! the router splits off a fee the transfers are separate and sum to it). The pools are chosen by the route data, so
//! the intent is `PoolRef::Tokens`.

use alloy::primitives::{Address, address};
use mev_protocol::{PoolRef, SwapIntent, abi};

pub const ROUTER_G: Address = address!("09f94b5fc68e227c323a6fbae3bd98c97fd8c849");
pub const SELECTOR_DAG_SWAP: [u8; 4] = [0xf2, 0xc4, 0x26, 0x96];

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    if sel != SELECTOR_DAG_SWAP {
        return None;
    }
    let (src, dst) = (abi::addr(d, 1)?, abi::addr(d, 2)?);
    Some(SwapIntent {
        pool: PoolRef::Tokens {
            token_a: src,
            token_b: dst,
        },
        token_in: src,
        amount_in: abi::uint(d, 3)?,
        sqrt_limit_x96: None,
        hops: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn wa(a: Address) -> Vec<u8> {
        let mut o = vec![0u8; 12];
        o.extend_from_slice(a.as_slice());
        o
    }

    #[test]
    fn reads_tokens_and_amount_of_a_landed_call() {
        // 0xcd516946...: 97 USDC for 0x77b7...; the transfer events moved 97 USDC.
        let usdc = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
        let out = address!("77b7787a09818502305c95d68a2571f090abb135");
        let mut input = SELECTOR_DAG_SWAP.to_vec();
        for w in [
            U256::from(0x3bc3133eu64).to_be_bytes::<32>().to_vec(),
            wa(usdc),
            wa(out),
            U256::from(97_000_000u64).to_be_bytes::<32>().to_vec(),
            U256::from(1u64).to_be_bytes::<32>().to_vec(),
        ] {
            input.extend(w);
        }
        let (sel, d) = abi::split(&input).unwrap();
        let s = decode(sel, d).unwrap();
        assert_eq!(s.token_in, usdc);
        assert_eq!(s.amount_in, U256::from(97_000_000u64));
        assert_eq!(
            s.pool,
            PoolRef::Tokens {
                token_a: usdc,
                token_b: out
            }
        );
        assert!(decode([0; 4], d).is_none());
    }
}
