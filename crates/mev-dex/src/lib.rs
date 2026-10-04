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

    /// Reserve of `token` (virtual for V3), raw units.
    pub fn reserve_of(&self, token: Address) -> f64 {
        let (r_in, _) = self.reserves(token);
        r_in
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
}
