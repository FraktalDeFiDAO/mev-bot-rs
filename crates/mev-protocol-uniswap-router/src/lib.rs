//! Uniswap's routers on Arbitrum One. They have no events of their own (the pools emit them); this crate reads what a
//! call to a router *intends*: the first hop's pool or pair, the input token and the amount.
//!
//! Decoded: SwapRouter02 `exactInput`, `exactInputSingle`, V2-style `swapExactTokensForTokens` and both `multicall`
//! wrappers; SwapRouter v1 `exactInputSingle` and `exactInput`; Universal Router `execute` with V2 and V3 exact-input
//! commands. Not decoded (yet): exact-output swaps, V4 swaps, and commands that spend the router's own balance (the
//! input amount is then a sentinel, not a number).

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::SolCall,
};
use mev_protocol::{Contract, Decoded, PoolRef, Protocol, SwapIntent, abi};

sol! {
    interface SwapRouter02 {
        struct ExactInputParams { bytes path; address recipient; uint256 amountIn; uint256 amountOutMinimum; }
        struct ExactInputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
        function exactInput(ExactInputParams params) external payable returns (uint256 amountOut);
        function exactInputSingle(ExactInputSingleParams params) external payable returns (uint256 amountOut);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to) external payable returns (uint256 amountOut);
    }
    interface Multicall {
        function multicall(bytes[] data) external payable returns (bytes[] results);
    }
    interface MulticallDeadline {
        function multicall(uint256 deadline, bytes[] data) external payable returns (bytes[] results);
    }
    interface SwapRouterV1 {
        struct ExactInputParams { bytes path; address recipient; uint256 deadline; uint256 amountIn; uint256 amountOutMinimum; }
        struct ExactInputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 deadline; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
        function exactInput(ExactInputParams params) external payable returns (uint256 amountOut);
        function exactInputSingle(ExactInputSingleParams params) external payable returns (uint256 amountOut);
    }
    interface UniversalRouter {
        function execute(bytes commands, bytes[] inputs, uint256 deadline) external payable;
    }
    interface UniversalRouterNoDeadline {
        function execute(bytes commands, bytes[] inputs) external payable;
    }
    interface UrV3ExactIn {
        function input(address recipient, uint256 amountIn, uint256 amountOutMin, bytes path, bool payerIsUser) external;
    }
    interface UrV2ExactIn {
        function input(address recipient, uint256 amountIn, uint256 amountOutMin, address[] path, bool payerIsUser) external;
    }
}

pub const SWAP_ROUTER_02: Address = address!("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45");
pub const SWAP_ROUTER_V1: Address = address!("E592427A0AEce92De3Edee1F18E0157C05861564");
pub const UNIVERSAL_ROUTER: Address = address!("A51afAFe0263b40EdaEf0Df8781eA9aa03E381a3");
/// A second contract that answers to the Universal Router's `execute` selector (98 successful transactions in a few
/// hours on 2026-10-08, mostly V3 swaps). Treated as a Universal Router; `scripts/verify-decoder.py` checks that.
pub const UNIVERSAL_ROUTER_2: Address = address!("2d01411773c8c24805306e89a41f7855c3c4fe65");

/// Universal Router command ids (low 5 bits of each command byte; the top bit means "allow revert").
const CMD_V3_SWAP_EXACT_IN: u8 = 0x00;
const CMD_V2_SWAP_EXACT_IN: u8 = 0x08;
/// Nested calls are followed this deep (a multicall inside a multicall).
const MAX_DEPTH: usize = 2;

/// Amounts at or above 2^255 are router sentinels (for example "use the router's whole balance"), not amounts.
fn real_amount(a: U256) -> Option<U256> {
    (a < (U256::from(1u8) << 255)).then_some(a)
}

fn limit(l: impl Into<U256>) -> Option<U256> {
    let l = l.into();
    (!l.is_zero()).then_some(l)
}

/// The first hop of a Uniswap V3 packed path (`tokenIn(20) fee(3) tokenOut(20) [fee(3) token(20)]...`) and the number
/// of hops. `None` unless the length fits the pattern.
pub fn first_hop(path: &[u8]) -> Option<(Address, u32, Address, u8)> {
    if path.len() < 43 || !(path.len() - 20).is_multiple_of(23) {
        return None;
    }
    let hops = u8::try_from((path.len() - 20) / 23).ok()?;
    let fee = u32::from_be_bytes([0, path[20], path[21], path[22]]);
    Some((
        Address::from_slice(&path[..20]),
        fee,
        Address::from_slice(&path[23..43]),
        hops,
    ))
}

fn v3_path_swap(path: &[u8], amount_in: U256) -> Option<SwapIntent> {
    let (token_in, fee, token_out, hops) = first_hop(path)?;
    Some(SwapIntent {
        pool: PoolRef::Pair {
            token_a: token_in,
            token_b: token_out,
            fee,
        },
        token_in,
        amount_in: real_amount(amount_in)?,
        sqrt_limit_x96: None,
        hops,
    })
}

fn v3_single(
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
    sqrt_limit: U256,
) -> Option<SwapIntent> {
    Some(SwapIntent {
        pool: PoolRef::Pair {
            token_a: token_in,
            token_b: token_out,
            fee,
        },
        token_in,
        amount_in: real_amount(amount_in)?,
        sqrt_limit_x96: limit(sqrt_limit),
        hops: 1,
    })
}

fn v2_path_swap(path: &[Address], amount_in: U256) -> Option<SwapIntent> {
    let (a, b) = (*path.first()?, *path.get(1)?);
    Some(SwapIntent {
        pool: PoolRef::PairV2 {
            token_a: a,
            token_b: b,
        },
        token_in: a,
        amount_in: real_amount(amount_in)?,
        sqrt_limit_x96: None,
        hops: u8::try_from(path.len() - 1).ok()?,
    })
}

fn swap_router_02(input: &[u8], depth: usize) -> Option<SwapIntent> {
    let (sel, args) = abi::split(input)?;
    if sel == SwapRouter02::exactInputCall::SELECTOR {
        let c = SwapRouter02::exactInputCall::abi_decode_raw(args).ok()?;
        return v3_path_swap(&c.params.path, c.params.amountIn);
    }
    if sel == SwapRouter02::exactInputSingleCall::SELECTOR {
        let p = SwapRouter02::exactInputSingleCall::abi_decode_raw(args)
            .ok()?
            .params;
        return v3_single(
            p.tokenIn,
            p.tokenOut,
            p.fee.to::<u32>(),
            p.amountIn,
            U256::from(p.sqrtPriceLimitX96),
        );
    }
    if sel == SwapRouter02::swapExactTokensForTokensCall::SELECTOR {
        let c = SwapRouter02::swapExactTokensForTokensCall::abi_decode_raw(args).ok()?;
        return v2_path_swap(&c.path, c.amountIn);
    }
    if depth < MAX_DEPTH {
        let calls = if sel == Multicall::multicallCall::SELECTOR {
            Multicall::multicallCall::abi_decode_raw(args).ok()?.data
        } else if sel == MulticallDeadline::multicallCall::SELECTOR {
            MulticallDeadline::multicallCall::abi_decode_raw(args)
                .ok()?
                .data
        } else {
            return None;
        };
        return calls.iter().find_map(|c| swap_router_02(c, depth + 1));
    }
    None
}

fn swap_router_v1(input: &[u8]) -> Option<SwapIntent> {
    let (sel, args) = abi::split(input)?;
    if sel == SwapRouterV1::exactInputCall::SELECTOR {
        let c = SwapRouterV1::exactInputCall::abi_decode_raw(args).ok()?;
        return v3_path_swap(&c.params.path, c.params.amountIn);
    }
    if sel == SwapRouterV1::exactInputSingleCall::SELECTOR {
        let p = SwapRouterV1::exactInputSingleCall::abi_decode_raw(args)
            .ok()?
            .params;
        return v3_single(
            p.tokenIn,
            p.tokenOut,
            p.fee.to::<u32>(),
            p.amountIn,
            U256::from(p.sqrtPriceLimitX96),
        );
    }
    None
}

fn universal_router(input: &[u8]) -> Option<SwapIntent> {
    let (sel, args) = abi::split(input)?;
    let (commands, inputs) = if sel == UniversalRouter::executeCall::SELECTOR {
        let c = UniversalRouter::executeCall::abi_decode_raw(args).ok()?;
        (c.commands, c.inputs)
    } else if sel == UniversalRouterNoDeadline::executeCall::SELECTOR {
        let c = UniversalRouterNoDeadline::executeCall::abi_decode_raw(args).ok()?;
        (c.commands, c.inputs)
    } else {
        return None;
    };
    commands
        .iter()
        .zip(inputs.iter())
        .find_map(|(cmd, data)| match cmd & 0x1f {
            CMD_V3_SWAP_EXACT_IN => {
                let c = UrV3ExactIn::inputCall::abi_decode_raw(data).ok()?;
                v3_path_swap(&c.path, c.amountIn)
            }
            CMD_V2_SWAP_EXACT_IN => {
                let c = UrV2ExactIn::inputCall::abi_decode_raw(data).ok()?;
                v2_path_swap(&c.path, c.amountIn)
            }
            _ => None,
        })
}

pub struct UniswapRouter;

impl Protocol for UniswapRouter {
    fn id(&self) -> &'static str {
        "uniswap_router"
    }

    fn events(&self) -> Vec<mev_protocol::EventKind> {
        Vec::new()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        let swap = if to == SWAP_ROUTER_02 {
            swap_router_02(input, 0)
        } else if to == SWAP_ROUTER_V1 {
            swap_router_v1(input)
        } else if to == UNIVERSAL_ROUTER || to == UNIVERSAL_ROUTER_2 {
            universal_router(input)
        } else {
            None
        };
        swap.map(Decoded::Swap)
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![
            Contract {
                address: SWAP_ROUTER_02,
                name: "Uniswap SwapRouter02 (exact-in swaps and multicall decoded)",
                decoded: true,
            },
            Contract {
                address: SWAP_ROUTER_V1,
                name: "Uniswap V3 SwapRouter v1 (exact-in decoded)",
                decoded: true,
            },
            Contract {
                address: UNIVERSAL_ROUTER,
                name: "Uniswap Universal Router (V2/V3 exact-in commands decoded)",
                decoded: true,
            },
            Contract {
                address: UNIVERSAL_ROUTER_2,
                name: "Universal Router, second deployment (V2/V3 exact-in commands decoded)",
                decoded: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, aliases::U24};

    const A: Address = Address::with_last_byte(0xa1);
    const B: Address = Address::with_last_byte(0xb2);
    const C: Address = Address::with_last_byte(0xc3);

    fn path(tokens: &[Address], fees: &[u32]) -> Vec<u8> {
        let mut p = tokens[0].to_vec();
        for (t, f) in tokens[1..].iter().zip(fees) {
            p.extend_from_slice(&f.to_be_bytes()[1..]);
            p.extend_from_slice(t.as_slice());
        }
        p
    }

    fn swap(d: Option<Decoded>) -> SwapIntent {
        match d {
            Some(Decoded::Swap(s)) => s,
            other => panic!("expected a swap, got {other:?}"),
        }
    }

    #[test]
    fn exact_input_reads_the_first_hop_of_a_single_and_a_multi_hop_path() {
        let one = SwapRouter02::exactInputCall {
            params: SwapRouter02::ExactInputParams {
                path: Bytes::from(path(&[A, B], &[100])),
                recipient: C,
                amountIn: U256::from(5_000u64),
                amountOutMinimum: U256::ZERO,
            },
        };
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &one.abi_encode()));
        assert_eq!(
            &one.abi_encode()[..4],
            [0xb8, 0x58, 0x18, 0x3f],
            "selector of exactInput"
        );
        assert_eq!(
            s.pool,
            PoolRef::Pair {
                token_a: A,
                token_b: B,
                fee: 100
            }
        );
        assert_eq!(
            (s.token_in, s.amount_in, s.hops),
            (A, U256::from(5_000u64), 1)
        );
        let two = SwapRouter02::exactInputCall {
            params: SwapRouter02::ExactInputParams {
                path: Bytes::from(path(&[A, B, C], &[500, 3000])),
                recipient: C,
                amountIn: U256::from(9u64),
                amountOutMinimum: U256::ZERO,
            },
        };
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &two.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::Pair {
                token_a: A,
                token_b: B,
                fee: 500
            },
            "first hop only"
        );
        assert_eq!(s.hops, 2);
    }

    #[test]
    fn exact_input_single_carries_its_price_limit() {
        let c = SwapRouter02::exactInputSingleCall {
            params: SwapRouter02::ExactInputSingleParams {
                tokenIn: A,
                tokenOut: B,
                fee: U24::from(3000u32),
                recipient: C,
                amountIn: U256::from(77u64),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: alloy::primitives::aliases::U160::from(12345u64),
            },
        };
        assert_eq!(&c.abi_encode()[..4], [0x04, 0xe4, 0x5a, 0xaf]);
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &c.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::Pair {
                token_a: A,
                token_b: B,
                fee: 3000
            }
        );
        assert_eq!(s.sqrt_limit_x96, Some(U256::from(12345u64)));
        // A zero limit means no limit.
        let mut z = c.clone();
        z.params.sqrtPriceLimitX96 = Default::default();
        assert_eq!(
            swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &z.abi_encode())).sqrt_limit_x96,
            None
        );
    }

    #[test]
    fn a_v2_style_swap_names_the_pair_and_counts_hops() {
        let c = SwapRouter02::swapExactTokensForTokensCall {
            amountIn: U256::from(1_000u64),
            amountOutMin: U256::ZERO,
            path: vec![A, B, C],
            to: C,
        };
        assert_eq!(&c.abi_encode()[..4], [0x47, 0x2b, 0x43, 0xf3]);
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &c.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::PairV2 {
                token_a: A,
                token_b: B
            }
        );
        assert_eq!(s.hops, 2);
    }

    #[test]
    fn multicall_wrappers_are_unwrapped_to_the_first_swap() {
        let inner = SwapRouter02::exactInputSingleCall {
            params: SwapRouter02::ExactInputSingleParams {
                tokenIn: A,
                tokenOut: B,
                fee: U24::from(500u32),
                recipient: C,
                amountIn: U256::from(42u64),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: Default::default(),
            },
        }
        .abi_encode();
        let noise = vec![0xde, 0xad, 0xbe, 0xef, 0x00];
        let plain = Multicall::multicallCall {
            data: vec![Bytes::from(noise.clone()), Bytes::from(inner.clone())],
        };
        assert_eq!(&plain.abi_encode()[..4], [0xac, 0x96, 0x50, 0xd8]);
        assert_eq!(
            swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &plain.abi_encode())).amount_in,
            U256::from(42u64)
        );
        let dl = MulticallDeadline::multicallCall {
            deadline: U256::from(9u64),
            data: vec![Bytes::from(inner.clone())],
        };
        assert_eq!(&dl.abi_encode()[..4], [0x5a, 0xe4, 0x01, 0xdc]);
        assert_eq!(
            swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &dl.abi_encode())).amount_in,
            U256::from(42u64)
        );
        // A multicall nested deeper than the limit is not followed forever.
        let mut nested = inner;
        for _ in 0..4 {
            nested = Multicall::multicallCall {
                data: vec![Bytes::from(nested)],
            }
            .abi_encode();
        }
        assert!(UniswapRouter.decode_call(SWAP_ROUTER_02, &nested).is_none());
    }

    #[test]
    fn the_router_v1_exact_input_single_has_a_deadline_field() {
        let c = SwapRouterV1::exactInputSingleCall {
            params: SwapRouterV1::ExactInputSingleParams {
                tokenIn: A,
                tokenOut: B,
                fee: U24::from(10000u32),
                recipient: C,
                deadline: U256::from(1_800_000_000u64),
                amountIn: U256::from(300u64),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: Default::default(),
            },
        };
        assert_eq!(&c.abi_encode()[..4], [0x41, 0x4b, 0xf3, 0x89]);
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_V1, &c.abi_encode()));
        assert_eq!((s.amount_in, s.hops), (U256::from(300u64), 1));
        // The same bytes sent to another contract are not decoded.
        assert!(
            UniswapRouter
                .decode_call(SWAP_ROUTER_02, &c.abi_encode())
                .is_none()
        );
    }

    #[test]
    fn the_universal_router_finds_the_first_exact_input_command() {
        let v3 = UrV3ExactIn::inputCall {
            recipient: C,
            amountIn: U256::from(1_000u64),
            amountOutMin: U256::ZERO,
            path: Bytes::from(path(&[A, B], &[500])),
            payerIsUser: true,
        };
        let v3_in = v3.abi_encode()[4..].to_vec();
        let wrap = vec![0u8; 64]; // some other command's input (WRAP_ETH = 0x0b): skipped
        let exec = UniversalRouter::executeCall {
            commands: Bytes::from(vec![0x0b, 0x80 | CMD_V3_SWAP_EXACT_IN]), // second command has the allow-revert bit
            inputs: vec![Bytes::from(wrap), Bytes::from(v3_in)],
            deadline: U256::from(2_000_000_000u64),
        };
        assert_eq!(&exec.abi_encode()[..4], [0x35, 0x93, 0x56, 0x4c]);
        let s = swap(UniswapRouter.decode_call(UNIVERSAL_ROUTER, &exec.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::Pair {
                token_a: A,
                token_b: B,
                fee: 500
            }
        );
        assert_eq!(s.amount_in, U256::from(1_000u64));
        // V2 exact-in via the no-deadline overload.
        let v2 = UrV2ExactIn::inputCall {
            recipient: C,
            amountIn: U256::from(55u64),
            amountOutMin: U256::ZERO,
            path: vec![A, B],
            payerIsUser: false,
        };
        let exec2 = UniversalRouterNoDeadline::executeCall {
            commands: Bytes::from(vec![CMD_V2_SWAP_EXACT_IN]),
            inputs: vec![Bytes::from(v2.abi_encode()[4..].to_vec())],
        };
        assert_eq!(&exec2.abi_encode()[..4], [0x24, 0x85, 0x6b, 0xc3]);
        let s = swap(UniswapRouter.decode_call(UNIVERSAL_ROUTER, &exec2.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::PairV2 {
                token_a: A,
                token_b: B
            }
        );
    }

    #[test]
    fn router_balance_sentinels_are_not_amounts() {
        let v3 = UrV3ExactIn::inputCall {
            recipient: C,
            amountIn: U256::from(1u8) << 255, // CONTRACT_BALANCE
            amountOutMin: U256::ZERO,
            path: Bytes::from(path(&[A, B], &[500])),
            payerIsUser: false,
        };
        let exec = UniversalRouter::executeCall {
            commands: Bytes::from(vec![CMD_V3_SWAP_EXACT_IN]),
            inputs: vec![Bytes::from(v3.abi_encode()[4..].to_vec())],
            deadline: U256::ZERO,
        };
        assert!(
            UniswapRouter
                .decode_call(UNIVERSAL_ROUTER, &exec.abi_encode())
                .is_none()
        );
    }

    #[test]
    fn paths_that_do_not_fit_the_pattern_are_rejected() {
        assert_eq!(first_hop(&[0u8; 42]), None);
        assert_eq!(first_hop(&[0u8; 44]), None);
        assert!(first_hop(&[0u8; 43]).is_some());
        assert_eq!(first_hop(&[0u8; 66]).map(|h| h.3), Some(2));
    }
}
