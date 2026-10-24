//! Shared by the swap matrices: every pool model the bot prices, built over given raw reserves, with an
//! independent reference transcription of each contract's integer math (512-bit, straight from the
//! Solidity; only `TickMath` is shared with mev-dex). Included with `#[path]` from other crates' tests.
#![allow(dead_code)]

use alloy::primitives::{B256, U256, U512};
use mev_dex::{ExactState, Pool, PoolState, TickMap, exact};
use mev_types::{Address, PoolInfo, PoolKind};

pub fn pow10(e: u32) -> U256 {
    U256::from(10u8).pow(U256::from(e))
}

#[derive(Clone, Copy, Debug)]
pub enum Model {
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

pub const MODELS: [Model; 12] = [
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

pub fn w(x: U256) -> U512 {
    U512::from(x)
}

pub fn n(x: U512) -> Option<U256> {
    (x <= U512::from(U256::MAX)).then(|| U256::from(x))
}

pub fn q96() -> U512 {
    U512::from(1u8) << 96
}

pub fn fits256(x: U512) -> bool {
    x <= U512::from(U256::MAX)
}

pub fn mul_div(a: U512, b: U512, d: U512) -> Option<U512> {
    let r = a * b / d;
    fits256(r).then_some(r)
}

pub fn mul_div_up(a: U512, b: U512, d: U512) -> Option<U512> {
    let p = a * b;
    let r = p / d + U512::from(u8::from(!(p % d).is_zero()));
    fits256(r).then_some(r)
}

pub fn div_up(a: U512, d: U512) -> U512 {
    a / d + U512::from(u8::from(!(a % d).is_zero()))
}

pub fn isqrt(x: U512) -> U512 {
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
pub fn amount0_delta(a: U512, b: U512, l: U512, up: bool) -> Option<U512> {
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
pub fn amount1_delta(a: U512, b: U512, l: U512, up: bool) -> Option<U512> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    if up {
        mul_div_up(l, b - a, q96())
    } else {
        mul_div(l, b - a, q96())
    }
}

/// `SqrtPriceMath.getNextSqrtPriceFromInput`, with the 256-bit overflow checks of the original.
pub fn next_from_input(p: U512, l: U512, amount: U512, zfo: bool) -> Option<U512> {
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

pub struct RefStep {
    next: U512,
    amount_in: U512,
    amount_out: U512,
    fee: U512,
}

/// `SwapMath.computeSwapStep`, exact input.
pub fn compute_step(
    cur: U512,
    target: U512,
    l: U512,
    remaining: U512,
    fee: u32,
) -> Option<RefStep> {
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
pub fn ref_swap(
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
pub fn ref_v2(a: U256, rin: U256, rout: U256, fee: u32, den: u32) -> Option<U256> {
    if a.is_zero() || rin.is_zero() || rout.is_zero() {
        return None;
    }
    let with_fee = w(a) * U512::from(den - fee);
    // `swap` requires a non-zero output (`INSUFFICIENT_OUTPUT_AMOUNT`).
    n(with_fee * w(rout) / (w(rin) * U512::from(den) + with_fee)).filter(|o| !o.is_zero())
}

/// Solidly volatile `getAmountOut`: the fee is taken from the input first, in basis points.
pub fn ref_solidly(a: U256, rin: U256, rout: U256, fee_bps: u32) -> Option<U256> {
    if a.is_zero() || rin.is_zero() || rout.is_zero() {
        return None;
    }
    let net = w(a) - w(a) * U512::from(fee_bps) / U512::from(10_000u32);
    // `swap` reverts `InsufficientOutputAmount` on a zero output.
    n(net * w(rout) / (w(rin) + net)).filter(|o| !o.is_zero())
}

// ---------------------------------------------------------------------------------------------
// Pool construction: the same market as the bot's model and as the reference.
// ---------------------------------------------------------------------------------------------

/// Concentrated-liquidity layout around a price, in the pool's own orientation.
pub struct Layout {
    sqrt: U256,
    tick: i32,
    liquidity: u128,
    ticks: Vec<(i32, i128)>,
    lo: i32,
    hi: i32,
}

/// Liquidity `sqrt(r0·r1)` at price `r1/r0` (both raw), plus two positions ending on each side so a
/// large swap crosses initialized ticks, inside a window several bitmap words wide.
pub fn layout(
    r0: U256,
    r1: U256,
    spacing: i32,
    table_spacing: i32,
    positions: bool,
) -> Option<Layout> {
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

pub fn x96_to_f64(x: U256) -> f64 {
    f64::from(x) / 2f64.powi(96)
}

pub fn to_f64(x: U256) -> f64 {
    f64::from(x)
}

fn info(kind: PoolKind, address: Address, token0: Address, token1: Address) -> PoolInfo {
    PoolInfo {
        address,
        dex: "matrix".into(),
        kind,
        token0,
        token1,
    }
}

/// What the matrix checks a pool against: the reference quote for (token_in, amount), with the number
/// of swap steps the contract takes (each step rounds its input and fee by up to a raw unit).
pub type Reference = Box<dyn Fn(Address, U256) -> Option<(U256, usize)>>;

/// `positions`: add the initialized ticks; without them liquidity is uniform across the window.
/// Build `model` over raw reserves `r0` of `t0` and `r1` of `t1` (`t0 < t1`, as pools order them), at
/// `address`, together with its reference quote.
pub fn build_pool(
    model: Model,
    address: Address,
    t0: Address,
    t1: Address,
    r0: U256,
    r1: U256,
    positions: bool,
) -> Option<(Pool, Reference)> {
    assert!(t0 < t1, "pools order their tokens by address");
    let info = |kind| info(kind, address, t0, t1);
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
            let zfo = tin == t0;
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
                    let (i, o) = if tin == t0 { (r0, r1) } else { (r1, r0) };
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
                    let (i, o, f) = if tin == t0 {
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
                    let (i, o) = if tin == t0 { (r0, r1) } else { (r1, r0) };
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
                let native_zfo = (tin == t0) != flipped;
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

pub fn quote(model: Model, pool: &Pool, tin: Address, a: U256) -> Option<U256> {
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
