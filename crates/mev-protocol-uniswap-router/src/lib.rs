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
    interface SwapRouterV1Out {
        struct ExactOutputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 deadline; uint256 amountOut; uint256 amountInMaximum; uint160 sqrtPriceLimitX96; }
        function exactOutputSingle(ExactOutputSingleParams params) external payable returns (uint256 amountIn);
    }
    interface SwapRouter02Out {
        struct ExactOutputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountOut; uint256 amountInMaximum; uint160 sqrtPriceLimitX96; }
        function exactOutputSingle(ExactOutputSingleParams params) external payable returns (uint256 amountIn);
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
const CMD_V4_SWAP: u8 = 0x10;
/// Uniswap V4 router action ids inside a `V4_SWAP` command (v4-periphery `Actions`).
const V4_SWAP_EXACT_IN_SINGLE: u8 = 0x06;
const V4_SWAP_EXACT_IN: u8 = 0x07;
const V4_SETTLE: u8 = 0x0b;
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
        amount_out_exact: None,
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
        amount_out_exact: None,
    })
}

/// An exact-output single-pool swap: the call fixes `amount_out`; `amount_in_max` is only the most it may spend.
fn v3_single_out(
    (token_in, token_out, fee): (Address, Address, u32),
    amount_out: U256,
    amount_in_max: U256,
    sqrt_limit: U256,
) -> Option<SwapIntent> {
    Some(SwapIntent {
        pool: PoolRef::Pair {
            token_a: token_in,
            token_b: token_out,
            fee,
        },
        token_in,
        amount_in: real_amount(amount_in_max)?,
        sqrt_limit_x96: limit(sqrt_limit),
        hops: 1,
        amount_out_exact: Some(real_amount(amount_out)?).filter(|a| !a.is_zero()),
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
        amount_out_exact: None,
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
    if sel == SwapRouter02Out::exactOutputSingleCall::SELECTOR {
        let p = SwapRouter02Out::exactOutputSingleCall::abi_decode_raw(args)
            .ok()?
            .params;
        return v3_single_out(
            (p.tokenIn, p.tokenOut, p.fee.to::<u32>()),
            p.amountOut,
            p.amountInMaximum,
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
    if sel == SwapRouterV1Out::exactOutputSingleCall::SELECTOR {
        let p = SwapRouterV1Out::exactOutputSingleCall::abi_decode_raw(args)
            .ok()?
            .params;
        return v3_single_out(
            (p.tokenIn, p.tokenOut, p.fee.to::<u32>()),
            p.amountOut,
            p.amountInMaximum,
            U256::from(p.sqrtPriceLimitX96),
        );
    }
    None
}

/// Every exact-input swap in a Universal Router `execute`, in command order: V3 and V2 exact-in commands and the
/// exact-in swaps inside `V4_SWAP` commands. Exact-output swaps and swaps that spend the router's own balance are not
/// read (their input amount is not in the calldata).
fn universal_router(input: &[u8]) -> Vec<SwapIntent> {
    let Some((sel, args)) = abi::split(input) else {
        return Vec::new();
    };
    let (commands, inputs) = if sel == UniversalRouter::executeCall::SELECTOR {
        match UniversalRouter::executeCall::abi_decode_raw(args) {
            Ok(c) => (c.commands, c.inputs),
            Err(_) => return Vec::new(),
        }
    } else if sel == UniversalRouterNoDeadline::executeCall::SELECTOR {
        match UniversalRouterNoDeadline::executeCall::abi_decode_raw(args) {
            Ok(c) => (c.commands, c.inputs),
            Err(_) => return Vec::new(),
        }
    } else {
        return Vec::new();
    };
    commands
        .iter()
        .zip(inputs.iter())
        .flat_map(|(cmd, data)| -> Vec<SwapIntent> {
            match cmd & 0x1f {
                CMD_V3_SWAP_EXACT_IN => UrV3ExactIn::inputCall::abi_decode_raw(data)
                    .ok()
                    .and_then(|c| v3_path_swap(&c.path, c.amountIn))
                    .into_iter()
                    .collect(),
                CMD_V2_SWAP_EXACT_IN => UrV2ExactIn::inputCall::abi_decode_raw(data)
                    .ok()
                    .and_then(|c| v2_path_swap(&c.path, c.amountIn))
                    .into_iter()
                    .collect(),
                CMD_V4_SWAP => v4_swaps(data),
                _ => Vec::new(),
            }
        })
        .collect()
}

sol! {
    interface UrV4Swap {
        function input(bytes actions, bytes[] params) external;
    }
    struct V4PoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct V4ExactInSingle { V4PoolKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    function v4ExactInSingle(V4ExactInSingle params) external;
}

/// The word at byte `at` of `data`, if it is all there.
fn word_at(data: &[u8], at: usize) -> Option<U256> {
    data.get(at..at.checked_add(32)?).map(U256::from_be_slice)
}

fn addr_at(data: &[u8], at: usize) -> Option<Address> {
    let w = data.get(at..at.checked_add(32)?)?;
    w[..12]
        .iter()
        .all(|b| *b == 0)
        .then(|| Address::from_slice(&w[12..]))
}

fn usize_at(data: &[u8], at: usize) -> Option<usize> {
    usize::try_from(word_at(data, at)?)
        .ok()
        .filter(|v| *v < (1 << 24))
}

/// A V4 first hop: the pool's stand-in address (see `mev_protocol::v4`), swapping `token_in` for the other currency.
fn v4_hop(
    token_in: Address,
    other: Address,
    (fee, tick_spacing, hooks): (u32, i32, Address),
    amount_in: U256,
    hops: u8,
) -> Option<SwapIntent> {
    let (c0, c1) = if token_in < other {
        (token_in, other)
    } else {
        (other, token_in)
    };
    let id = mev_protocol::v4::pool_id(c0, c1, fee, tick_spacing, hooks);
    Some(SwapIntent {
        pool: PoolRef::Address(mev_protocol::v4::stand_in(id)),
        token_in,
        amount_in: real_amount(amount_in)?,
        sqrt_limit_x96: None,
        hops,
        amount_out_exact: None,
    })
}

/// `SWAP_EXACT_IN_SINGLE`: `abi.encode(ExactInputSingleParams)`.
fn v4_exact_in_single(p: &[u8], settled: &[(Address, U256)]) -> Option<SwapIntent> {
    let c = v4ExactInSingleCall::abi_decode_raw(p).ok()?;
    let k = &c.params.poolKey;
    let token_in = if c.params.zeroForOne {
        k.currency0
    } else {
        k.currency1
    };
    let other = if c.params.zeroForOne {
        k.currency1
    } else {
        k.currency0
    };
    let mut amount = U256::from(c.params.amountIn);
    if amount.is_zero() {
        // OPEN_DELTA: swap whatever the earlier SETTLE paid in.
        amount = settled.iter().find(|(cur, _)| *cur == token_in)?.1;
    }
    v4_hop(
        token_in,
        other,
        (k.fee.to::<u32>(), k.tickSpacing.as_i32(), k.hooks),
        amount,
        1,
    )
}

/// `SWAP_EXACT_IN` (multi-hop): `abi.encode(ExactInputParams)`, whose first hop is read. Two layouts are live: the
/// first deployment has `(currencyIn, path, amountIn, amountOutMinimum)`, the newer one adds a `maxHopSlippage` array
/// after `path`; the offset of `path` (4 or 5 head words) tells them apart.
fn v4_exact_in(p: &[u8], settled: &[(Address, U256)]) -> Option<SwapIntent> {
    if usize_at(p, 0)? != 32 {
        return None;
    }
    let s = 32;
    let currency_in = addr_at(p, s)?;
    let path_off = usize_at(p, s + 32)?;
    let amount_at = match path_off {
        128 => s + 64,
        160 => s + 96,
        _ => return None,
    };
    let mut amount = word_at(p, amount_at)?;
    let path = s + path_off;
    let n = usize_at(p, path)?;
    if n == 0 || n > 8 {
        return None;
    }
    // Elements are dynamic structs (they end in `bytes hookData`), so each is reached through an offset.
    let el = path + 32 + usize_at(p, path + 32)?;
    let intermediate = addr_at(p, el)?;
    let fee = u32::try_from(word_at(p, el + 32)?).ok()?;
    let spacing_word = p.get(el + 64..el + 96)?;
    let tick_spacing = i32::from_be_bytes(spacing_word[28..32].try_into().ok()?);
    let hooks = addr_at(p, el + 96)?;
    if amount.is_zero() {
        amount = settled.iter().find(|(cur, _)| *cur == currency_in)?.1;
    }
    v4_hop(
        currency_in,
        intermediate,
        (fee, tick_spacing, hooks),
        amount,
        u8::try_from(n).ok()?,
    )
}

/// Every exact-input swap inside a `V4_SWAP` command's input (`abi.encode(bytes actions, bytes[] params)`).
fn v4_swaps(data: &[u8]) -> Vec<SwapIntent> {
    let Ok(c) = UrV4Swap::inputCall::abi_decode_raw(data) else {
        return Vec::new();
    };
    // Amounts the actions settle first, for swaps that use "whatever was settled".
    let settled: Vec<(Address, U256)> = c
        .actions
        .iter()
        .zip(c.params.iter())
        .filter(|(a, _)| **a == V4_SETTLE)
        .filter_map(|(_, p)| Some((addr_at(p, 0)?, word_at(p, 32)?)))
        .collect();
    c.actions
        .iter()
        .zip(c.params.iter())
        .filter_map(|(a, p)| match *a {
            V4_SWAP_EXACT_IN_SINGLE => v4_exact_in_single(p, &settled),
            V4_SWAP_EXACT_IN => v4_exact_in(p, &settled),
            _ => None,
        })
        .collect()
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
            universal_router(input).into_iter().next()
        } else {
            None
        };
        swap.map(Decoded::Swap)
    }

    fn decode_calls(&self, to: Address, input: &[u8]) -> Vec<Decoded> {
        if to == UNIVERSAL_ROUTER || to == UNIVERSAL_ROUTER_2 {
            universal_router(input)
                .into_iter()
                .map(Decoded::Swap)
                .collect()
        } else {
            self.decode_call(to, input).into_iter().collect()
        }
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
                name: "Uniswap Universal Router (V2/V3/V4 exact-in commands decoded)",
                decoded: true,
            },
            Contract {
                address: UNIVERSAL_ROUTER_2,
                name: "Universal Router, second deployment (V2/V3/V4 exact-in commands decoded)",
                decoded: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, aliases::U24};

    #[test]
    fn landed_exact_output_calls_fix_the_output_and_bound_the_input() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/exact_output_single_v1.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = alloy::hex::decode(f[0].trim_start_matches("0x")).unwrap();
            let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_V1, &input));
            let (tin, tout): (Address, Address) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            assert_eq!(s.token_in, tin, "{l}");
            assert_eq!(
                s.pool,
                PoolRef::Pair {
                    token_a: tin,
                    token_b: tout,
                    fee: f[3].parse().unwrap()
                },
                "{l}"
            );
            assert_eq!(
                s.amount_out_exact,
                Some(f[4].parse::<U256>().unwrap()),
                "{l}"
            );
            assert_eq!(s.amount_in, f[5].parse::<U256>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 35, "{n}");
    }

    #[test]
    fn swap_router_02_exact_output_single_decodes_and_exact_input_stays_exact() {
        let c = SwapRouter02Out::exactOutputSingleCall {
            params: SwapRouter02Out::ExactOutputSingleParams {
                tokenIn: A,
                tokenOut: B,
                fee: U24::from(500u32),
                recipient: C,
                amountOut: U256::from(1_000u32),
                amountInMaximum: U256::from(1_200u32),
                sqrtPriceLimitX96: Default::default(),
            },
        };
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &c.abi_encode()));
        assert_eq!(s.amount_out_exact, Some(U256::from(1_000u32)));
        assert_eq!(s.amount_in, U256::from(1_200u32));
        assert_eq!(s.token_in, A);
        // Exact-input calls carry no fixed output.
        let one = SwapRouter02::exactInputSingleCall {
            params: SwapRouter02::ExactInputSingleParams {
                tokenIn: A,
                tokenOut: B,
                fee: U24::from(500u32),
                recipient: C,
                amountIn: U256::from(7u32),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: Default::default(),
            },
        };
        let s = swap(UniswapRouter.decode_call(SWAP_ROUTER_02, &one.abi_encode()));
        assert_eq!(s.amount_out_exact, None);
    }

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

    /// Real Universal Router `V4_SWAP` transactions (three layouts: the newer deployment's multi-hop with
    /// `maxHopSlippage`, the first deployment's without it, and a SETTLE-first bundle whose swap is not param 0). The
    /// expected pool is the id the chain's own V4 `Swap` log names for the first hop.
    #[test]
    fn real_universal_router_v4_swaps_name_the_pool_the_chain_logged() {
        let rows = include_str!("../tests/fixtures/ur_v4_rows.csv");
        let mut n = 0;
        for row in rows.lines().skip(1) {
            let f: Vec<&str> = row.split(',').collect();
            let to: Address = f[1].parse().unwrap();
            let input = alloy::hex::decode(f[2].trim_start_matches("0x")).unwrap();
            let id: alloy::primitives::B256 = f[3].parse().unwrap();
            let swaps = UniswapRouter.decode_calls(to, &input);
            let first = match swaps.first() {
                Some(Decoded::Swap(s)) => s,
                other => panic!("{}: not decoded: {other:?}", f[0]),
            };
            assert_eq!(
                first.pool,
                PoolRef::Address(mev_protocol::v4::stand_in(id)),
                "{}",
                f[0]
            );
            assert!(first.amount_in > U256::ZERO, "{}", f[0]);
            assert_eq!(
                UniswapRouter.decode_call(to, &input),
                swaps.first().cloned(),
                "decode_call is the first of decode_calls"
            );
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn the_known_amounts_of_the_real_transactions_are_read() {
        let rows = include_str!("../tests/fixtures/ur_v4_rows.csv");
        let amounts: Vec<U256> = rows
            .lines()
            .skip(1)
            .map(|row| {
                let f: Vec<&str> = row.split(',').collect();
                let input = alloy::hex::decode(f[2].trim_start_matches("0x")).unwrap();
                match UniswapRouter
                    .decode_calls(f[1].parse().unwrap(), &input)
                    .first()
                {
                    Some(Decoded::Swap(s)) => s.amount_in,
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        // Read off the chain: 110,000,000 (6-decimal token), 0.43 ETH, 78,974,100 (settled by an earlier action).
        assert_eq!(
            amounts,
            vec![
                U256::from(110_000_000u64),
                U256::from(430_000_000_000_000_000u64),
                U256::from(78_974_100u64)
            ]
        );
    }

    #[test]
    fn malformed_v4_params_decode_nothing_and_never_panic() {
        // Truncations and garbage of a real input, at every length.
        let rows = include_str!("../tests/fixtures/ur_v4_rows.csv");
        let f: Vec<&str> = rows.lines().nth(1).unwrap().split(',').collect();
        let input = alloy::hex::decode(f[2].trim_start_matches("0x")).unwrap();
        let to: Address = f[1].parse().unwrap();
        for cut in (0..input.len()).step_by(7) {
            let _ = UniswapRouter.decode_calls(to, &input[..cut]);
        }
        let mut junk = input.clone();
        for b in junk.iter_mut().skip(200) {
            *b = 0xff;
        }
        assert!(
            UniswapRouter.decode_calls(to, &junk).is_empty(),
            "a huge offset or amount decodes nothing"
        );
        assert!(v4_swaps(&[0u8; 10]).is_empty());
    }

    #[test]
    fn a_v4_single_hop_swap_reads_direction_amount_and_pool_and_one_command_can_hold_two() {
        let (c0, c1) = (Address::with_last_byte(0x10), Address::with_last_byte(0x20));
        let hooks = Address::with_last_byte(0x99);
        let single = |zero_for_one: bool, amount: u128| {
            v4ExactInSingleCall {
                params: V4ExactInSingle {
                    poolKey: V4PoolKey {
                        currency0: c0,
                        currency1: c1,
                        fee: alloy::primitives::Uint::<24, 1>::from(3000u32),
                        tickSpacing: alloy::primitives::Signed::<24, 1>::try_from(60i32).unwrap(),
                        hooks,
                    },
                    zeroForOne: zero_for_one,
                    amountIn: amount,
                    amountOutMinimum: 0,
                    hookData: Default::default(),
                },
            }
            .abi_encode()[4..]
                .to_vec()
        };
        let v4 = UrV4Swap::inputCall {
            actions: vec![V4_SWAP_EXACT_IN_SINGLE, V4_SWAP_EXACT_IN_SINGLE].into(),
            params: vec![single(true, 1_000).into(), single(false, 2_000).into()],
        }
        .abi_encode()[4..]
            .to_vec();
        let exec = UniversalRouterNoDeadline::executeCall {
            commands: Bytes::from(vec![CMD_V4_SWAP]),
            inputs: vec![v4.into()],
        };
        let swaps = UniswapRouter.decode_calls(UNIVERSAL_ROUTER, &exec.abi_encode());
        assert_eq!(swaps.len(), 2, "both swaps of the command, in order");
        let id = mev_protocol::v4::pool_id(c0, c1, 3000, 60, hooks);
        let want = |token_in, amount: u64| {
            Decoded::Swap(SwapIntent {
                pool: PoolRef::Address(mev_protocol::v4::stand_in(id)),
                token_in,
                amount_in: U256::from(amount),
                sqrt_limit_x96: None,
                hops: 1,
                amount_out_exact: None,
            })
        };
        assert_eq!(swaps, vec![want(c0, 1_000), want(c1, 2_000)]);
    }
}
