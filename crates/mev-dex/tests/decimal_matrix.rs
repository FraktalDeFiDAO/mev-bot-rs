//! Single-swap matrix: every exchange model × token-decimal pair × price × depth × direction × size.
//!
//! Each pool is built from the same human-unit economics (reserves in whole tokens, a price in whole
//! token1 per whole token0) at every decimal pair from 0 to 24, then every swap is checked three ways:
//!
//! 1. **Reference**: the integer output equals an independent re-implementation of the contract's math,
//!    written here in 512-bit integers straight from the Solidity (Uniswap V2 / Camelot / Solidly
//!    `getAmountOut`; `SwapMath.computeSwapStep`, `SqrtPriceMath`, and `nextInitializedTickWithinOneWord`
//!    for every V3-style pool, including the 256-bit overflow branches that extreme decimal gaps reach).
//! 2. **Screening model**: the `f64` model the search uses agrees with the integer output, so a decimal
//!    gap can neither hide an opportunity nor invent one.
//! 3. **Decimal invariance**: the same trade in whole tokens gives the same whole-token output at every
//!    decimal pair, up to one raw unit of the output token's rounding.
//!
//! Ground truth against the deployed contracts is in `mev-discovery/tests/exact_*_live.rs`.

use alloy::primitives::{B256, U256, U512};
use mev_dex::{ExactState, Pool, PoolState, TickMap, exact};
use mev_types::{Address, PoolInfo, PoolKind};

const T0: Address = Address::with_last_byte(0x10);
const T1: Address = Address::with_last_byte(0x20);

/// Every decimal count a real token uses on the chains we trade (0: some memecoins, 2: EURS-style,
/// 6: USDC/USDT, 8: WBTC/cbBTC, 9: some bridged, 12, 18: most, 24: a few rebasing/yield tokens).
const DECIMALS: [u32; 8] = [0, 2, 6, 8, 9, 12, 18, 24];
/// Human price of token0 in token1, as `m × 10^e`: 1e-8, 0.025, 1, 3127.45, 1e6.
const PRICES: [(u128, i32); 5] = [(1, -8), (25, -3), (1, 0), (312_745, -2), (1, 6)];
/// Human depth: whole token0 in the pool.
const DEPTHS: [u32; 2] = [4, 8];
/// Trade size as a fraction of the input-side reserve, `n / 10^7`.
const FRACTIONS: [u64; 6] = [1, 1_000, 10_000, 100_000, 1_000_000, 5_000_000];
/// Raw reserves stay within what a pool can hold (Uniswap V2 reserves are uint112 ≈ 5.2e33) and above dust.
const MIN_RAW_EXP: i32 = 6;
const MAX_RAW_EXP: i32 = 32;

fn pow10(e: u32) -> U256 {
    U256::from(10u8).pow(U256::from(e))
}

fn log10(m: u128) -> i32 {
    (m as f64).log10().floor() as i32
}

/// A market: raw reserves of token0 and token1 for `10^depth` whole token0 at `price`.
#[derive(Clone, Copy, Debug)]
struct Market {
    d0: u32,
    d1: u32,
    price: (u128, i32),
    depth: u32,
    r0: U256,
    r1: U256,
}

fn market(d0: u32, d1: u32, price: (u128, i32), depth: u32) -> Option<Market> {
    let (m, e) = price;
    let e0 = depth as i32 + d0 as i32;
    let e1 = depth as i32 + d1 as i32 + e;
    let top1 = e1 + log10(m);
    if e1 < 0 || e0 < MIN_RAW_EXP || top1 < MIN_RAW_EXP || e0 > MAX_RAW_EXP || top1 > MAX_RAW_EXP {
        return None; // such a pool cannot exist with sane depth: not a market
    }
    Some(Market {
        d0,
        d1,
        price,
        depth,
        r0: pow10(e0 as u32),
        r1: U256::from(m) * pow10(e1 as u32),
    })
}

#[derive(Clone, Copy, Debug)]
enum Model {
    V2 {
        fee_bps: u16,
    },
    Camelot {
        fee0: u32,
        fee1: u32,
    },
    Solidly {
        fee_bps: u32,
    },
    UniV3 {
        fee: u32,
    },
    Pancake {
        fee: u32,
    },
    Slipstream {
        fee: u32,
        spacing: u32,
    },
    V4 {
        fee_zfo: u32,
        fee_ofz: u32,
        spacing: u32,
        flipped: bool,
    },
    Algebra {
        max_zfo: u32,
        max_ofz: u32,
    },
}

const MODELS: [Model; 12] = [
    Model::V2 { fee_bps: 30 },
    Model::V2 { fee_bps: 5 },
    Model::Camelot {
        fee0: 300,
        fee1: 150,
    },
    Model::Solidly { fee_bps: 30 },
    Model::Solidly { fee_bps: 1 },
    Model::UniV3 { fee: 500 },
    Model::UniV3 { fee: 3000 },
    Model::Pancake { fee: 2500 },
    Model::Slipstream {
        fee: 684,
        spacing: 100,
    },
    Model::V4 {
        fee_zfo: 3000,
        fee_ofz: 3500,
        spacing: 60,
        flipped: false,
    },
    Model::V4 {
        fee_zfo: 500,
        fee_ofz: 700,
        spacing: 10,
        flipped: true,
    },
    Model::Algebra {
        max_zfo: 2900,
        max_ofz: 3100,
    },
];

// ---------------------------------------------------------------------------------------------
// Reference math (U512, transcribed from the contracts; shares nothing with mev-dex but TickMath).
// ---------------------------------------------------------------------------------------------

fn w(x: U256) -> U512 {
    U512::from(x)
}

fn n(x: U512) -> Option<U256> {
    (x <= U512::from(U256::MAX)).then(|| U256::from(x))
}

fn q96() -> U512 {
    U512::from(1u8) << 96
}

fn fits256(x: U512) -> bool {
    x <= U512::from(U256::MAX)
}

fn mul_div(a: U512, b: U512, d: U512) -> Option<U512> {
    let r = a * b / d;
    fits256(r).then_some(r)
}

fn mul_div_up(a: U512, b: U512, d: U512) -> Option<U512> {
    let p = a * b;
    let r = p / d + U512::from(u8::from(!(p % d).is_zero()));
    fits256(r).then_some(r)
}

fn div_up(a: U512, d: U512) -> U512 {
    a / d + U512::from(u8::from(!(a % d).is_zero()))
}

fn isqrt(x: U512) -> U512 {
    if x.is_zero() {
        return x;
    }
    let mut r = U512::from(1u8) << (x.bit_len().div_ceil(2));
    loop {
        let next = (r + x / r) >> 1;
        if next >= r {
            return r;
        }
        r = next;
    }
}

/// `SqrtPriceMath.getAmount0Delta`.
fn amount0_delta(a: U512, b: U512, l: U512, up: bool) -> Option<U512> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    let n1 = l << 96;
    let n2 = b - a;
    if up {
        Some(div_up(mul_div_up(n1, n2, b)?, a))
    } else {
        Some(mul_div(n1, n2, b)? / a)
    }
}

/// `SqrtPriceMath.getAmount1Delta`.
fn amount1_delta(a: U512, b: U512, l: U512, up: bool) -> Option<U512> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    if up {
        mul_div_up(l, b - a, q96())
    } else {
        mul_div(l, b - a, q96())
    }
}

/// `SqrtPriceMath.getNextSqrtPriceFromInput`, with the 256-bit overflow checks of the original.
fn next_from_input(p: U512, l: U512, amount: U512, zfo: bool) -> Option<U512> {
    if zfo {
        // getNextSqrtPriceFromAmount0RoundingUp(add = true)
        if amount.is_zero() {
            return Some(p);
        }
        let n1 = l << 96;
        let product = amount * p;
        if fits256(product) {
            let denominator = n1 + product;
            if fits256(denominator) {
                return mul_div_up(n1, p, denominator);
            }
        }
        Some(div_up(n1, n1 / p + amount))
    } else {
        // getNextSqrtPriceFromAmount1RoundingDown(add = true)
        let quotient = if amount <= (U512::from(1u8) << 160) - U512::from(1u8) {
            (amount << 96) / l
        } else {
            mul_div(amount, q96(), l)?
        };
        let r = p + quotient;
        (r < (U512::from(1u8) << 160)).then_some(r)
    }
}

struct RefStep {
    next: U512,
    amount_in: U512,
    amount_out: U512,
    fee: U512,
}

/// `SwapMath.computeSwapStep`, exact input.
fn compute_step(cur: U512, target: U512, l: U512, remaining: U512, fee: u32) -> Option<RefStep> {
    let zfo = cur >= target;
    let e6 = U512::from(1_000_000u32);
    let fee = U512::from(fee);
    let less_fee = mul_div(remaining, e6 - fee, e6)?;
    let mut amount_in = if zfo {
        amount0_delta(target, cur, l, true)?
    } else {
        amount1_delta(cur, target, l, true)?
    };
    let next = if less_fee >= amount_in {
        target
    } else {
        next_from_input(cur, l, less_fee, zfo)?
    };
    let max = next == target;
    if !max {
        amount_in = if zfo {
            amount0_delta(next, cur, l, true)?
        } else {
            amount1_delta(cur, next, l, true)?
        };
    }
    let amount_out = if zfo {
        amount1_delta(next, cur, l, false)?
    } else {
        amount0_delta(cur, next, l, false)?
    };
    let fee_amount = if !max {
        remaining - amount_in
    } else {
        mul_div_up(amount_in, fee, e6 - fee)?
    };
    Some(RefStep {
        next,
        amount_in,
        amount_out,
        fee: fee_amount,
    })
}

/// The pool's swap loop for exact input, in the pool's own orientation. `word_spacing` compresses the
/// bitmap (V3/V4: the tick spacing; Algebra: 1). Returns `None` when the loaded window `[lo, hi]` runs
/// out before the input does (the bot refuses to quote that, and so must the reference).
#[allow(clippy::too_many_arguments)]
fn ref_swap(
    sqrt: U256,
    tick: i32,
    liquidity: u128,
    fee: u32,
    word_spacing: i32,
    ticks: &[(i32, i128)],
    lo: i32,
    hi: i32,
    zfo: bool,
    amount: U256,
) -> Option<(U256, usize)> {
    let (mut s, mut tick, mut l) = (w(sqrt), tick, liquidity as i128);
    let mut steps = 0usize;
    let mut remaining = w(amount);
    let mut out = U512::ZERO;
    let initialized = |t: i32| ticks.iter().find(|(k, _)| *k == t).map(|(_, n)| *n);
    while !remaining.is_zero() {
        // nextInitializedTickWithinOneWord
        let compressed = tick.div_euclid(word_spacing);
        let (mut next, net) = if zfo {
            let word_start = compressed - compressed.rem_euclid(256);
            let found = (word_start..=compressed)
                .rev()
                .map(|c| c * word_spacing)
                .find_map(|t| initialized(t).map(|n| (t, n)));
            match found {
                Some((t, n)) => (t, Some(n)),
                None => (word_start * word_spacing, None),
            }
        } else {
            let c1 = compressed + 1;
            let word_end = c1 - c1.rem_euclid(256) + 255;
            let found = (c1..=word_end)
                .map(|c| c * word_spacing)
                .find_map(|t| initialized(t).map(|n| (t, n)));
            match found {
                Some((t, n)) => (t, Some(n)),
                None => (word_end * word_spacing, None),
            }
        };
        let mut edge = false;
        if zfo && next < lo {
            (next, edge) = (lo, true);
        }
        if !zfo && next > hi {
            (next, edge) = (hi, true);
        }
        let net = if edge { None } else { net };
        let next = next.clamp(exact::MIN_TICK, exact::MAX_TICK);
        let target = w(exact::sqrt_ratio_at_tick(next)?);
        if l <= 0 {
            return None; // no liquidity: not a quotable trade in this matrix
        }
        let st = compute_step(s, target, U512::from(l as u128), remaining, fee)?;
        steps += 1;
        remaining -= st.amount_in + st.fee;
        out += st.amount_out;
        s = st.next;
        if s == target {
            if edge {
                return remaining
                    .is_zero()
                    .then(|| n(out).map(|o| (o, steps)))
                    .flatten();
            }
            if let Some(nn) = net {
                l += if zfo { -nn } else { nn };
            }
            tick = if zfo { next - 1 } else { next };
        } else {
            tick = exact::tick_at_sqrt_ratio(n(s)?)?;
        }
    }
    n(out).map(|o| (o, steps))
}

/// `UniswapV2Library.getAmountOut` and Camelot's `_getAmountOut` (fee out of `den`).
fn ref_v2(a: U256, rin: U256, rout: U256, fee: u32, den: u32) -> Option<U256> {
    if a.is_zero() || rin.is_zero() || rout.is_zero() {
        return None;
    }
    let with_fee = w(a) * U512::from(den - fee);
    n(with_fee * w(rout) / (w(rin) * U512::from(den) + with_fee))
}

/// Solidly volatile `getAmountOut`: the fee is taken from the input first, in basis points.
fn ref_solidly(a: U256, rin: U256, rout: U256, fee_bps: u32) -> Option<U256> {
    if a.is_zero() || rin.is_zero() || rout.is_zero() {
        return None;
    }
    let net = w(a) - w(a) * U512::from(fee_bps) / U512::from(10_000u32);
    n(net * w(rout) / (w(rin) + net))
}

// ---------------------------------------------------------------------------------------------
// Pool construction: the same market as the bot's model and as the reference.
// ---------------------------------------------------------------------------------------------

/// Concentrated-liquidity layout around a price, in the pool's own orientation.
struct Layout {
    sqrt: U256,
    tick: i32,
    liquidity: u128,
    ticks: Vec<(i32, i128)>,
    lo: i32,
    hi: i32,
}

/// Liquidity `sqrt(r0·r1)` at price `r1/r0` (both raw), plus two positions ending on each side so a
/// large swap crosses initialized ticks, inside a window several bitmap words wide.
fn layout(r0: U256, r1: U256, spacing: i32, table_spacing: i32, positions: bool) -> Option<Layout> {
    let sqrt = n(isqrt((w(r1) << 192) / w(r0)))?;
    let liquidity = u128::try_from(n(isqrt(w(r0) * w(r1)))?).ok()?;
    if liquidity < 4 {
        return None;
    }
    let tick = exact::tick_at_sqrt_ratio(sqrt)?;
    let align = |t: i32| t.div_euclid(spacing) * spacing;
    let q = (liquidity / 4) as i128;
    let below1 = align(tick) - 40 * spacing;
    let below2 = align(tick) - 400 * spacing;
    let above1 = align(tick) + 40 * spacing;
    let above2 = align(tick) + 400 * spacing;
    let lo =
        (align(tick) - 3 * 256 * table_spacing.max(spacing)).max(align(exact::MIN_TICK) + spacing);
    let hi =
        (align(tick) + 3 * 256 * table_spacing.max(spacing)).min(align(exact::MAX_TICK) - spacing);
    let mut ticks: Vec<(i32, i128)> = [(below2, q), (below1, q), (above1, -q), (above2, -q)]
        .into_iter()
        .filter(|(t, _)| positions && *t > lo && *t < hi)
        .collect();
    ticks.sort_by_key(|(t, _)| *t);
    Some(Layout {
        sqrt,
        tick,
        liquidity,
        ticks,
        lo,
        hi,
    })
}

fn x96_to_f64(x: U256) -> f64 {
    f64::from(x) / 2f64.powi(96)
}

fn to_f64(x: U256) -> f64 {
    f64::from(x)
}

fn info(kind: PoolKind) -> PoolInfo {
    PoolInfo {
        address: Address::with_last_byte(0xee),
        dex: "matrix".into(),
        kind,
        token0: T0,
        token1: T1,
    }
}

/// What the matrix checks a pool against: the reference quote for (token_in, amount), with the number
/// of swap steps the contract takes (each step rounds its input and fee by up to a raw unit).
type Reference = Box<dyn Fn(Address, U256) -> Option<(U256, usize)>>;

/// `positions`: add the initialized ticks; without them liquidity is uniform across the window.
fn build(model: Model, mk: &Market, positions: bool) -> Option<(Pool, Reference)> {
    let (r0, r1) = (mk.r0, mk.r1);
    let v2_state = PoolState::V2 {
        reserve0: to_f64(r0),
        reserve1: to_f64(r1),
    };
    let cl = |kind: PoolKind, spacing: i32, fee: u32, table: i32| -> Option<(Pool, Reference)> {
        let lay = layout(r0, r1, spacing, table, positions)?;
        let pool = Pool::new(
            info(kind),
            PoolState::V3 {
                sqrt_price: x96_to_f64(lay.sqrt),
                liquidity: lay.liquidity as f64,
            },
        );
        let map = TickMap {
            ticks: lay.ticks.iter().map(|&(t, n)| (t, n as f64)).collect(),
            exact: lay.ticks.clone(),
            lo: lay.lo,
            hi: lay.hi,
            hint: Some((x96_to_f64(lay.sqrt), lay.tick)),
        };
        let exact_state = if matches!(kind, PoolKind::AlgebraV3) {
            let Model::Algebra { max_zfo, max_ofz } = model else {
                unreachable!()
            };
            ExactState::Algebra {
                sqrt_price: lay.sqrt,
                tick: lay.tick,
                liquidity: lay.liquidity,
                max_fee_zero_for_one: max_zfo,
                max_fee_one_for_zero: max_ofz,
            }
        } else {
            ExactState::V3 {
                sqrt_price: lay.sqrt,
                tick: lay.tick,
                liquidity: lay.liquidity,
            }
        };
        let pool = pool.with_ticks(map).with_exact(exact_state);
        let word = if matches!(kind, PoolKind::AlgebraV3) {
            1
        } else {
            spacing
        };
        let fees = match model {
            Model::Algebra { max_zfo, max_ofz } => (max_zfo, max_ofz),
            _ => (fee, fee),
        };
        let pool = pool.with_directional_fees(f64::from(fees.0) / 1e6, f64::from(fees.1) / 1e6);
        let reference: Reference = Box::new(move |tin, a| {
            let zfo = tin == T0;
            let fee = if zfo { fees.0 } else { fees.1 };
            ref_swap(
                lay.sqrt,
                lay.tick,
                lay.liquidity,
                fee,
                word,
                &lay.ticks,
                lay.lo,
                lay.hi,
                zfo,
                a,
            )
        });
        Some((pool, reference))
    };
    match model {
        Model::V2 { fee_bps } => {
            let pool =
                Pool::new(info(PoolKind::UniV2 { fee_bps }), v2_state).with_exact(ExactState::V2 {
                    reserve0: r0,
                    reserve1: r1,
                });
            Some((
                pool,
                Box::new(move |tin, a| {
                    let (i, o) = if tin == T0 { (r0, r1) } else { (r1, r0) };
                    ref_v2(a, i, o, u32::from(fee_bps), 10_000).map(|o| (o, 1))
                }),
            ))
        }
        Model::Camelot { fee0, fee1 } => {
            let pool = Pool::new(info(PoolKind::CamelotV2), v2_state)
                .with_directional_fees(f64::from(fee0) / 1e5, f64::from(fee1) / 1e5)
                .with_exact(ExactState::V2Fee {
                    reserve0: r0,
                    reserve1: r1,
                    fee0,
                    fee1,
                });
            Some((
                pool,
                Box::new(move |tin, a| {
                    let (i, o, f) = if tin == T0 {
                        (r0, r1, fee0)
                    } else {
                        (r1, r0, fee1)
                    };
                    ref_v2(a, i, o, f, 100_000).map(|o| (o, 1))
                }),
            ))
        }
        Model::Solidly { fee_bps } => {
            let f = f64::from(fee_bps) / 1e4;
            let pool = Pool::new(
                info(PoolKind::SolidlyV2 {
                    factory: Address::with_last_byte(0xfa),
                }),
                v2_state,
            )
            .with_directional_fees(f, f)
            .with_exact(ExactState::V2Fee {
                reserve0: r0,
                reserve1: r1,
                fee0: fee_bps,
                fee1: fee_bps,
            });
            Some((
                pool,
                Box::new(move |tin, a| {
                    let (i, o) = if tin == T0 { (r0, r1) } else { (r1, r0) };
                    ref_solidly(a, i, o, fee_bps).map(|o| (o, 1))
                }),
            ))
        }
        Model::UniV3 { fee } => {
            let k = PoolKind::UniV3 { fee };
            cl(
                k,
                mev_dex::tick_spacing(&k) as i32,
                fee,
                mev_dex::tick_spacing(&k) as i32,
            )
        }
        Model::Pancake { fee } => {
            let k = PoolKind::PancakeV3 { fee };
            cl(
                k,
                mev_dex::tick_spacing(&k) as i32,
                fee,
                mev_dex::tick_spacing(&k) as i32,
            )
        }
        Model::Slipstream { fee, spacing } => cl(
            PoolKind::SlipstreamV3 {
                fee,
                tick_spacing: spacing,
            },
            spacing as i32,
            fee,
            spacing as i32,
        ),
        Model::Algebra { .. } => cl(PoolKind::AlgebraV3, 10, 0, 1),
        Model::V4 {
            fee_zfo,
            fee_ofz,
            spacing,
            flipped,
        } => {
            let sp = spacing as i32;
            // The pool's own orientation: when flipped, its currency0 is our token1.
            let (n0, n1) = if flipped { (r1, r0) } else { (r0, r1) };
            let lay = layout(n0, n1, sp, sp, positions)?;
            let kind = PoolKind::UniV4 {
                fee: fee_zfo,
                tick_spacing: spacing,
                native: false,
                flipped,
                id: B256::with_last_byte(0x44),
                manager: Address::with_last_byte(0x4d),
            };
            // Our view, as discovery builds it (ticks.rs `finish`): mirrored when flipped.
            let (our_ticks, our_lo, our_hi, our_sqrt, our_hint_tick) = if flipped {
                let mut t: Vec<(i32, i128)> = lay.ticks.iter().map(|&(k, n)| (-k, -n)).collect();
                t.sort_by_key(|(k, _)| *k);
                (
                    t,
                    -lay.hi,
                    -lay.lo,
                    1.0 / x96_to_f64(lay.sqrt),
                    -lay.tick - 1,
                )
            } else {
                (
                    lay.ticks.clone(),
                    lay.lo,
                    lay.hi,
                    x96_to_f64(lay.sqrt),
                    lay.tick,
                )
            };
            // Our fee0 is the fee for our token0 as input: the pool's one-for-zero fee when flipped.
            let (our_fee0, our_fee1) = if flipped {
                (fee_ofz, fee_zfo)
            } else {
                (fee_zfo, fee_ofz)
            };
            let pool = Pool::new(
                info(kind),
                PoolState::V3 {
                    sqrt_price: our_sqrt,
                    liquidity: lay.liquidity as f64,
                },
            )
            .with_directional_fees(f64::from(our_fee0) / 1e6, f64::from(our_fee1) / 1e6)
            .with_ticks(TickMap {
                ticks: our_ticks.iter().map(|&(t, n)| (t, n as f64)).collect(),
                exact: our_ticks,
                lo: our_lo,
                hi: our_hi,
                hint: Some((our_sqrt, our_hint_tick)),
            })
            .with_exact(ExactState::V4 {
                sqrt_price: lay.sqrt,
                tick: lay.tick,
                liquidity: lay.liquidity,
                fee_zero_for_one: fee_zfo,
                fee_one_for_zero: fee_ofz,
            });
            let reference: Reference = Box::new(move |tin, a| {
                let native_zfo = (tin == T0) != flipped;
                let fee = if native_zfo { fee_zfo } else { fee_ofz };
                ref_swap(
                    lay.sqrt,
                    lay.tick,
                    lay.liquidity,
                    fee,
                    sp,
                    &lay.ticks,
                    lay.lo,
                    lay.hi,
                    native_zfo,
                    a,
                )
            });
            Some((pool, reference))
        }
    }
}

fn quote(model: Model, pool: &Pool, tin: Address, a: U256) -> Option<U256> {
    match model {
        Model::Algebra { .. } => pool.exact_amount_out_bound(tin, a).map(|(o, is_exact)| {
            assert!(
                !is_exact,
                "Algebra is priced at its maximum fee: a bound, never exact"
            );
            o
        }),
        _ => pool.exact_amount_out(tin, a),
    }
}

fn amounts(mk: &Market, tin: Address) -> Vec<U256> {
    let (rin, din) = if tin == T0 {
        (mk.r0, mk.d0)
    } else {
        (mk.r1, mk.d1)
    };
    let mut v: Vec<U256> = FRACTIONS
        .iter()
        .map(|f| rin * U256::from(*f) / U256::from(10_000_000u64))
        .collect();
    v.push(U256::from(1u8)); // one raw unit
    v.push(pow10(din)); // one whole token
    v.retain(|a| !a.is_zero());
    v
}

fn markets() -> Vec<Market> {
    let mut v = Vec::new();
    for d0 in DECIMALS {
        for d1 in DECIMALS {
            for price in PRICES {
                for depth in DEPTHS {
                    if let Some(m) = market(d0, d1, price, depth) {
                        v.push(m);
                    }
                }
            }
        }
    }
    v
}

#[test]
fn the_matrix_covers_every_decimal_pair() {
    let ms = markets();
    for d0 in DECIMALS {
        for d1 in DECIMALS {
            assert!(
                ms.iter().any(|m| m.d0 == d0 && m.d1 == d1),
                "no feasible market for decimals ({d0}, {d1})"
            );
        }
    }
}

/// Check 1: the integer quote equals the reference transcription of the contract, to the unit.
#[test]
fn every_model_matches_the_reference_at_every_decimal_pair() {
    let mut checked = 0usize;
    let mut refused = 0usize;
    for model in MODELS {
        for mk in markets() {
            let Some((pool, reference)) = build(model, &mk, true) else {
                continue;
            };
            for tin in [T0, T1] {
                for a in amounts(&mk, tin) {
                    let mine = quote(model, &pool, tin, a);
                    let want = reference(tin, a).map(|(o, _)| o);
                    assert_eq!(
                        mine,
                        want,
                        "{model:?} decimals ({}, {}) price {:?} depth 1e{} in {} amount {a}",
                        mk.d0,
                        mk.d1,
                        mk.price,
                        mk.depth,
                        if tin == T0 { "token0" } else { "token1" },
                    );
                    if mine.is_some() {
                        checked += 1;
                    } else {
                        refused += 1;
                    }
                }
            }
        }
    }
    eprintln!("reference matrix: {checked} quotes equal to the unit, {refused} refused by both");
    assert!(
        checked > 10_000,
        "the matrix must actually quote: {checked}"
    );
}

/// Check 2: the `f64` screening model agrees with the integer quote wherever the integer quote exists.
#[test]
fn the_screening_model_agrees_with_the_integer_quote() {
    let mut worst = 0f64;
    for model in MODELS {
        for mk in markets() {
            let Some((pool, reference)) = build(model, &mk, true) else {
                continue;
            };
            for tin in [T0, T1] {
                for a in amounts(&mk, tin) {
                    let Some(exact_out) = quote(model, &pool, tin, a) else {
                        continue;
                    };
                    let steps = reference(tin, a).map_or(1, |(_, k)| k) as f64;
                    let e = to_f64(exact_out);
                    let f = pool.amount_out(tin, to_f64(a));
                    // The contracts round in integers: every swap step floors its input after fee and rounds
                    // its fee up, at most two raw input units per step at the marginal rate, plus a raw output
                    // unit per step; beyond that the two must agree to 1e-9.
                    let err = (f - e).abs();
                    let rel = if e > 0.0 { err / e } else { 0.0 };
                    let one_input_unit = pool.amount_out(tin, 1.0).max(e / to_f64(a));
                    if err > 4.0 * steps + 2.0 * steps * one_input_unit {
                        worst = worst.max(rel);
                        assert!(
                            rel < 1e-9,
                            "{model:?} decimals ({}, {}) price {:?} in {} amount {a}: f64 {f} vs exact {e} (rel {rel:e})",
                            mk.d0,
                            mk.d1,
                            mk.price,
                            if tin == T0 { "token0" } else { "token1" },
                        );
                    }
                }
            }
        }
    }
    eprintln!("screening model: worst relative error {worst:e}");
}

/// Check 3, decimals are only units: the same trade in whole tokens gives the same whole-token output at every
/// decimal pair. Compared against 18/18, allowing the output token's raw rounding plus the reference
/// pool's own relative rounding.
#[test]
fn whole_token_results_do_not_depend_on_decimals() {
    let mut compared = 0usize;
    for model in MODELS {
        for price in PRICES {
            for depth in DEPTHS {
                let Some(base) = market(18, 18, price, depth) else {
                    continue;
                };
                let Some((base_pool, base_ref)) = build(model, &base, false) else {
                    continue;
                };
                for d0 in DECIMALS {
                    for d1 in DECIMALS {
                        let Some(mk) = market(d0, d1, price, depth) else {
                            continue;
                        };
                        let Some((pool, reference)) = build(model, &mk, false) else {
                            continue;
                        };
                        for zfo in [true, false] {
                            let (tin, din, dout) = if zfo { (T0, d0, d1) } else { (T1, d1, d0) };
                            let rin = if zfo { mk.r0 } else { mk.r1 };
                            let base_rin = if zfo { base.r0 } else { base.r1 };
                            for f in FRACTIONS {
                                let a = rin * U256::from(f) / U256::from(10_000_000u64);
                                let base_a = base_rin * U256::from(f) / U256::from(10_000_000u64);
                                if a.is_zero() {
                                    continue;
                                }
                                let (Some(out), Some(base_out)) = (
                                    quote(model, &pool, tin, a),
                                    quote(model, &base_pool, tin, base_a),
                                ) else {
                                    continue;
                                };
                                // Whole tokens, compared in exact rationals: out / 10^dout vs base_out / 10^18.
                                let lhs = w(out) * w(pow10(18));
                                let rhs = w(base_out) * w(pow10(dout));
                                let diff = if lhs > rhs { lhs - rhs } else { rhs - lhs };
                                // Each pool rounds per swap step: up to two raw input units (input after fee
                                // floored, fee rounded up) and a raw output unit. In the common scale a raw
                                // output unit is 10^18 on the left and 10^dout on the right; a raw input unit
                                // is worth lhs / a and rhs / base_a. The input amount itself is floored at
                                // `din` decimals (one more input unit), and the sqrt price is an integer
                                // square root (relative 1e-9 covers it at every depth in the matrix).
                                let steps =
                                    U512::from(reference(tin, a).map_or(1, |(_, k)| k) as u64 + 1);
                                let base_steps = U512::from(
                                    base_ref(tin, base_a).map_or(1, |(_, k)| k) as u64 + 1,
                                );
                                let two = U512::from(2u8);
                                let out_units = steps * w(pow10(18)) + base_steps * w(pow10(dout));
                                let in_units =
                                    two * steps * lhs / w(a) + two * base_steps * rhs / w(base_a);
                                let slack =
                                    out_units + in_units + rhs / U512::from(1_000_000_000u64);
                                assert!(
                                    diff <= slack,
                                    "{model:?} ({d0},{d1}) vs (18,18) price {price:?} depth 1e{depth} zfo {zfo} frac {f}: {out} @{dout} vs {base_out} @18 (in decimals {din})",
                                );
                                compared += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("decimal invariance: {compared} trades compared against 18/18");
    assert!(compared > 5_000);
}
