//! Protocol-exact integer swap math (invariant 1): Uniswap V2 `getAmountOut` and the Uniswap V3
//! `TickMath` / `SqrtPriceMath` / `SwapMath` single-step functions, ported from v3-core with the
//! same rounding directions. Every function returns `None` where the contract would revert or
//! overflow; nothing here uses floating point. The `f64` model in the parent module remains a
//! search heuristic; amounts that gate a send must be re-quoted with these functions.

use alloy::primitives::{U256, U512};

pub const MIN_TICK: i32 = -887_272;
pub const MAX_TICK: i32 = 887_272;
pub const MIN_SQRT_RATIO: u128 = 4_295_128_739;
pub const Q96: U256 = U256::from_limbs([0, 0x1_0000_0000, 0, 0]);

fn narrow(q: U512) -> Option<U256> {
    let l = q.as_limbs();
    if l[4..].iter().any(|x| *x != 0) {
        return None;
    }
    Some(U256::from_limbs([l[0], l[1], l[2], l[3]]))
}

fn mul_div(a: U256, b: U256, d: U256) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let q = U512::from(a) * U512::from(b) / U512::from(d);
    narrow(q)
}

fn mul_div_up(a: U256, b: U256, d: U256) -> Option<U256> {
    if d.is_zero() {
        return None;
    }
    let p = U512::from(a) * U512::from(b);
    let d5 = U512::from(d);
    let mut q = p / d5;
    if !(p % d5).is_zero() {
        q += U512::from(1u8);
    }
    narrow(q)
}

fn div_up(a: U256, b: U256) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    let q = a / b;
    Some(if (a % b).is_zero() {
        q
    } else {
        q + U256::from(1u8)
    })
}

fn max_sqrt_ratio() -> U256 {
    U256::from_str_radix("1461446703485210103287273052203988822378723970342", 10).unwrap()
}

/// UniswapV2Library.getAmountOut with a fee expressed as `fee_num / fee_den` kept by the pool
/// (Uniswap's 0.3% is 997/1000 retained).
pub fn v2_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    retained_num: u64,
    retained_den: u64,
) -> Option<U256> {
    if amount_in.is_zero()
        || reserve_in.is_zero()
        || reserve_out.is_zero()
        || retained_den == 0
        || retained_num > retained_den
    {
        return None;
    }
    let with_fee = amount_in.checked_mul(U256::from(retained_num))?;
    let num = with_fee.checked_mul(reserve_out)?;
    let den = reserve_in
        .checked_mul(U256::from(retained_den))?
        .checked_add(with_fee)?;
    Some(num / den)
}

const RATIOS: [(u32, &str); 19] = [
    (0x2, "fff97272373d413259a46990580e213a"),
    (0x4, "fff2e50f5f656932ef12357cf3c7fdcc"),
    (0x8, "ffe5caca7e10e4e61c3624eaa0941cd0"),
    (0x10, "ffcb9843d60f6159c9db58835c926644"),
    (0x20, "ff973b41fa98c081472e6896dfb254c0"),
    (0x40, "ff2ea16466c96a3843ec78b326b52861"),
    (0x80, "fe5dee046a99a2a811c461f1969c3053"),
    (0x100, "fcbe86c7900a88aedcffc83b479aa3a4"),
    (0x200, "f987a7253ac413176f2b074cf7815e54"),
    (0x400, "f3392b0822b70005940c7a398e4b70f3"),
    (0x800, "e7159475a2c29b7443b29c7fa6e889d9"),
    (0x1000, "d097f3bdfd2022b8845ad8f792aa5825"),
    (0x2000, "a9f746462d870fdf8a65dc1f90e061e5"),
    (0x4000, "70d869a156d2a1b890bb3df62baf32f7"),
    (0x8000, "31be135f97d08fd981231505542fcfa6"),
    (0x10000, "9aa508b5b7a84e1c677de54f3e99bc9"),
    (0x20000, "5d6af8dedb81196699c329225ee604"),
    (0x40000, "2216e584f5fa1ea926041bedfe98"),
    (0x80000, "48a170391f7dc42444e8fa2"),
];

/// TickMath.getSqrtRatioAtTick.
pub fn sqrt_ratio_at_tick(tick: i32) -> Option<U256> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return None;
    }
    let abs = tick.unsigned_abs();
    let mut ratio = if abs & 1 != 0 {
        U256::from_str_radix("fffcb933bd6fad37aa2d162d1a594001", 16).ok()?
    } else {
        U256::from(1u8) << 128
    };
    for (bit, hex) in RATIOS {
        if abs & bit != 0 {
            ratio = (ratio * U256::from_str_radix(hex, 16).ok()?) >> 128;
        }
    }
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }
    let low = ratio & U256::from(u32::MAX);
    let r = (ratio >> 32)
        + if low.is_zero() {
            U256::ZERO
        } else {
            U256::from(1u8)
        };
    Some(r)
}

pub fn amount0_delta(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<U256> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    if a.is_zero() {
        return None;
    }
    let n1 = U256::from(liquidity) << 96;
    let n2 = b - a;
    if round_up {
        div_up(mul_div_up(n1, n2, b)?, a)
    } else {
        Some(mul_div(n1, n2, b)? / a)
    }
}

pub fn amount1_delta(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<U256> {
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    let l = U256::from(liquidity);
    if round_up {
        mul_div_up(l, b - a, Q96)
    } else {
        mul_div(l, b - a, Q96)
    }
}

fn next_price_from_amount0_up_add(p: U256, liquidity: u128, amount: U256) -> Option<U256> {
    if amount.is_zero() {
        return Some(p);
    }
    let n1: U256 = U256::from(liquidity) << 96;
    if let Some(product) = amount.checked_mul(p)
        && let Some(den) = n1.checked_add(product)
    {
        return mul_div_up(n1, p, den);
    }
    div_up(n1, (n1 / p).checked_add(amount)?)
}

fn next_price_from_amount1_down_add(p: U256, liquidity: u128, amount: U256) -> Option<U256> {
    let l = U256::from(liquidity);
    let quotient = if amount <= (U256::from(1u8) << 160) - U256::from(1u8) {
        (amount << 96) / l
    } else {
        mul_div(amount, Q96, l)?
    };
    let r = p.checked_add(quotient)?;
    (r < (U256::from(1u8) << 160)).then_some(r)
}

/// SqrtPriceMath.getNextSqrtPriceFromInput.
pub fn next_sqrt_price_from_input(
    p: U256,
    liquidity: u128,
    amount_in: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if p.is_zero() || liquidity == 0 {
        return None;
    }
    if zero_for_one {
        next_price_from_amount0_up_add(p, liquidity, amount_in)
    } else {
        next_price_from_amount1_down_add(p, liquidity, amount_in)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Step {
    pub sqrt_next: U256,
    pub amount_in: U256,
    pub amount_out: U256,
    pub fee: U256,
}

/// SwapMath.computeSwapStep for an exact-input swap (`amount_remaining` ≥ 0).
pub fn swap_step_exact_in(
    current: U256,
    target: U256,
    liquidity: u128,
    remaining: U256,
    fee_pips: u32,
) -> Option<Step> {
    if fee_pips >= 1_000_000 {
        return None;
    }
    let zero_for_one = current >= target;
    let m = U256::from(1_000_000u32);
    let less_fee = mul_div(remaining, m - U256::from(fee_pips), m)?;
    let to_target = if zero_for_one {
        amount0_delta(target, current, liquidity, true)?
    } else {
        amount1_delta(current, target, liquidity, true)?
    };
    let next = if less_fee >= to_target {
        target
    } else {
        next_sqrt_price_from_input(current, liquidity, less_fee, zero_for_one)?
    };
    let max = next == target;
    let (amount_in, amount_out) = if zero_for_one {
        (
            if max {
                to_target
            } else {
                amount0_delta(next, current, liquidity, true)?
            },
            amount1_delta(next, current, liquidity, false)?,
        )
    } else {
        (
            if max {
                to_target
            } else {
                amount1_delta(current, next, liquidity, true)?
            },
            amount0_delta(current, next, liquidity, false)?,
        )
    };
    let fee = if next != target {
        remaining - amount_in
    } else {
        mul_div_up(amount_in, U256::from(fee_pips), m - U256::from(fee_pips))?
    };
    Some(Step {
        sqrt_next: next,
        amount_in,
        amount_out,
        fee,
    })
}

/// Largest tick whose `sqrt_ratio_at_tick` is ≤ `sqrt_price` (TickMath.getTickAtSqrtRatio, by
/// bisection over the exact forward function so the two can never disagree).
pub fn tick_at_sqrt_ratio(sqrt_price: U256) -> Option<i32> {
    if sqrt_price < U256::from(MIN_SQRT_RATIO) || sqrt_price >= max_sqrt_ratio() {
        return None;
    }
    let (mut lo, mut hi) = (MIN_TICK, MAX_TICK);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if sqrt_ratio_at_tick(mid)? <= sqrt_price {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(lo)
}

/// A concentrated-liquidity pool in exact integers. `ticks` are `(tick, liquidityNet)` sorted by tick and
/// valid only inside `[lo, hi]`; the walk stops (`complete == false`) rather than guess past that window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V3Exact {
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
    pub fee_pips: u32,
    /// The pool's tick spacing: each swap step looks for an initialized tick **within one 256-bit bitmap word**
    /// (`nextInitializedTickWithinOneWord`) and otherwise stops at the word boundary, which splits the step and
    /// changes rounding. Emulating that is what makes the result equal the contract's to the unit.
    pub spacing: i32,
    pub ticks: Vec<(i32, i128)>,
    pub lo: i32,
    pub hi: i32,
}

enum Next {
    /// An initialized tick inside the current word.
    Init(i32, i128),
    /// The word boundary with no initialized tick before it: price moves there, liquidity unchanged.
    Boundary(i32),
    /// The edge of the loaded window: nothing beyond is known.
    Edge(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swapped {
    /// Input consumed including fees.
    pub amount_in: U256,
    pub amount_out: U256,
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
    /// The input was fully spent or `limit` reached; false means the loaded window ran out first.
    pub complete: bool,
}

impl V3Exact {
    /// The `UniswapV3Pool.swap` loop for an exact-input swap.
    pub fn swap_exact_in(
        &self,
        zero_for_one: bool,
        amount_in: U256,
        limit: Option<U256>,
    ) -> Option<Swapped> {
        if self.spacing <= 0
            || self.spacing > 32_767
            || self.fee_pips >= 1_000_000
            || self.lo < MIN_TICK
            || self.hi > MAX_TICK
            || self.lo >= self.hi
            || self.tick < self.lo
            || self.tick >= self.hi
            || self.ticks.len() > 8192
            || self.ticks.windows(2).any(|w| w[0].0 >= w[1].0)
            || self
                .ticks
                .iter()
                .any(|(t, _)| *t < self.lo || *t > self.hi || t.rem_euclid(self.spacing) != 0)
            || self.sqrt_price < U256::from(MIN_SQRT_RATIO)
            || self.sqrt_price >= max_sqrt_ratio()
        {
            return None;
        }
        let (mut s, mut tick, mut l) = (self.sqrt_price, self.tick, self.liquidity);
        let mut remaining = amount_in;
        let mut out = U256::ZERO;
        let max = max_sqrt_ratio();
        let limit = limit.unwrap_or(if zero_for_one {
            U256::from(MIN_SQRT_RATIO) + U256::from(1u8)
        } else {
            max - U256::from(1u8)
        });
        if limit <= U256::from(MIN_SQRT_RATIO) || limit >= max {
            return None;
        }
        if if zero_for_one { limit >= s } else { limit <= s } {
            return None;
        }
        let mut complete = false;
        // At most 8192 initialized ticks plus the full-domain bitmap word boundaries.
        for _ in 0..16_384 {
            if remaining.is_zero() {
                break;
            }
            // `nextInitializedTickWithinOneWord`: the nearest initialized tick in the current bitmap word, else
            // that word's boundary; the loaded window's edge stops the walk (nothing beyond it is known).
            let spacing = self.spacing;
            let compressed = tick.div_euclid(spacing);
            let next = if zero_for_one {
                let boundary = compressed.div_euclid(256) * 256 * spacing;
                let i = self.ticks.partition_point(|(t, _)| *t <= tick);
                match i
                    .checked_sub(1)
                    .and_then(|i| self.ticks.get(i))
                    .filter(|(t, _)| *t >= boundary)
                {
                    Some(&(t, n)) => Next::Init(t, n),
                    None if boundary >= self.lo => Next::Boundary(boundary),
                    None => Next::Edge(self.lo),
                }
            } else {
                let boundary = ((compressed + 1).div_euclid(256) * 256 + 255) * spacing;
                let i = self.ticks.partition_point(|(t, _)| *t <= tick);
                match self.ticks.get(i).filter(|(t, _)| *t <= boundary) {
                    Some(&(t, n)) => Next::Init(t, n),
                    None if boundary <= self.hi => Next::Boundary(boundary),
                    None => Next::Edge(self.hi),
                }
            };
            let (next_tick, net, at_edge) = match next {
                Next::Init(t, n) => (t, Some(n), false),
                Next::Boundary(b) => (b, None, false),
                Next::Edge(e) => (e, None, true),
            };
            let next_price = sqrt_ratio_at_tick(next_tick.clamp(MIN_TICK, MAX_TICK))?;
            let beyond = if zero_for_one {
                next_price < limit
            } else {
                next_price > limit
            };
            let target = if beyond { limit } else { next_price };
            if if zero_for_one {
                target >= s
            } else {
                target <= s
            } {
                // Sitting on the boundary: nothing to trade until the tick is crossed.
                if at_edge && !beyond {
                    break;
                }
            }
            let step = if l == 0 {
                // No liquidity in this band: the price just moves to the target.
                Step {
                    sqrt_next: target,
                    amount_in: U256::ZERO,
                    amount_out: U256::ZERO,
                    fee: U256::ZERO,
                }
            } else {
                swap_step_exact_in(s, target, l, remaining, self.fee_pips)?
            };
            remaining = remaining.checked_sub(step.amount_in.checked_add(step.fee)?)?;
            out = out.checked_add(step.amount_out)?;
            s = step.sqrt_next;
            if s == next_price && !beyond {
                if at_edge {
                    tick = if zero_for_one {
                        next_tick - 1
                    } else {
                        next_tick
                    };
                    break; // window edge: nothing more is known
                }
                if let Some(n) = net {
                    let delta = if zero_for_one { n.checked_neg()? } else { n };
                    l = if delta < 0 {
                        l.checked_sub(delta.unsigned_abs())?
                    } else {
                        l.checked_add(delta as u128)?
                    };
                }
                tick = if zero_for_one {
                    next_tick - 1
                } else {
                    next_tick
                };
            } else {
                tick = tick_at_sqrt_ratio(s)?;
                if s == limit {
                    complete = true;
                    break;
                }
            }
        }
        if remaining.is_zero() {
            complete = true;
        }
        Some(Swapped {
            amount_in: amount_in - remaining,
            amount_out: out,
            sqrt_price: s,
            tick,
            liquidity: l,
            complete,
        })
    }
}

pub fn max_sqrt() -> U256 {
    max_sqrt_ratio()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> U256 {
        U256::from_str_radix(s, 10).unwrap()
    }

    #[test]
    fn tick_math_matches_the_contract_at_known_points() {
        assert_eq!(sqrt_ratio_at_tick(0), Some(Q96));
        assert_eq!(
            sqrt_ratio_at_tick(MIN_TICK),
            Some(U256::from(MIN_SQRT_RATIO))
        );
        assert_eq!(
            sqrt_ratio_at_tick(MAX_TICK),
            Some(u("1461446703485210103287273052203988822378723970342"))
        );
        assert_eq!(sqrt_ratio_at_tick(MAX_TICK + 1), None);
        // v3-core TickMath.spec snapshot values.
        assert_eq!(
            sqrt_ratio_at_tick(50),
            Some(u("79426470787362580746886972461"))
        );
        assert_eq!(
            sqrt_ratio_at_tick(-50),
            Some(u("79030349367926598376800521322"))
        );
    }

    #[test]
    fn v2_matches_the_library_and_reverts_on_empty_inputs() {
        // 1e18 into a 100e18/100e18 pool at 0.3%: 1e18*997*100e18 / (100e18*1000 + 1e18*997).
        let out = v2_amount_out(
            u("1000000000000000000"),
            u("100000000000000000000"),
            u("100000000000000000000"),
            997,
            1000,
        )
        .unwrap();
        assert_eq!(out, u("987158034397061298"));
        assert_eq!(
            v2_amount_out(U256::ZERO, U256::from(1u8), U256::from(1u8), 997, 1000),
            None
        );
        assert_eq!(
            v2_amount_out(U256::from(1u8), U256::from(1u8), U256::from(1u8), 0, 0),
            None
        );
    }

    #[test]
    fn a_price_outside_the_known_tick_window_is_not_quoted() {
        let p = V3Exact {
            tick: 3000,
            sqrt_price: sqrt_ratio_at_tick(3000).unwrap(),
            ..pool(vec![])
        };
        assert!(
            p.swap_exact_in(true, U256::from(10u128.pow(18)), None)
                .is_none()
        );
        let p = V3Exact {
            spacing: 0,
            ..pool(vec![])
        };
        assert!(p.swap_exact_in(true, U256::from(1u8), None).is_none());
        let p = pool(vec![(10, 1), (-10, 1)]);
        assert!(p.swap_exact_in(true, U256::from(1u8), None).is_none());
    }

    #[test]
    fn swap_step_conserves_the_input_and_never_overpays() {
        let cur = sqrt_ratio_at_tick(0).unwrap();
        let tgt = sqrt_ratio_at_tick(-600).unwrap();
        let l = 10u128.pow(18);
        for amt in [1u128, 1_000, 10u128.pow(12), 10u128.pow(15), 10u128.pow(18)] {
            let s = swap_step_exact_in(cur, tgt, l, U256::from(amt), 3000).unwrap();
            assert!(s.sqrt_next <= cur && s.sqrt_next >= tgt);
            assert!(s.amount_in + s.fee <= U256::from(amt) || s.sqrt_next == tgt);
            if s.sqrt_next != tgt {
                assert_eq!(s.amount_in + s.fee, U256::from(amt));
            }
        }
        // Enough input to cross the whole band stops exactly at the target.
        let s = swap_step_exact_in(cur, tgt, l, U256::from(10u128.pow(18)), 3000).unwrap();
        assert_eq!(s.sqrt_next, tgt);
    }

    fn pool(ticks: Vec<(i32, i128)>) -> V3Exact {
        V3Exact {
            sqrt_price: Q96,
            tick: 0,
            liquidity: 10u128.pow(18),
            fee_pips: 3000,
            spacing: 10,
            ticks,
            lo: -2000,
            hi: 2000,
        }
    }

    #[test]
    fn tick_lookup_inverts_the_forward_function() {
        for t in [-887_271, -60_000, -1, 0, 1, 12_345, 887_271] {
            let p = sqrt_ratio_at_tick(t).unwrap();
            assert_eq!(tick_at_sqrt_ratio(p), Some(t));
            assert_eq!(tick_at_sqrt_ratio(p + U256::from(1u8)).unwrap(), t);
            assert_eq!(tick_at_sqrt_ratio(p - U256::from(1u8)).unwrap(), t - 1);
        }
    }

    #[test]
    fn a_swap_inside_one_band_matches_the_closed_form() {
        let p = pool(vec![(-1000, 10i128.pow(18)), (1000, -(10i128.pow(18)))]);
        let amt = U256::from(10u128.pow(15));
        let r = p.swap_exact_in(false, amt, None).unwrap();
        assert!(r.complete && r.amount_in == amt && r.tick >= 0 && r.liquidity == 10u128.pow(18));
        // token1 in: out0 ≈ L * (1/s0 - 1/s1) with s1 = s0 + in_less_fee/L
        let in_less_fee = 10f64.powi(15) * 0.997;
        let l = 1e18f64;
        let expect = l * (1.0 - 1.0 / (1.0 + in_less_fee / l));
        let got = r.amount_out.to_string().parse::<f64>().unwrap();
        assert!(((got - expect) / expect).abs() < 1e-9, "{got} vs {expect}");
    }

    #[test]
    fn crossing_a_tick_changes_liquidity_and_spends_exactly_the_input() {
        let p = pool(vec![
            (100, 5 * 10i128.pow(17)),
            (1000, -(5 * 10i128.pow(17))),
        ]);
        let amt = U256::from(10u128.pow(16));
        let r = p.swap_exact_in(false, amt, None).unwrap();
        assert!(r.complete);
        assert_eq!(r.amount_in, amt);
        assert!(r.tick >= 100, "tick {}", r.tick);
        assert_eq!(r.liquidity, 15 * 10u128.pow(17));
        // Selling back the other way crosses it again and restores the liquidity.
        let back = V3Exact {
            sqrt_price: r.sqrt_price,
            tick: r.tick,
            liquidity: r.liquidity,
            ..p.clone()
        };
        let b = back.swap_exact_in(true, r.amount_out, None).unwrap();
        assert!(b.complete && b.tick < 100 && b.liquidity == 10u128.pow(18));
        assert!(b.amount_out < amt, "round trip cannot profit");
    }

    #[test]
    fn running_past_the_loaded_window_is_reported_not_guessed() {
        let p = pool(vec![]);
        let r = p
            .swap_exact_in(false, U256::from(10u128.pow(30)), None)
            .unwrap();
        assert!(!r.complete);
        assert_eq!(r.tick, 2000);
    }

    #[test]
    fn steps_stop_at_bitmap_word_boundaries_like_the_contract() {
        // Spacing 10 -> a word spans 2560 ticks. Starting at tick 100 swapping down must stop at tick 0 (the word
        // start, uninitialized), cross into the previous word and keep going; liquidity is unchanged throughout.
        let p = V3Exact {
            tick: 100,
            sqrt_price: sqrt_ratio_at_tick(100).unwrap(),
            ..pool(vec![])
        };
        let r = p
            .swap_exact_in(true, U256::from(2u128 * 10u128.pow(16)), None)
            .unwrap();
        assert!(
            r.complete && r.tick < 0 && r.liquidity == 10u128.pow(18),
            "tick {}",
            r.tick
        );
        // Splitting at the boundary can only cost rounding dust: the one-step answer is at most a few units higher.
        let one_step = swap_step_exact_in(
            sqrt_ratio_at_tick(100).unwrap(),
            sqrt_ratio_at_tick(-2000).unwrap(),
            10u128.pow(18),
            U256::from(2u128 * 10u128.pow(16)),
            3000,
        )
        .unwrap();
        assert!(
            one_step.amount_out >= r.amount_out
                && one_step.amount_out - r.amount_out < U256::from(10u8)
        );
    }
}
