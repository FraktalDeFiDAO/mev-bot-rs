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
    /// Collateral weight in the health factor (bps, e.g. 8300 = 83%).
    #[serde(default)]
    pub liquidation_threshold_bps: u32,
    /// Oracle price, USD per whole token.
    pub price_usd: f64,
    /// The oracle's raw base-currency price (8 decimals); zero when unknown.
    #[serde(default)]
    pub price_raw: alloy::primitives::U256,
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
    /// The same debt balances as exact integers (variable + stable), for amounts that are sent on-chain.
    pub debt_exact: Vec<(Address, alloy::primitives::U256)>,
    /// Exact aToken balances of the collateral reserves.
    pub collateral_exact: Vec<(Address, alloy::primitives::U256)>,
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
                let mut plan = LiquidationPlan {
                    user: pos.user,
                    collateral: *c_asset,
                    debt: *d_asset,
                    debt_to_cover: d.raw(cover_usd),
                    collateral_out,
                    debt_usd: cover_usd,
                    bonus_usd,
                };
                // Replace the float payout with the contract's integer result when exact inputs are known;
                // the debt repaid is capped by the exact balance, never the float estimate.
                if let Some(x) = exact_payout(pos, d, c, plan.debt_to_cover) {
                    plan.debt_to_cover = u256_to_f64(x.debt_needed);
                    plan.collateral_out = u256_to_f64(x.collateral_to_liquidator);
                }
                best = Some(plan);
            }
        }
    }
    best
}

fn u256_to_f64(v: alloy::primitives::U256) -> f64 {
    f64::from(v)
}

/// The contract's integer payout for repaying about `debt_to_cover` raw units of `d` against `c`.
fn exact_payout(
    pos: &Position,
    d: &ReserveInfo,
    c: &ReserveInfo,
    debt_to_cover: f64,
) -> Option<crate::exact::Payout> {
    use alloy::primitives::U256;
    if d.price_raw.is_zero()
        || c.price_raw.is_zero()
        || !debt_to_cover.is_finite()
        || debt_to_cover < 0.0
    {
        return None;
    }
    let debt_balance = pos.debt_exact.iter().find(|(a, _)| *a == d.asset)?.1;
    let coll_balance = pos.collateral_exact.iter().find(|(a, _)| *a == c.asset)?.1;
    // Floor the float to an integer, then cap by the exact debt we know exists.
    let want = if debt_to_cover >= 3.0e38 {
        U256::MAX
    } else {
        U256::from(debt_to_cover as u128)
    };
    crate::exact::available_collateral(&crate::exact::Inputs {
        collateral_price: c.price_raw,
        debt_price: d.price_raw,
        collateral_decimals: c.decimals,
        debt_decimals: d.decimals,
        liquidation_bonus: U256::from(c.liquidation_bonus_bps),
        protocol_fee_bps: U256::from(c.protocol_fee_bps),
        user_collateral_balance: coll_balance,
        debt_to_cover: want.min(debt_balance),
    })
}

/// Predicted health below this triggers a backrun. A little under 1.0 so calibration error doesn't
/// send transactions for positions that stay healthy (each costs gas when the contract reverts).
pub const PREDICT_HF_MAX: f64 = 0.9995;

/// Aave's health factor from balances and prices: Σ collateral·price·LT / Σ debt·price.
/// None without debt. Ignores e-mode; `predict` calibrates that away per user.
pub fn model_hf(pos: &Position, reserves: &[ReserveInfo]) -> Option<f64> {
    let info = |a: &Address| reserves.iter().find(|r| r.asset == *a);
    let debt: f64 = pos
        .debt
        .iter()
        .filter_map(|(a, amt)| info(a).map(|r| r.usd(*amt)))
        .sum();
    if debt <= 0.0 {
        return None;
    }
    let weighted: f64 = pos
        .collateral
        .iter()
        .filter_map(|(a, amt)| {
            info(a).map(|r| r.usd(*amt) * r.liquidation_threshold_bps as f64 / 10_000.0)
        })
        .sum();
    Some(weighted / debt)
}

/// A position cached for prediction: balances, plus `calibration` = on-chain HF / `model_hf` at
/// snapshot time (absorbs e-mode thresholds and rounding).
#[derive(Debug, Clone)]
pub struct Cached {
    pub pos: Position,
    pub calibration: f64,
}

impl Cached {
    /// Calibrate against the on-chain health factor in `pos`.
    pub fn new(pos: Position, reserves: &[ReserveInfo]) -> Option<Self> {
        let model = model_hf(&pos, reserves)?;
        (model > 0.0).then(|| Self {
            calibration: pos.health_factor / model,
            pos,
        })
    }
}

/// Apply a price move: every asset in `assets` is repriced by `ratio` (new / old aggregator
/// answer) in `reserves`, which keeps the move for later updates. Returns the cached positions
/// that hold one of `assets` and this move pushes under water (healthy before, liquidatable
/// after), with their liquidation plans. Positions already under water are the regular
/// re-poll's job, not a race against the price update.
pub fn predict(
    reserves: &mut [ReserveInfo],
    cached: &[Cached],
    assets: &[Address],
    ratio: f64,
) -> Vec<(Position, LiquidationPlan)> {
    if !(ratio.is_finite() && ratio > 0.0) {
        return vec![];
    }
    let hf = |c: &Cached, reserves: &[ReserveInfo]| {
        model_hf(&c.pos, reserves).map(|h| h * c.calibration)
    };
    let exposed = |p: &Position| {
        p.collateral
            .iter()
            .chain(&p.debt)
            .any(|(a, _)| assets.contains(a))
    };
    let candidates: Vec<&Cached> = cached
        .iter()
        .filter(|c| exposed(&c.pos) && hf(c, reserves).is_some_and(|h| h >= PREDICT_HF_MAX))
        .collect();
    for r in reserves.iter_mut().filter(|r| assets.contains(&r.asset)) {
        r.price_usd *= ratio;
    }
    candidates
        .into_iter()
        .filter_map(|c| {
            let hf = hf(c, reserves)?;
            if hf >= PREDICT_HF_MAX {
                return None;
            }
            let pos = Position {
                health_factor: hf,
                ..c.pos.clone()
            };
            let plan = plan(&pos, reserves)?;
            Some((pos, plan))
        })
        .collect()
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
                liquidation_threshold_bps: 8_300,
                price_usd: 2_000.0,
                price_raw: Default::default(),
            },
            ReserveInfo {
                asset: USDC,
                decimals: 6,
                liquidation_bonus_bps: 10_450,
                protocol_fee_bps: 1_000,
                liquidation_threshold_bps: 7_800,
                price_usd: 1.0,
                price_raw: Default::default(),
            },
        ]
    }

    fn pos(hf: f64, weth: f64, usdc_debt: f64) -> Position {
        Position {
            user: Address::ZERO,
            health_factor: hf,
            collateral: vec![(WETH, weth * 1e18)],
            debt: vec![(USDC, usdc_debt * 1e6)],
            ..Default::default()
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
    #[test]
    fn model_hf_weights_collateral_by_threshold() {
        // 10 WETH @ $2000, LT 83% = $16.6k weighted; $15k debt.
        let hf = model_hf(&pos(0.0, 10.0, 15_000.0), &reserves()).unwrap();
        assert!((hf - 16_600.0 / 15_000.0).abs() < 1e-9);
        assert!(model_hf(&pos(0.0, 10.0, 0.0), &reserves()).is_none());
    }

    #[test]
    fn predict_flags_positions_a_price_drop_makes_liquidatable() {
        let mut rs = reserves();
        // On-chain HF 1.122 vs model 1.1067: calibration (e.g. e-mode) carries through.
        let c = Cached::new(pos(1.122, 10.0, 15_000.0), &rs).unwrap();
        let cached = vec![c.clone()];
        // ETH -5%: model 1.0513 * 1.0138 = 1.066 -> still healthy.
        assert!(predict(&mut rs, &cached, &[WETH], 0.95).is_empty());
        // A further -10% (prices persist across updates): 0.959 -> liquidatable, planned.
        let hits = predict(&mut rs, &cached, &[WETH], 0.90);
        assert_eq!(hits.len(), 1);
        let (p, plan) = &hits[0];
        assert!(p.health_factor < PREDICT_HF_MAX && p.health_factor > 0.9);
        assert_eq!((plan.collateral, plan.debt), (WETH, USDC));
        assert!((rs[0].price_usd - 2_000.0 * 0.95 * 0.90).abs() < 1e-6);
        // Already under water: a further drop is not a new crossing.
        assert!(predict(&mut rs, &cached, &[WETH], 0.99).is_empty());
    }

    #[test]
    fn predict_ignores_unexposed_positions_and_bad_ratios() {
        let mut rs = reserves();
        let cached = vec![Cached::new(pos(1.01, 10.0, 16_000.0), &rs).unwrap()];
        let other = Address::with_last_byte(9);
        assert!(predict(&mut rs, &cached, &[other], 0.5).is_empty());
        assert!(predict(&mut rs, &cached, &[WETH], f64::NAN).is_empty());
        assert!(predict(&mut rs, &cached, &[WETH], 0.0).is_empty());
        assert_eq!(
            rs[0].price_usd, 2_000.0,
            "rejected ratios don't move prices"
        );
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
                ReserveInfo { asset: WETH, decimals: 18, liquidation_bonus_bps: 10_500, protocol_fee_bps: 1_000, liquidation_threshold_bps: 8_300, price_usd: px, price_raw: Default::default() },
                ReserveInfo { asset: USDC, decimals: 6, liquidation_bonus_bps: 10_450, protocol_fee_bps: 1_000, liquidation_threshold_bps: 7_800, price_usd: 1.0, price_raw: Default::default() },
            ];
            let pos = Position { user: Address::ZERO, health_factor: hf, collateral: vec![(WETH, weth * 1e18)], debt: vec![(USDC, debt * 1e6)], ..Default::default() };
            if let Some(p) = plan(&pos, &reserves) {
                prop_assert!(p.debt_usd <= debt + 1e-6, "never repays more than the debt");
                prop_assert!(p.collateral_out <= weth * 1e18 * (1.0 + 1e-9), "never seizes more than exists");
                prop_assert!(p.bonus_usd >= 0.0);
                prop_assert!(p.bonus_usd <= p.debt_usd * 0.05 + 1e-6, "bonus capped by the 5% incentive");
            }
        }
    }

    #[test]
    fn exact_inputs_replace_the_float_payout_and_cap_repayment_at_the_real_debt() {
        use alloy::primitives::U256;
        let reserves = vec![
            ReserveInfo {
                asset: WETH,
                decimals: 18,
                liquidation_bonus_bps: 10_500,
                protocol_fee_bps: 1_000,
                liquidation_threshold_bps: 8_300,
                price_usd: 2000.0,
                price_raw: U256::from(2_000u64 * 100_000_000),
            },
            ReserveInfo {
                asset: USDC,
                decimals: 6,
                liquidation_bonus_bps: 10_450,
                protocol_fee_bps: 1_000,
                liquidation_threshold_bps: 7_800,
                price_usd: 1.0,
                price_raw: U256::from(100_000_000u64),
            },
        ];
        let mut p = Position {
            user: Address::ZERO,
            health_factor: 0.9,
            collateral: vec![(WETH, 10e18)],
            debt: vec![(USDC, 1_000e6)],
            ..Default::default()
        };
        p.debt_exact = vec![(USDC, U256::from(1_000_000_000u64))];
        p.collateral_exact = vec![(WETH, U256::from(10u128 * 10u128.pow(18)))];
        let plan = plan(&p, &reserves).unwrap();
        let x = crate::exact::available_collateral(&crate::exact::Inputs {
            collateral_price: reserves[0].price_raw,
            debt_price: reserves[1].price_raw,
            collateral_decimals: 18,
            debt_decimals: 6,
            liquidation_bonus: U256::from(10_500u64),
            protocol_fee_bps: U256::from(1_000u64),
            user_collateral_balance: p.collateral_exact[0].1,
            debt_to_cover: U256::from(1_000_000_000u64),
        })
        .unwrap();
        assert_eq!(plan.collateral_out, f64::from(x.collateral_to_liquidator));
        assert_eq!(plan.debt_to_cover, f64::from(x.debt_needed));
    }
}
