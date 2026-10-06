//! Pure pool pricing math. No I/O — usable from any strategy, simulator, or analytics tool.
//!
//! Amounts are `f64` raw token units: fast enough to evaluate thousands of candidate sizes per
//! update. Results are estimates; the executor's on-chain simulation is the source of truth.

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

#[derive(Clone, Debug)]
pub struct Pool {
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
            info,
            state,
            fee0: fee,
            fee1: fee,
        }
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
