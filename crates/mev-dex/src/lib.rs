//! Pure pool pricing math. No I/O — usable from any strategy, simulator, or analytics tool.
//!
//! Amounts are `f64` raw token units: fast enough to evaluate thousands of candidate sizes per
//! update. Results are estimates; the executor's on-chain simulation is the source of truth.

use std::sync::Arc;

use mev_types::{Address, PoolInfo, PoolKind};

const Q96: f64 = 79_228_162_514_264_337_593_543_950_336.0; // 2^96

#[derive(Clone, Debug, PartialEq)]
pub enum PoolState {
    /// Constant product reserves.
    V2 { reserve0: f64, reserve1: f64 },
    /// Concentrated liquidity at the current price. Modeled as constant product on the active
    /// range's virtual reserves — accurate while a swap stays inside the current tick range.
    V3 { sqrt_price: f64, liquidity: f64 },
}

/// The initialized ticks of a concentrated-liquidity pool around its price, so a swap can be priced
/// across tick boundaries instead of only inside the current bucket. Known liquidity changes
/// (`liquidityNet`, raw units) are valid only inside the loaded window `[lo, hi]`: beyond it the
/// model stops (it never guesses liquidity it has not seen).
#[derive(Clone, Debug, PartialEq)]
pub struct TickMap {
    /// `(tick, liquidityNet)` sorted by tick, all within `[lo, hi]`.
    pub ticks: Vec<(i32, f64)>,
    pub lo: i32,
    pub hi: i32,
    /// The pool's own `(sqrt price, tick)` when the map was loaded. A price sitting exactly on a tick
    /// boundary cannot be told apart from just above or below it in f64, so while the pool's price
    /// is still this one the on-chain tick decides which side it is on.
    pub hint: Option<(f64, i32)>,
}

/// Result of walking a swap through the loaded ticks.
struct Walked {
    out: f64,
    used: f64,
    sqrt_price: f64,
    liquidity: f64,
    /// The swap ended by exhausting its input or reaching its price limit (not by running out of loaded ticks).
    complete: bool,
}

#[derive(Clone, Debug)]
pub struct Pool {
    /// Initialized ticks near the price, when loaded (V3-style pools).
    pub ticks: Option<Arc<TickMap>>,
    pub info: PoolInfo,
    pub state: PoolState,
    /// Fee fraction charged when token0 / token1 is the input (e.g. 0.003).
    pub fee0: f64,
    pub fee1: f64,
}

impl Pool {
    pub fn new(info: PoolInfo, state: PoolState) -> Self {
        let fee = match info.kind {
            PoolKind::UniV2 { fee_bps } => fee_bps as f64 / 10_000.0,
            PoolKind::UniV3 { fee } | PoolKind::PancakeV3 { fee } => fee as f64 / 1_000_000.0,
            PoolKind::UniV4 { fee, .. } => fee as f64 / 1_000_000.0,
            PoolKind::CamelotV2 => 0.003,
            // Real fees come from globalState() via `with_directional_fees`.
            PoolKind::AlgebraV3 => 0.003,
        };
        Self {
            ticks: None,
            info,
            state,
            fee0: fee,
            fee1: fee,
        }
    }

    pub fn with_ticks(mut self, ticks: TickMap) -> Self {
        self.ticks = Some(Arc::new(ticks));
        self
    }

    pub fn with_directional_fees(mut self, fee0: f64, fee1: f64) -> Self {
        self.fee0 = fee0;
        self.fee1 = fee1;
        self
    }

    /// Reserves (in, out) for a swap starting with `token_in`. V3 uses virtual reserves.
    pub fn reserves(&self, token_in: Address) -> (f64, f64) {
        let zero_for_one = token_in == self.info.token0;
        let (r0, r1) = match self.state {
            PoolState::V2 { reserve0, reserve1 } => (reserve0, reserve1),
            PoolState::V3 {
                sqrt_price,
                liquidity,
            } => {
                if sqrt_price <= 0.0 {
                    return (0.0, 0.0);
                }
                (liquidity / sqrt_price, liquidity * sqrt_price)
            }
        };
        if zero_for_one { (r0, r1) } else { (r1, r0) }
    }

    pub fn fee(&self, token_in: Address) -> f64 {
        if token_in == self.info.token0 {
            self.fee0
        } else {
            self.fee1
        }
    }

    /// Estimated output for `amount_in` of `token_in`.
    pub fn amount_out(&self, token_in: Address, amount_in: f64) -> f64 {
        if let Some(out) = self.walk(token_in, amount_in).map(|(out, _)| out) {
            return out;
        }
        let (r_in, r_out) = self.reserves(token_in);
        if r_in <= 0.0 || r_out <= 0.0 || amount_in <= 0.0 {
            return 0.0;
        }
        let in_after_fee = amount_in * (1.0 - self.fee(token_in));
        r_out * in_after_fee / (r_in + in_after_fee)
    }

    /// Marginal price of `token_in` in units of the other token, after fees.
    pub fn marginal_rate(&self, token_in: Address) -> f64 {
        let (r_in, r_out) = self.reserves(token_in);
        if r_in <= 0.0 {
            0.0
        } else {
            (r_out / r_in) * (1.0 - self.fee(token_in))
        }
    }

    /// Mid price of token0 in token1 raw units.
    pub fn price0(&self) -> f64 {
        match self.state {
            PoolState::V2 { reserve0, reserve1 } => {
                if reserve0 > 0.0 {
                    reserve1 / reserve0
                } else {
                    0.0
                }
            }
            PoolState::V3 { sqrt_price, .. } => sqrt_price * sqrt_price,
        }
    }

    /// Largest input of `token_in` the model prices exactly. V2: unbounded. V3: the input that
    /// moves the price to the edge of its tick-spacing bucket. Ticks are only initialized at
    /// multiples of the spacing, so liquidity is constant inside the bucket and the constant-product
    /// model is exact; beyond it, liquidity may change at the next initialized tick.
    pub fn max_exact_in(&self, token_in: Address) -> f64 {
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = self.state
        else {
            return f64::INFINITY;
        };
        if sqrt_price <= 0.0 || liquidity <= 0.0 {
            return 0.0;
        }
        // Algebra (Camelot V3) fees move with volatility on every swap; ours are refreshed only
        // periodically, so the model is never treated as exact here (routes get verified first).
        // Seen live: two mispriced reverts through the same Algebra pool on 2026-10-05.
        if matches!(self.info.kind, PoolKind::AlgebraV3) {
            return 0.0;
        }
        if let Some((_, used)) = self.walk(token_in, f64::INFINITY) {
            return used;
        }
        let spacing = tick_spacing(&self.info.kind) as f64;
        let tick = (sqrt_price * sqrt_price).ln() / 1.0001f64.ln();
        let lower = (tick / spacing).floor() * spacing;
        let sqrt_at = |t: f64| 1.0001f64.powf(t / 2.0);
        let fee = self.fee(token_in);
        let raw = if token_in == self.info.token0 {
            // token0 in: price falls to the bucket's lower edge.
            liquidity * (1.0 / sqrt_at(lower) - 1.0 / sqrt_price)
        } else {
            liquidity * (sqrt_at(lower + spacing) - sqrt_price)
        };
        (raw / (1.0 - fee)).max(0.0)
    }

    /// Swap `amount_in` of `token_in` through the loaded ticks. Returns `(output, input_used)`;
    /// `input_used < amount_in` when the loaded window ran out first (the model stops there).
    /// `None` when this pool has no tick map or is not concentrated-liquidity.
    fn walk(&self, token_in: Address, amount_in: f64) -> Option<(f64, f64)> {
        self.walk_limited(token_in, amount_in, None)
            .map(|w| (w.out, w.used))
    }

    /// Like `walk`, but stops at `limit` (a sqrt price: a swap's `sqrtPriceLimit`) and returns where the swap ended.
    fn walk_limited(
        &self,
        token_in: Address,
        amount_in: f64,
        limit: Option<f64>,
    ) -> Option<Walked> {
        let map = self.ticks.as_ref()?;
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = self.state
        else {
            return None;
        };
        let done = |out, used, sqrt_price, liquidity, complete| {
            Some(Walked {
                out,
                used,
                sqrt_price,
                liquidity,
                complete,
            })
        };
        if sqrt_price <= 0.0 || liquidity < 0.0 || amount_in <= 0.0 {
            return done(0.0, 0.0, sqrt_price, liquidity, true);
        }
        let fee = self.fee(token_in);
        let zero_for_one = token_in == self.info.token0;
        // Already at or past the limit: the swap moves nothing.
        if limit.is_some_and(|lm| {
            (zero_for_one && sqrt_price <= lm) || (!zero_for_one && sqrt_price >= lm)
        }) {
            return done(0.0, 0.0, sqrt_price, liquidity, true);
        }
        let mut complete = false;
        let sqrt_at = |t: f64| 1.0001f64.powf(t / 2.0);
        let current = match map.hint {
            Some((price, tick)) if price == sqrt_price => tick,
            _ => ((sqrt_price * sqrt_price).ln() / 1.0001f64.ln()).floor() as i32,
        };
        // First loaded tick strictly above the current tick.
        let mut i = map.ticks.partition_point(|&(t, _)| t <= current);
        let (mut s, mut l) = (sqrt_price, liquidity);
        let (mut budget, mut out, mut used) = (amount_in * (1.0 - fee), 0.0, 0.0);
        loop {
            // The price this step may reach: the next initialized tick or the window edge.
            let (target_tick, net) = if zero_for_one {
                match i.checked_sub(1).map(|j| map.ticks[j]) {
                    Some((t, n)) => (t, Some(n)),
                    None => (map.lo, None),
                }
            } else {
                match map.ticks.get(i) {
                    Some(&(t, n)) => (t, Some(n)),
                    None => (map.hi, None),
                }
            };
            let mut target = sqrt_at(target_tick as f64);
            let limit_hit = limit.is_some_and(|lm| {
                (zero_for_one && target <= lm) || (!zero_for_one && target >= lm)
            });
            if let (true, Some(lm)) = (limit_hit, limit) {
                target = lm;
            }
            if (zero_for_one && target >= s) || (!zero_for_one && target <= s) {
                if net.is_none() {
                    // Already at or past the window edge: nothing more is known.
                    break;
                }
                // Sitting on the boundary of a tick to cross: zero distance, then cross it.
                target = s;
            }
            // Input needed to reach `target`, and the output it yields, at liquidity `l`.
            let (need, gain) = if zero_for_one {
                (l * (1.0 / target - 1.0 / s), l * (s - target))
            } else {
                (l * (target - s), l * (1.0 / s - 1.0 / target))
            };
            if budget <= need {
                // The swap ends inside this step.
                let (ds, o) = if zero_for_one {
                    let s2 = 1.0 / (1.0 / s + budget / l.max(f64::MIN_POSITIVE));
                    (s2, l * (s - s2))
                } else {
                    let s2 = s + budget / l.max(f64::MIN_POSITIVE);
                    (s2, l * (1.0 / s - 1.0 / s2))
                };
                s = ds;
                out += o;
                used += budget;
                budget = 0.0;
                complete = true;
                break;
            }
            out += gain;
            used += need;
            budget -= need;
            s = target;
            if limit_hit {
                // The price limit stops the swap here, with input left over.
                complete = true;
                break;
            }
            match net {
                // Crossed an initialized tick: liquidity changes.
                Some(n) if zero_for_one => {
                    l -= n;
                    i -= 1;
                }
                Some(n) => {
                    l += n;
                    i += 1;
                }
                None => break, // reached the window edge
            }
            if l < 0.0 {
                break;
            }
        }
        let _ = budget;
        // `used` was measured after fees; gross it back up.
        done(out, used / (1.0 - fee), s, l, complete)
    }

    /// The pool's state after swapping `amount_in` of `token_in`, optionally bounded by a sqrt-price `limit`. `None`
    /// when it cannot be known: no tick map, or the swap runs past the loaded window.
    pub fn state_after(
        &self,
        token_in: Address,
        amount_in: f64,
        limit: Option<f64>,
    ) -> Option<PoolState> {
        let w = self.walk_limited(token_in, amount_in, limit)?;
        w.complete.then_some(PoolState::V3 {
            sqrt_price: w.sqrt_price,
            liquidity: w.liquidity,
        })
    }

    /// Reserve of `token` (virtual for V3), raw units.
    pub fn reserve_of(&self, token: Address) -> f64 {
        let (r_in, _) = self.reserves(token);
        r_in
    }
}

/// Tick spacing of a V3-style pool (the factories' fee-tier defaults; Algebra/Camelot uses 60).
pub fn tick_spacing(kind: &PoolKind) -> u32 {
    match *kind {
        PoolKind::UniV3 { fee } | PoolKind::PancakeV3 { fee } => match fee {
            100 => 1,
            500 => 10,
            2500 => 50,
            3000 => 60,
            10000 => 200,
            f => (f / 50).max(1),
        },
        PoolKind::AlgebraV3 => 60,
        PoolKind::UniV4 { tick_spacing, .. } => tick_spacing.max(1),
        PoolKind::UniV2 { .. } | PoolKind::CamelotV2 => 1,
    }
}

/// Convert a V3 `sqrtPriceX96` into the raw sqrt price ratio.
pub fn sqrt_price_from_x96(x96: f64) -> f64 {
    x96 / Q96
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const A: Address = address!("0000000000000000000000000000000000000001");
    const B: Address = address!("0000000000000000000000000000000000000002");

    fn info(kind: PoolKind) -> PoolInfo {
        PoolInfo {
            address: Address::ZERO,
            dex: "t".into(),
            kind,
            token0: A,
            token1: B,
        }
    }

    #[test]
    fn v2_matches_uniswap_formula() {
        let p = Pool::new(
            info(PoolKind::UniV2 { fee_bps: 30 }),
            PoolState::V2 {
                reserve0: 1_000.0,
                reserve1: 2_000.0,
            },
        );
        let out = p.amount_out(A, 10.0);
        let expected = 10.0 * 0.997 * 2_000.0 / (1_000.0 + 10.0 * 0.997);
        assert!((out - expected).abs() < 1e-9);
        assert!(p.amount_out(B, 10.0) < 5.0);
    }

    #[test]
    fn v3_virtual_reserves_give_spot_price() {
        // price0 = 4 -> sqrt = 2; liquidity 1e6 -> x = 5e5, y = 2e6
        let p = Pool::new(
            info(PoolKind::UniV3 { fee: 500 }),
            PoolState::V3 {
                sqrt_price: 2.0,
                liquidity: 1e6,
            },
        );
        assert!((p.price0() - 4.0).abs() < 1e-12);
        let out = p.amount_out(A, 1.0);
        assert!((out - 4.0 * 0.9995).abs() < 1e-3);
    }

    #[test]
    fn empty_pool_returns_zero() {
        let p = Pool::new(
            info(PoolKind::UniV2 { fee_bps: 30 }),
            PoolState::V2 {
                reserve0: 0.0,
                reserve1: 0.0,
            },
        );
        assert_eq!(p.amount_out(A, 1.0), 0.0);
    }

    /// A pool with liquidity 1e6 over [-100, 100) and 3e6 over [-200, -100) / [100, 200), fee 0.
    fn banded() -> Pool {
        let mut p = Pool::new(
            info(PoolKind::UniV3 { fee: 100 }),
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1e6,
            },
        );
        p.fee0 = 0.0;
        p.fee1 = 0.0;
        p.with_ticks(TickMap {
            // At -200 and 200 liquidity goes to 0; at -100 and 100 it steps by 2e6.
            ticks: vec![(-200, 3e6), (-100, -2e6), (100, -2e6), (200, -3e6)],
            lo: -300,
            hi: 300,
            hint: None,
        })
    }

    #[test]
    fn a_small_swap_inside_the_bucket_matches_constant_product() {
        let p = banded();
        let flat = Pool {
            ticks: None,
            ..p.clone()
        };
        let (a, b) = (p.amount_out(A, 1_000.0), flat.amount_out(A, 1_000.0));
        assert!((a - b).abs() < 1e-6 * b, "{a} vs {b}");
    }

    #[test]
    fn state_after_a_swap_inside_the_band_moves_only_the_price() {
        let p = banded();
        let PoolState::V3 {
            sqrt_price: s0,
            liquidity: l0,
        } = p.state
        else {
            unreachable!()
        };
        let amount = 1e3; // far from the band edge
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = p.state_after(A, amount, None).unwrap()
        else {
            unreachable!()
        };
        let expected = 1.0 / (1.0 / s0 + amount * (1.0 - p.fee0) / l0);
        assert!(
            (sqrt_price - expected).abs() < 1e-12 * expected,
            "{sqrt_price} vs {expected}"
        );
        assert_eq!(liquidity, l0);
    }

    #[test]
    fn a_price_limit_stops_the_swap_where_it_is_and_leaves_input_unspent() {
        let p = banded();
        let PoolState::V3 { sqrt_price: s0, .. } = p.state else {
            unreachable!()
        };
        let limit = s0 * 0.999_999; // a hair below the start
        let huge = 1e12;
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = p.state_after(A, huge, Some(limit)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(sqrt_price, limit, "the limit binds, not the input");
        assert_eq!(liquidity, 1e6, "and no tick is crossed");
        // A limit on the wrong side of the price moves nothing.
        let PoolState::V3 {
            sqrt_price: same, ..
        } = p.state_after(A, huge, Some(s0 * 1.1)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(same, s0);
    }

    #[test]
    fn walking_to_a_price_above_the_start_with_the_other_token_ends_exactly_there() {
        let p = banded();
        let limit = 1.0001f64.powf(30.0); // above the start, inside the first band
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = p.state_after(B, 1e60, Some(limit)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(sqrt_price, limit);
        assert_eq!(liquidity, 1e6);
    }

    #[test]
    fn state_after_crosses_a_tick_and_takes_the_new_liquidity() {
        let p = banded();
        let s100 = 1.0001f64.powf(-50.0);
        let to_edge = 1e6 * (1.0 / s100 - 1.0);
        let PoolState::V3 {
            liquidity,
            sqrt_price,
        } = p.state_after(A, 2.0 * to_edge, None).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(liquidity, 3e6);
        assert!(sqrt_price < s100);
    }

    #[test]
    fn a_swap_across_a_tick_uses_the_next_bands_liquidity() {
        let p = banded();
        // Down: token0 in. Reaching tick -100 costs L*(1/s_t - 1) of token0 at L=1e6.
        let s100 = 1.0001f64.powf(-50.0);
        let to_edge = 1e6 * (1.0 / s100 - 1.0);
        // Spend twice that: the second half is priced with 3e6 liquidity (deeper), so the output
        // beats pricing everything at 1e6.
        let across = p.amount_out(A, 2.0 * to_edge);
        let flat = Pool {
            ticks: None,
            ..p.clone()
        }
        .amount_out(A, 2.0 * to_edge);
        assert!(
            across > flat,
            "deeper liquidity past the tick: {across} vs {flat}"
        );
        // Exact check of the first leg plus the second leg at 3e6.
        let first = 1e6 * (1.0 - s100);
        let s2 = 1.0 / (1.0 / s100 + to_edge / 3e6);
        let expected = first + 3e6 * (s100 - s2);
        assert!(
            (across - expected).abs() < 1e-6 * expected,
            "{across} vs {expected}"
        );
    }

    #[test]
    fn the_model_stops_at_the_edge_of_the_loaded_window() {
        let p = banded();
        // Far more than the window holds: output is capped, and max_exact_in is the window's depth.
        let huge = p.amount_out(A, 1e30);
        let cap = p.max_exact_in(A);
        assert!(cap.is_finite() && cap > 0.0);
        assert!((p.amount_out(A, cap) - huge).abs() < 1e-6 * huge);
        // The other direction is bounded too.
        assert!(p.max_exact_in(B).is_finite());
    }

    #[test]
    fn a_price_on_a_tick_boundary_crosses_it_only_on_the_side_the_chain_says() {
        // Liquidity 1e6 now, and 0 below tick 0 (a position whose lower bound is tick 0).
        let on = |hint_tick: i32| {
            let sp = 1.0;
            let mut p = Pool::new(
                info(PoolKind::UniV3 { fee: 100 }),
                PoolState::V3 {
                    sqrt_price: sp,
                    liquidity: 1e6,
                },
            );
            p.fee0 = 0.0;
            p.fee1 = 0.0;
            p.with_ticks(TickMap {
                ticks: vec![(0, 1e6), (100, -1e6)],
                lo: -300,
                hi: 300,
                hint: Some((sp, hint_tick)),
            })
        };
        // Chain says tick 0: the tick is at or below the price, so selling token0 crosses it
        // and runs out of liquidity (nothing below).
        let down = on(0).amount_out(A, 1e9);
        assert!(down < 1.0, "no liquidity below the lower bound: {down}");
        // Chain says tick -1 (price came down onto tick 0 and crossed it already): the tick is
        // above the price, so selling token0 meets no tick before the window edge.
        let down = on(-1).amount_out(A, 1e3);
        assert!(down > 900.0, "liquidity is still the pool's: {down}");
    }

    #[test]
    fn the_two_directions_are_symmetric_around_the_price() {
        // Swapping out and back through the ticks loses only rounding (fee 0).
        let p = banded();
        let out = p.amount_out(A, 2e3);
        let moved = Pool {
            state: PoolState::V3 {
                sqrt_price: 1.0 / (1.0 + 2e3 / 1e6),
                liquidity: 1e6,
            },
            ..p.clone()
        };
        let back = moved.amount_out(B, out);
        assert!((back - 2e3).abs() < 1e-3 * 2e3, "{back}");
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use alloy::primitives::address;
    use proptest::prelude::*;

    const A: Address = address!("0000000000000000000000000000000000000001");
    const B: Address = address!("0000000000000000000000000000000000000002");

    fn v2(r0: f64, r1: f64, fee_bps: u16) -> Pool {
        let info = PoolInfo {
            address: Address::ZERO,
            dex: "p".into(),
            kind: PoolKind::UniV2 { fee_bps },
            token0: A,
            token1: B,
        };
        Pool::new(
            info,
            PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
        )
    }

    proptest! {
        #[test]
        fn output_is_monotonic_and_bounded(r0 in 1e3f64..1e24, r1 in 1e3f64..1e24, a in 1.0f64..1e22, k in 1.01f64..10.0) {
            let p = v2(r0, r1, 30);
            let small = p.amount_out(A, a);
            let big = p.amount_out(A, a * k);
            prop_assert!(big >= small);
            prop_assert!(big < r1, "can never drain the out reserve");
        }

        #[test]
        fn fees_never_increase_output(r0 in 1e3f64..1e24, r1 in 1e3f64..1e24, a in 1.0f64..1e22, fee in 1u16..1000) {
            prop_assert!(v2(r0, r1, fee).amount_out(A, a) <= v2(r0, r1, 0).amount_out(A, a));
        }
    }

    #[test]
    fn exact_input_stops_at_the_tick_bucket_edge() {
        // price 1.0 (tick 0), spacing 60: token1 in may move the price up to tick 60.
        let p = Pool::new(
            PoolInfo {
                address: Address::ZERO,
                dex: "t".into(),
                kind: PoolKind::UniV3 { fee: 3000 },
                token0: A,
                token1: B,
            },
            PoolState::V3 {
                sqrt_price: 1.0001f64.powf(30.0 / 2.0), // tick 30, bucket [0, 60)
                liquidity: 1e18,
            },
        );
        let up = p.max_exact_in(B);
        let down = p.max_exact_in(A);
        // Both directions have room (mid-bucket), and the room matches L * dsqrt / (1 - fee).
        let expect_up = 1e18 * (1.0001f64.powf(30.0) - 1.0001f64.powf(15.0)) / 0.997;
        assert!((up / expect_up - 1.0).abs() < 1e-9 && down > 0.0);
        // Swapping exactly that much lands on the edge: the model's post-price is tick 60.
        let (r_in, r_out) = p.reserves(B);
        let in_after = up * 0.997;
        let new_price = (r_in + in_after) / (r_out - r_out * in_after / (r_in + in_after));
        assert!(((new_price.ln() / 1.0001f64.ln()) - 60.0).abs() < 1e-6);
        // V2 is never bounded.
        let v2 = Pool::new(
            PoolInfo {
                address: Address::ZERO,
                dex: "t".into(),
                kind: PoolKind::UniV2 { fee_bps: 30 },
                token0: A,
                token1: B,
            },
            PoolState::V2 {
                reserve0: 1.0,
                reserve1: 1.0,
            },
        );
        assert!(v2.max_exact_in(A).is_infinite());
    }

    #[test]
    fn algebra_pools_are_never_exact() {
        let p = Pool::new(
            PoolInfo {
                address: Address::ZERO,
                dex: "camelot_v3".into(),
                kind: PoolKind::AlgebraV3,
                token0: A,
                token1: B,
            },
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1e18,
            },
        );
        assert_eq!(p.max_exact_in(A), 0.0);
        assert_eq!(p.max_exact_in(B), 0.0);
    }
}
