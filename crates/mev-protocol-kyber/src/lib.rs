//! KyberSwap `MetaAggregationRouterV2.swap(SwapExecutionParams)` (selector `0xe21fd0e9`). The description names the
//! source and destination tokens and the amount; the executor's call data picks the pools, so the intent is
//! `PoolRef::Tokens`. Measured 2026-10-09 on 33 of 33 landed calls: source, destination and amount exact (the Transfer
//! logs, or the transaction value for a native source). Native ETH (`0xEeee..`) is WETH for pool purposes; a native to
//! WETH swap is a wrap and decodes to nothing.

use alloy::{
    primitives::{Address, address},
    sol,
    sol_types::SolCall,
};
use mev_protocol::{Contract, Decoded, EventKind, PoolRef, Protocol, SwapIntent};

sol! {
    struct SwapDescriptionV2 {
        address srcToken; address dstToken; address[] srcReceivers; uint256[] srcAmounts;
        address[] feeReceivers; uint256[] feeAmounts; address dstReceiver; uint256 amount; uint256 minReturnAmount;
        uint256 flags; bytes permit;
    }
    struct SwapExecutionParams {
        address callTarget; address approveTarget; bytes targetData; SwapDescriptionV2 desc; bytes clientData;
    }
    function swap(SwapExecutionParams execution) external payable returns (uint256 returnAmount, uint256 gasUsed);
}

pub const ROUTER: Address = address!("6131B5fae19EA4f9D964eAc0408E4408b66337b5");
const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

pub struct Kyber;

impl Protocol for Kyber {
    fn id(&self) -> &'static str {
        "kyber"
    }

    fn events(&self) -> Vec<EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        if to != ROUTER {
            return None;
        }
        let c = swapCall::abi_decode(input).ok()?;
        let d = &c.execution.desc;
        let wrap = |a: Address| if a == NATIVE { WETH } else { a };
        let (src, dst) = (wrap(d.srcToken), wrap(d.dstToken));
        (src != dst && !d.amount.is_zero()).then_some(Decoded::Swap(SwapIntent {
            pool: PoolRef::Tokens {
                token_a: src,
                token_b: dst,
            },
            token_in: src,
            amount_in: d.amount,
            sqrt_limit_x96: None,
            hops: 1,
            amount_out_exact: None,
        }))
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: ROUTER,
            name: "KyberSwap MetaAggregationRouterV2 (swap decoded to its tokens)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, hex};

    #[test]
    fn landed_calls_decode_to_their_tokens_and_amount() {
        let (mut n, mut wraps) = (0, 0);
        for l in include_str!("../tests/fixtures/swap_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let (src, dst): (Address, Address) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            let wrap = |a: Address| if a == NATIVE { WETH } else { a };
            if wrap(src) == wrap(dst) {
                assert!(Kyber.decode_call(ROUTER, &input).is_none(), "{l}");
                wraps += 1;
                continue;
            }
            let Some(Decoded::Swap(s)) = Kyber.decode_call(ROUTER, &input) else {
                panic!("not decoded: {l}")
            };
            assert_eq!(s.token_in, wrap(src), "{l}");
            assert_eq!(
                s.pool,
                PoolRef::Tokens {
                    token_a: wrap(src),
                    token_b: wrap(dst)
                },
                "{l}"
            );
            assert_eq!(s.amount_in, f[3].parse::<U256>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 25, "{n} {wraps}");
    }

    #[test]
    fn other_contracts_and_garbage_decode_to_nothing() {
        let l = include_str!("../tests/fixtures/swap_rows.csv")
            .lines()
            .nth(1)
            .unwrap();
        let input = hex::decode(l.split(',').next().unwrap().trim_start_matches("0x")).unwrap();
        assert!(
            Kyber
                .decode_call(Address::with_last_byte(1), &input)
                .is_none()
        );
        assert!(Kyber.decode_call(ROUTER, &input[..100]).is_none());
        assert!(Kyber.decode_call(ROUTER, b"").is_none());
    }
}
