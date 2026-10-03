//! Aave v3 liquidation math (close factor, bonus after protocol fee). Pure — no I/O.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

/// Aave v3: positions below this HF can be fully liquidated (above it, 50%).
pub const CLOSE_FACTOR_HF_THRESHOLD: f64 = 0.95;
/// Aave v3.3+: positions whose debt or collateral is below this (USD) can be fully liquidated.
pub const MIN_BASE_MAX_CLOSE_FACTOR_USD: f64 = 2_000.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveInfo {
    pub asset: Address,
    pub decimals: u8,
    /// e.g. 10500 = 5% bonus.
    pub liquidation_bonus_bps: u32,
    /// Share of the bonus Aave keeps (bps of the bonus, e.g. 1000 = 10%).
    pub protocol_fee_bps: u32,
    /// Oracle price, USD per whole token.
    pub price_usd: f64,
}

impl ReserveInfo {
    pub fn usd(&self, raw: f64) -> f64 {
        raw / 10f64.powi(self.decimals as i32) * self.price_usd
    }
    pub fn raw(&self, usd: f64) -> f64 {
        if self.price_usd <= 0.0 {
            0.0
        } else {
            usd / self.price_usd * 10f64.powi(self.decimals as i32)
        }
    }
}

/// One user's balances per reserve (raw units).
#[derive(Debug, Clone, Default)]
pub struct Position {
    pub user: Address,
    pub health_factor: f64,
    /// (asset, aToken balance) for reserves used as collateral.
    pub collateral: Vec<(Address, f64)>,
    /// (asset, variable + stable debt).
    pub debt: Vec<(Address, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiquidationPlan {
    pub user: Address,
    pub collateral: Address,
    pub debt: Address,
    /// Debt we expect Aave to let us repay (raw units of `debt`).
    pub debt_to_cover: f64,
    /// Collateral we expect to receive (raw units of `collateral`), after Aave's protocol fee.
    pub collateral_out: f64,
    pub debt_usd: f64,
    /// Gross liquidator bonus in USD at oracle prices (before swap costs and gas).
    pub bonus_usd: f64,
}

/// Choose the (collateral, debt) pair that maximizes the liquidator's bonus and size it under Aave
/// v3 rules: close factor 50% (or 100% when HF < 0.95 or the position is small), capped by available
/// collateral. Returns None if the user is not liquidatable or nothing is worth taking.
pub fn plan(pos: &Position, reserves: &[ReserveInfo]) -> Option<LiquidationPlan> {
    if pos.health_factor >= 1.0 {
        return None;
    }
    let info = |a: &Address| reserves.iter().find(|r| r.asset == *a);
    let total_debt_usd: f64 = pos
        .debt
        .iter()
        .filter_map(|(a, amt)| info(a).map(|r| r.usd(*amt)))
        .sum();
    let total_coll_usd: f64 = pos
        .collateral
        .iter()
        .filter_map(|(a, amt)| info(a).map(|r| r.usd(*amt)))
        .sum();
    let full = pos.health_factor < CLOSE_FACTOR_HF_THRESHOLD
        || total_debt_usd < MIN_BASE_MAX_CLOSE_FACTOR_USD
        || total_coll_usd < MIN_BASE_MAX_CLOSE_FACTOR_USD;
    let close_factor = if full { 1.0 } else { 0.5 };

    let mut best: Option<LiquidationPlan> = None;
    for (d_asset, d_amt) in &pos.debt {
        let Some(d) = info(d_asset) else { continue };
        for (c_asset, c_amt) in &pos.collateral {
            let Some(c) = info(c_asset) else { continue };
            let bonus = c.liquidation_bonus_bps as f64 / 10_000.0;
            if bonus <= 1.0 {
                continue;
            }
            // Debt coverable: close factor of this debt, and no more than the collateral can pay for.
            let max_by_debt = d.usd(*d_amt) * close_factor;
            let max_by_coll = c.usd(*c_amt) / bonus;
            let cover_usd = max_by_debt.min(max_by_coll);
            if cover_usd <= 0.0 {
                continue;
            }
            let gross_bonus_usd = cover_usd * (bonus - 1.0);
            let fee_usd = gross_bonus_usd * c.protocol_fee_bps as f64 / 10_000.0;
            let bonus_usd = gross_bonus_usd - fee_usd;
            let collateral_out = c.raw(cover_usd * bonus - fee_usd);
            if best.as_ref().is_none_or(|b| bonus_usd > b.bonus_usd) {
                best = Some(LiquidationPlan {
                    user: pos.user,
                    collateral: *c_asset,
                    debt: *d_asset,
                    debt_to_cover: d.raw(cover_usd),
                    collateral_out,
                    debt_usd: cover_usd,
                    bonus_usd,
                });
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const WETH: Address = address!("0000000000000000000000000000000000000001");
    const USDC: Address = address!("0000000000000000000000000000000000000002");

    fn reserves() -> Vec<ReserveInfo> {
        vec![
            ReserveInfo {
                asset: WETH,
                decimals: 18,
                liquidation_bonus_bps: 10_500,
                protocol_fee_bps: 1_000,
                price_usd: 2_000.0,
            },
            ReserveInfo {
                asset: USDC,
                decimals: 6,
                liquidation_bonus_bps: 10_450,
                protocol_fee_bps: 1_000,
                price_usd: 1.0,
            },
        ]
    }

    fn pos(hf: f64, weth: f64, usdc_debt: f64) -> Position {
        Position {
            user: Address::ZERO,
            health_factor: hf,
            collateral: vec![(WETH, weth * 1e18)],
            debt: vec![(USDC, usdc_debt * 1e6)],
        }
    }

    #[test]
    fn healthy_position_has_no_plan() {
        assert!(plan(&pos(1.01, 10.0, 15_000.0), &reserves()).is_none());
    }

    #[test]
    fn half_close_factor_above_095() {
        let p = plan(&pos(0.98, 10.0, 17_000.0), &reserves()).unwrap();
        assert_eq!(p.collateral, WETH);
        assert!((p.debt_usd - 8_500.0).abs() < 1e-6); // 50% of $17k
        // 5% bonus minus 10% protocol fee on the bonus = 4.5%
        assert!((p.bonus_usd - 8_500.0 * 0.045).abs() < 1e-6);
    }

    #[test]
    fn full_close_factor_below_095_capped_by_collateral() {
        // $20k collateral, $19.5k debt, HF < 0.95: full close, but collateral only covers $20k/1.05.
        let p = plan(&pos(0.90, 10.0, 19_500.0), &reserves()).unwrap();
        assert!((p.debt_usd - 20_000.0 / 1.05).abs() < 1e-6);
    }

    #[test]
    fn small_positions_close_fully() {
        let p = plan(&pos(0.99, 1.0, 1_900.0), &reserves()).unwrap();
        assert!((p.debt_usd - 1_900.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    const WETH: Address = Address::with_last_byte(1);
    const USDC: Address = Address::with_last_byte(2);

    proptest! {
        #[test]
        fn plans_respect_close_factor_collateral_and_fees(
            hf in 0.5f64..0.9999, weth in 0.01f64..1e4, debt in 10.0f64..5e7, px in 100.0f64..1e5,
        ) {
            let reserves = vec![
                ReserveInfo { asset: WETH, decimals: 18, liquidation_bonus_bps: 10_500, protocol_fee_bps: 1_000, price_usd: px },
                ReserveInfo { asset: USDC, decimals: 6, liquidation_bonus_bps: 10_450, protocol_fee_bps: 1_000, price_usd: 1.0 },
            ];
            let pos = Position { user: Address::ZERO, health_factor: hf, collateral: vec![(WETH, weth * 1e18)], debt: vec![(USDC, debt * 1e6)] };
            if let Some(p) = plan(&pos, &reserves) {
                prop_assert!(p.debt_usd <= debt + 1e-6, "never repays more than the debt");
                prop_assert!(p.collateral_out <= weth * 1e18 * (1.0 + 1e-9), "never seizes more than exists");
                prop_assert!(p.bonus_usd >= 0.0);
                prop_assert!(p.bonus_usd <= p.debt_usd * 0.05 + 1e-6, "bonus capped by the 5% incentive");
            }
        }
    }
}
