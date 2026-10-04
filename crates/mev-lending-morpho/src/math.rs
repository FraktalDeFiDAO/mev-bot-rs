//! Morpho Blue liquidation math (mirrors `Morpho.sol` / `MathLib` / `SharesMathLib`). Pure.
//!
//! Units: raw token amounts as f64; oracle price = collateral price in loan-token raw units,
//! scaled by 1e36 (Morpho's `ORACLE_PRICE_SCALE`).

pub const ORACLE_PRICE_SCALE: f64 = 1e36;
pub const VIRTUAL_SHARES: f64 = 1e6;
pub const VIRTUAL_ASSETS: f64 = 1.0;
pub const MAX_LIQUIDATION_INCENTIVE_FACTOR: f64 = 1.15;
pub const LIQUIDATION_CURSOR: f64 = 0.3;
/// Seize slightly less than the maximum so share rounding can't push repayment over the debt.
const SEIZE_HAIRCUT: f64 = 0.999;

/// Liquidation incentive factor for a market's LLTV (fraction, e.g. 0.86).
pub fn incentive_factor(lltv: f64) -> f64 {
    (1.0 / (1.0 - LIQUIDATION_CURSOR * (1.0 - lltv))).min(MAX_LIQUIDATION_INCENTIVE_FACTOR)
}

/// Borrow shares -> assets, rounding up (`toAssetsUp`).
pub fn borrow_assets(shares: f64, total_assets: f64, total_shares: f64) -> f64 {
    (shares * (total_assets + VIRTUAL_ASSETS) / (total_shares + VIRTUAL_SHARES)).ceil()
}

/// Health factor: max borrow / borrowed. < 1 ⇔ Morpho `_isHealthy` is false.
pub fn health_factor(collateral: f64, borrowed: f64, price: f64, lltv: f64) -> f64 {
    if borrowed <= 0.0 {
        return f64::INFINITY;
    }
    collateral * price / ORACLE_PRICE_SCALE * lltv / borrowed
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidationSize {
    /// Collateral to seize (raw).
    pub seized: f64,
    /// Loan token the liquidator repays (raw).
    pub repaid: f64,
    /// Liquidator bonus in loan-token raw units (seized value − repaid).
    pub bonus: f64,
}

/// Largest liquidation: seize as much collateral as the debt allows (Morpho has no close factor).
pub fn size(collateral: f64, borrowed: f64, price: f64, lltv: f64) -> Option<LiquidationSize> {
    if collateral <= 0.0 || borrowed <= 0.0 || price <= 0.0 {
        return None;
    }
    let lif = incentive_factor(lltv);
    let seize_by_debt = borrowed * lif * ORACLE_PRICE_SCALE / price;
    let seized = (collateral.min(seize_by_debt) * SEIZE_HAIRCUT).floor();
    if seized < 1.0 {
        return None;
    }
    let seized_value = seized * price / ORACLE_PRICE_SCALE;
    let repaid = (seized_value / lif).ceil();
    Some(LiquidationSize {
        seized,
        repaid,
        bonus: seized_value - repaid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incentive_factor_matches_morpho_formula() {
        assert!((incentive_factor(0.86) - 1.0 / (1.0 - 0.3 * 0.14)).abs() < 1e-12); // ~4.38%
        assert!((incentive_factor(0.915) - 1.0 / (1.0 - 0.3 * 0.085)).abs() < 1e-12); // ~2.62%
        assert_eq!(incentive_factor(0.0), MAX_LIQUIDATION_INCENTIVE_FACTOR); // capped
    }

    #[test]
    fn shares_round_up_with_virtual_offsets() {
        // 1e6 shares when totals are 1000 assets / 1e9 shares -> (1e6*1001)/(1e9+1e6) = 0.99 -> 1.
        assert_eq!(borrow_assets(1e6, 1000.0, 1e9), 1.0);
    }

    /// Price of 1 collateral raw unit in loan raw units, scaled 1e36. Collateral 18 dec at $2,000,
    /// loan 6 dec: 1 raw collateral = 2000e6 / 1e18 = 2e-9 raw loan -> × 1e36 = 2e27.
    const PRICE: f64 = 2e27;

    #[test]
    fn health_factor_threshold() {
        // 1 collateral ($2,000), lltv 0.86 -> max borrow $1,720.
        assert!(health_factor(1e18, 1_700e6, PRICE, 0.86) > 1.0);
        assert!(health_factor(1e18, 1_750e6, PRICE, 0.86) < 1.0);
        assert!(health_factor(1e18, 0.0, PRICE, 0.86).is_infinite());
    }

    #[test]
    fn size_is_capped_by_debt_and_bonus_matches_incentive() {
        let lif = incentive_factor(0.86);
        // Plenty of collateral: seize what repays the whole $1,750 debt.
        let s = size(10e18, 1_750e6, PRICE, 0.86).unwrap();
        assert!(s.repaid <= 1_750e6 + 1.0);
        assert!((s.bonus / s.repaid - (lif - 1.0)).abs() < 1e-6);
        // Little collateral: seize all of it (minus haircut).
        let t = size(0.5e18, 1_750e6, PRICE, 0.86).unwrap();
        assert!((t.seized - (0.5e18 * 0.999_f64).floor()).abs() < 1.0);
        assert!(size(0.0, 1.0, PRICE, 0.86).is_none());
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn sizes_never_overpay_or_overseize(coll in 1e3f64..1e24, borrowed in 1e3f64..1e15, price in 1e20f64..1e30, lltv in 0.3f64..0.98) {
            let lif = incentive_factor(lltv);
            prop_assert!(lif > 1.0 && lif <= MAX_LIQUIDATION_INCENTIVE_FACTOR);
            if let Some(s) = size(coll, borrowed, price, lltv) {
                prop_assert!(s.seized <= coll);
                prop_assert!(s.repaid <= borrowed * (1.0 + 1e-9) + 1.0);
                prop_assert!(s.bonus >= -1.0, "bonus non-negative up to rounding");
            }
        }
    }
}
