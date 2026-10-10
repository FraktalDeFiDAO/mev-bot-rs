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
/// `dagSwapTo(uint256 orderId, address receiver, (fromToken, toToken, amount, minReturn, deadline), paths[])`: the
/// same request one word later. Tokens exact on 34 of 34 landed calls, the amount on 32 (the router's own request
/// size; two calls moved it through several holders). A native-ETH source (`0xEeee..`) is not decoded.
pub const SELECTOR_DAG_SWAP_TO: [u8; 4] = [0x0c, 0x30, 0x7f, 0x76];
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

pub fn decode(sel: [u8; 4], d: &[u8]) -> Option<SwapIntent> {
    let shift = if sel == SELECTOR_DAG_SWAP {
        0
    } else if sel == SELECTOR_DAG_SWAP_TO {
        1
    } else {
        return None;
    };
    let (src, dst) = (abi::addr(d, 1 + shift)?, abi::addr(d, 2 + shift)?);
    if src == NATIVE {
        return None;
    }
    Some(SwapIntent {
        pool: PoolRef::Tokens {
            token_a: src,
            token_b: dst,
        },
        token_in: src,
        amount_in: abi::uint(d, 3 + shift)?,
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
        // `dagSwapTo` carries the receiver in word 1, so the request starts one word later.
        let mut to_call = vec![0u8; 0];
        to_call.extend(U256::from(1u64).to_be_bytes::<32>());
        to_call.extend(wa(Address::with_last_byte(5)));
        to_call.extend(wa(usdc));
        to_call.extend(wa(out));
        to_call.extend(U256::from(7u64).to_be_bytes::<32>());
        to_call.extend(U256::from(1u64).to_be_bytes::<32>());
        let s = decode(SELECTOR_DAG_SWAP_TO, &to_call).unwrap();
        assert_eq!((s.token_in, s.amount_in), (usdc, U256::from(7u64)));
        // A native-ETH source has no amount in the call.
        let mut native = to_call.clone();
        native[64..96].copy_from_slice(&wa(NATIVE));
        assert!(decode(SELECTOR_DAG_SWAP_TO, &native).is_none());
    }
}
