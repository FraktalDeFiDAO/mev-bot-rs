//! Pure pool pricing math. No I/O — usable from any strategy, simulator, or analytics tool.
//!
//! Amounts are `f64` raw token units: fast enough to evaluate thousands of candidate sizes per
//! update. Results are estimates; the executor's on-chain simulation is the source of truth.

use std::sync::Arc;

use alloy::primitives::U256;
use mev_types::{Address, PoolInfo, PoolKind};

pub mod exact;

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
    /// The same `(tick, liquidityNet)` table as exact integers (`liquidityNet` is an int128, which
    /// `f64` cannot hold), maintained alongside `ticks` for the exact re-quote (invariant 1).
    pub exact: Vec<(i32, i128)>,
}

/// A pool's on-chain state as exact integers, kept next to the `f64` model and set only from raw
/// reads or events. `None` on a pool means the exact state is unknown and exact re-quotes refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactState {
    V2 {
        reserve0: U256,
        reserve1: U256,
    },
    /// Camelot V2 (volatile): reserves plus the per-direction fee, in 1/100_000 (`token0FeePercent` /
    /// `token1FeePercent`), which is what the pair's own `getAmountOut` uses.
    V2Fee {
        reserve0: U256,
        reserve1: U256,
        fee0: u32,
        fee1: u32,
    },
    V3 {
        sqrt_price: U256,
        tick: i32,
        liquidity: u128,
    },
    /// Camelot V3 (Algebra): the price state plus the **largest fee** the pool's adaptive-fee configuration can
    /// ever charge per direction (`baseFee + alpha1 + alpha2` of `DataStorageOperator.feeConfig*`, in 1/1e6). The
    /// actual fee is recomputed from oracle history by the first swap of each block, so only the output at the
    /// maximum fee is a certain **lower bound** (output falls as fee rises); see `exact_amount_out_bound`.
    Algebra {
        sqrt_price: U256,
        tick: i32,
        liquidity: u128,
        max_fee_zero_for_one: u32,
        max_fee_one_for_zero: u32,
    },
    /// Uniswap V4 hookless pool, in the **pool's own** orientation (currency0/currency1), whatever order
    /// the bot stores its tokens in (a price flipped to our order is not an integer). Fees are per
    /// direction, in hundredths of a bip, as the manager charges them (protocol and LP compounded).
    V4 {
        sqrt_price: U256,
        tick: i32,
        liquidity: u128,
        fee_zero_for_one: u32,
        fee_one_for_zero: u32,
    },
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
    /// Exact integer state (see `ExactState`). Cleared by anything that cannot keep it exact.
    pub exact: Option<ExactState>,
}

impl Pool {
    pub fn new(info: PoolInfo, state: PoolState) -> Self {
        let fee = match info.kind {
            PoolKind::UniV2 { fee_bps } => fee_bps as f64 / 10_000.0,
            PoolKind::UniV3 { fee }
            | PoolKind::PancakeV3 { fee }
            | PoolKind::SlipstreamV3 { fee, .. } => fee as f64 / 1_000_000.0,
            PoolKind::UniV4 { fee, .. } => fee as f64 / 1_000_000.0,
            PoolKind::CamelotV2 => 0.003,
            // Real fee from the factory via `with_directional_fees`.
            PoolKind::SolidlyV2 { .. } => 0.003,
            // Real fees come from globalState() via `with_directional_fees`.
            PoolKind::AlgebraV3 => 0.003,
        };
        Self {
            ticks: None,
            info,
            state,
            fee0: fee,
            fee1: fee,
            exact: None,
        }
    }

    pub fn with_ticks(mut self, ticks: TickMap) -> Self {
        self.ticks = Some(Arc::new(ticks));
        self
    }

    pub fn with_exact(mut self, exact: ExactState) -> Self {
        self.exact = Some(exact);
        self
    }

    /// Mid-price and depth from canonical integer state; floats never vouch for a price.
    pub fn integer_rate(&self, token_in: Address) -> Option<(mev_types::price::Ratio, U256)> {
        use alloy::primitives::U512;
        use mev_types::price::Ratio;
        if !self.info.has(token_in) {
            return None;
        }
        let logical_zero = token_in == self.info.token0;
        match self.exact.as_ref()? {
            ExactState::V2 { reserve0, reserve1 }
            | ExactState::V2Fee {
                reserve0, reserve1, ..
            } => {
                let (rin, rout) = if logical_zero {
                    (*reserve0, *reserve1)
                } else {
                    (*reserve1, *reserve0)
                };
                Some((Ratio::new(U512::from(rout), U512::from(rin))?, rin))
            }
            ExactState::V3 {
                sqrt_price,
                liquidity,
                ..
            }
            | ExactState::Algebra {
                sqrt_price,
                liquidity,
                ..
            }
            | ExactState::V4 {
                sqrt_price,
                liquidity,
                ..
            } => {
                if *liquidity == 0
                    || *sqrt_price < U256::from(exact::MIN_SQRT_RATIO)
                    || *sqrt_price >= exact::max_sqrt()
                {
                    return None;
                }
                let physical_zero =
                    logical_zero ^ matches!(self.info.kind, PoolKind::UniV4 { flipped: true, .. });
                let p = U512::from(*sqrt_price);
                let q = U512::from(exact::Q96);
                let p0 = Ratio::new(p.checked_mul(p)?, q.checked_mul(q)?)?;
                let depth_rate = if physical_zero {
                    Ratio::new(q, p)?
                } else {
                    Ratio::new(p, q)?
                };
                let depth = depth_rate.value(U256::from(*liquidity), false)?;
                Some((if physical_zero { p0 } else { p0.inverse() }, depth))
            }
        }
    }

    /// The protocol-exact output of `amount_in` of `token_in`, or `None` when the exact state is not
    /// known, the pool type is not modelled exactly (Algebra: see `exact_amount_out_bound`), a V3 swap would
    /// leave the loaded tick window, or the contract would revert: every pool rejects a zero input, and V2-style
    /// pairs (Uniswap V2 `INSUFFICIENT_OUTPUT_AMOUNT`, Camelot, Aerodrome `InsufficientOutputAmount`) reject an
    /// output that rounds to zero, which small trades from a many-decimal token into a few-decimal one hit.
    /// Never an estimate: a `Some` equals what the contract pays from this state.
    pub fn exact_amount_out(&self, token_in: Address, amount_in: U256) -> Option<U256> {
        if amount_in.is_zero() {
            return None;
        }
        let out = self.exact_quote(token_in, amount_in)?;
        let pair = matches!(
            self.info.kind,
            PoolKind::UniV2 { .. } | PoolKind::CamelotV2 | PoolKind::SolidlyV2 { .. }
        );
        (!(pair && out.is_zero())).then_some(out)
    }

    fn exact_quote(&self, token_in: Address, amount_in: U256) -> Option<U256> {
        let zero_for_one = token_in == self.info.token0;
        match (self.exact.as_ref()?, self.info.kind) {
            (ExactState::V2 { reserve0, reserve1 }, PoolKind::UniV2 { fee_bps }) => {
                let (rin, rout) = if zero_for_one {
                    (*reserve0, *reserve1)
                } else {
                    (*reserve1, *reserve0)
                };
                exact::v2_amount_out(
                    amount_in,
                    rin,
                    rout,
                    10_000u64.checked_sub(fee_bps as u64)?,
                    10_000,
                )
            }
            (
                ExactState::V2Fee {
                    reserve0,
                    reserve1,
                    fee0,
                    fee1,
                },
                PoolKind::CamelotV2,
            ) => {
                let (rin, rout, fee) = if zero_for_one {
                    (*reserve0, *reserve1, *fee0)
                } else {
                    (*reserve1, *reserve0, *fee1)
                };
                exact::v2_amount_out(
                    amount_in,
                    rin,
                    rout,
                    100_000u64.checked_sub(u64::from(fee))?,
                    100_000,
                )
            }
            (
                ExactState::V2Fee {
                    reserve0,
                    reserve1,
                    fee0,
                    ..
                },
                PoolKind::SolidlyV2 { .. },
            ) => {
                // One fee for both directions, in basis points (stored in `fee0` and `fee1`).
                let (rin, rout) = if zero_for_one {
                    (*reserve0, *reserve1)
                } else {
                    (*reserve1, *reserve0)
                };
                exact::solidly_amount_out(amount_in, rin, rout, *fee0)
            }
            (
                ExactState::V3 {
                    sqrt_price,
                    tick,
                    liquidity,
                },
                PoolKind::UniV3 { fee }
                | PoolKind::PancakeV3 { fee }
                | PoolKind::SlipstreamV3 { fee, .. },
            ) => self.exact_v3_walk(
                *sqrt_price,
                *tick,
                *liquidity,
                fee,
                zero_for_one,
                false,
                amount_in,
            ),
            (
                ExactState::V4 {
                    sqrt_price,
                    tick,
                    liquidity,
                    fee_zero_for_one,
                    fee_one_for_zero,
                },
                PoolKind::UniV4 { flipped, .. },
            ) => {
                // Our token0 is the pool's currency1 when flipped.
                let native_zfo = zero_for_one != flipped;
                let fee = if native_zfo {
                    *fee_zero_for_one
                } else {
                    *fee_one_for_zero
                };
                self.exact_v3_walk(
                    *sqrt_price,
                    *tick,
                    *liquidity,
                    fee,
                    native_zfo,
                    flipped,
                    amount_in,
                )
            }
            _ => None,
        }
    }

    /// Like `exact_amount_out`, but also prices pools whose fee is not known in advance. The flag says whether the
    /// number is the contract's exact output (`true`) or a guaranteed **lower bound** on it (`false`: Algebra, quoted
    /// at its maximum possible fee). A route whose lower-bound profit is positive is profitable whatever the fee.
    pub fn exact_amount_out_bound(
        &self,
        token_in: Address,
        amount_in: U256,
    ) -> Option<(U256, bool)> {
        if amount_in.is_zero() {
            return None;
        }
        if let Some(out) = self.exact_amount_out(token_in, amount_in) {
            return Some((out, true));
        }
        let ExactState::Algebra {
            sqrt_price,
            tick,
            liquidity,
            max_fee_zero_for_one,
            max_fee_one_for_zero,
        } = self.exact.as_ref()?
        else {
            return None;
        };
        if !matches!(self.info.kind, PoolKind::AlgebraV3) {
            return None;
        }
        let zero_for_one = token_in == self.info.token0;
        let fee = if zero_for_one {
            *max_fee_zero_for_one
        } else {
            *max_fee_one_for_zero
        };
        // Algebra's tick table is not compressed by spacing (spacing 1 = row of 256 ticks), and a missing tick
        // map means the window is unknown: refuse rather than assume a bucket.
        self.ticks.as_ref()?;
        self.exact_v3_walk_with_spacing(
            1,
            *sqrt_price,
            *tick,
            *liquidity,
            fee,
            zero_for_one,
            false,
            amount_in,
        )
        .map(|out| (out, false))
    }

    /// Exact V3-style walk. `mirrored`: the tick table is stored for our flipped token order
    /// (`tick -> -tick`, `net -> -net`, window swapped), so it is turned back into the pool's own first.
    #[allow(clippy::too_many_arguments)]
    fn exact_v3_walk(
        &self,
        sqrt_price: U256,
        tick: i32,
        liquidity: u128,
        fee_pips: u32,
        native_zero_for_one: bool,
        mirrored: bool,
        amount_in: U256,
    ) -> Option<U256> {
        let spacing = tick_spacing(&self.info.kind) as i32;
        self.exact_v3_walk_with_spacing(
            spacing,
            sqrt_price,
            tick,
            liquidity,
            fee_pips,
            native_zero_for_one,
            mirrored,
            amount_in,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn exact_v3_walk_with_spacing(
        &self,
        spacing: i32,
        sqrt_price: U256,
        tick: i32,
        liquidity: u128,
        fee_pips: u32,
        native_zero_for_one: bool,
        mirrored: bool,
        amount_in: U256,
    ) -> Option<U256> {
        let (ticks, lo, hi) = match self.ticks.as_deref() {
            Some(m) if mirrored => {
                let mut t: Vec<(i32, i128)> = m
                    .exact
                    .iter()
                    .map(|&(k, n)| n.checked_neg().map(|n| (-k, n)))
                    .collect::<Option<Vec<_>>>()?;
                t.sort_by_key(|(k, _)| *k);
                (t, -m.hi, -m.lo)
            }
            Some(m) => (m.exact.clone(), m.lo, m.hi),
            None => {
                let lo = tick.div_euclid(spacing) * spacing;
                (Vec::new(), lo, lo + spacing)
            }
        };
        let pool = exact::V3Exact {
            sqrt_price,
            tick,
            liquidity,
            fee_pips,
            spacing,
            ticks,
            lo,
            hi,
        };
        let r = pool.swap_exact_in(native_zero_for_one, amount_in, None)?;
        (r.complete && r.amount_in == amount_in).then_some(r.amount_out)
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
            // l (1/lo - 1/s), written without the cancelling difference of reciprocals.
            let lo = sqrt_at(lower);
            liquidity * (sqrt_price - lo) / (lo * sqrt_price)
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
            // Input needed to reach `target`, and the output it yields, at liquidity `l`. Differences of
            // reciprocals are written as `(b - a) / (a b)`: with a large decimal gap the sqrt price is far
            // from 1, and `1/a - 1/b` would cancel catastrophically (a 1-token swap off by 1.5%).
            let (need, gain) = if zero_for_one {
                (l * (s - target) / (s * target), l * (s - target))
            } else {
                (l * (target - s), l * (target - s) / (s * target))
            };
            if budget <= need {
                // The swap ends inside this step.
                let l = l.max(f64::MIN_POSITIVE);
                let (ds, o) = if zero_for_one {
                    // 1/s2 = 1/s + budget/l, so s2 = l s / (l + budget s) and s - s2 = budget s² / (l + budget s).
                    let den = l + budget * s;
                    (l * s / den, l * budget * s * s / den)
                } else {
                    let s2 = s + budget / l;
                    // l (1/s - 1/s2) = l (s2 - s) / (s s2) = budget / (s s2).
                    (s2, budget / (s * s2))
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
        PoolKind::SlipstreamV3 { tick_spacing, .. } => tick_spacing.max(1),
        PoolKind::UniV3 { fee } | PoolKind::PancakeV3 { fee } => match fee {
            100 => 1,
            500 => 10,
            2500 => 50,
            3000 => 60,
            10000 => 200,
            f => (f / 50).max(1),
        },
        // Camelot's deployment (read from two live pools, 2026-10-11) uses spacing 10, not Algebra's default 60.
        PoolKind::AlgebraV3 => 10,
        PoolKind::UniV4 { tick_spacing, .. } => tick_spacing.max(1),
        PoolKind::UniV2 { .. } | PoolKind::CamelotV2 | PoolKind::SolidlyV2 { .. } => 1,
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
    fn solidly_pool_quotes_with_its_own_rounding_both_ways() {
        let p = Pool::new(
            info(PoolKind::SolidlyV2 {
                factory: Address::ZERO,
            }),
            PoolState::V2 {
                reserve0: 140_892.0,
                reserve1: 596_854.0,
            },
        )
        .with_exact(ExactState::V2Fee {
            reserve0: U256::from(140_892),
            reserve1: U256::from(596_854),
            fee0: 30,
            fee1: 30,
        });
        assert_eq!(
            p.exact_amount_out(A, U256::from(8272)),
            Some(U256::from(33_008))
        );
        assert_eq!(
            p.exact_amount_out(B, U256::from(8272)),
            exact::solidly_amount_out(
                U256::from(8272),
                U256::from(596_854),
                U256::from(140_892),
                30
            )
        );
        assert_eq!(
            p.info.kind.hop_kind(),
            1,
            "routed through the getAmountOut hop"
        );
        assert_eq!(tick_spacing(&p.info.kind), 1);
    }

    #[test]
    fn slipstream_uses_the_v3_hop_and_its_own_tick_spacing() {
        let k = PoolKind::SlipstreamV3 {
            fee: 684,
            tick_spacing: 100,
        };
        assert_eq!(k.hop_kind(), 2, "Uniswap V3 swap() and callback");
        assert_eq!(
            tick_spacing(&k),
            100,
            "keyed by spacing, not derived from the fee"
        );
        let p = Pool::new(
            info(k),
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1.0,
            },
        );
        assert_eq!((p.fee0, p.fee1), (0.000684, 0.000684));
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
    fn swaps_the_contract_would_revert_are_not_quoted() {
        // 18 decimals in, 8 out: 1e9 wei of the input is worth less than one raw unit of the output.
        let reserves = ExactState::V2 {
            reserve0: U256::from(10u128.pow(24)),
            reserve1: U256::from(10u128.pow(14)),
        };
        let p = Pool::new(
            PoolInfo {
                address: Address::ZERO,
                dex: "v2".into(),
                kind: PoolKind::UniV2 { fee_bps: 30 },
                token0: A,
                token1: B,
            },
            PoolState::V2 {
                reserve0: 1e24,
                reserve1: 1e14,
            },
        )
        .with_exact(reserves);
        assert_eq!(
            p.exact_amount_out(A, U256::from(1_000_000_000u64)),
            None,
            "zero output reverts"
        );
        assert_eq!(
            p.exact_amount_out(A, U256::ZERO),
            None,
            "zero input reverts"
        );
        assert!(
            p.exact_amount_out(A, U256::from(10u128.pow(12)))
                .is_some_and(|o| !o.is_zero())
        );
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
            exact: Vec::new(),
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
                exact: Vec::new(),
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
