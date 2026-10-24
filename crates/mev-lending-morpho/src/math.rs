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

/// Debt one liquidation can repay, in the same unit as both inputs: all of it, unless the
/// collateral runs out first. Morpho seizes `repaid × incentive`, so an underwater position
/// (collateral below debt × incentive) repays at most `collateral / incentive`; the rest is bad debt.
pub fn max_repay(debt: f64, collateral: f64, incentive: f64) -> f64 {
    debt.min(collateral / incentive).max(0.0)
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
    fn max_repay_is_capped_by_seizable_collateral() {
        assert_eq!(max_repay(100.0, 200.0, 1.05), 100.0); // healthy collateral: all debt
        // Underwater: 3,719 collateral against 131,396 debt (Base USR, 2026-10-24).
        let r = max_repay(131_396.0, 3_719.0, 1.026);
        assert!((r * 1.026 - 3_719.0).abs() < 1e-9);
        assert_eq!(max_repay(10.0, 0.0, 1.05), 0.0);
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

/// Integer-exact Morpho Blue liquidation math (invariant 1): `MathLib` / `SharesMathLib` rounding,
/// WAD = 1e18. `None` where the contract would revert or overflow.
pub mod exact {
    use alloy::primitives::U256;

    const WAD: u128 = 1_000_000_000_000_000_000;
    const SCALE: u128 = 1_000_000_000_000_000_000_000_000_000_000_000_000; // ORACLE_PRICE_SCALE = 1e36
    const VIRTUAL_SHARES: u128 = 1_000_000;
    const VIRTUAL_ASSETS: u128 = 1;
    const MAX_LIF: u128 = 1_150_000_000_000_000_000;
    const CURSOR: u128 = 300_000_000_000_000_000;

    fn mul_div_down(a: U256, b: U256, d: U256) -> Option<U256> {
        a.checked_mul(b)?.checked_div(d)
    }

    fn mul_div_up(a: U256, b: U256, d: U256) -> Option<U256> {
        if d.is_zero() {
            return None;
        }
        a.checked_mul(b)?
            .checked_add(d - U256::from(1u8))?
            .checked_div(d)
    }

    fn w_mul_down(a: U256, b: U256) -> Option<U256> {
        mul_div_down(a, b, U256::from(WAD))
    }

    fn w_div_up(a: U256, b: U256) -> Option<U256> {
        mul_div_up(a, U256::from(WAD), b)
    }

    /// `min(MAX_LIF, WAD / (WAD - CURSOR * (WAD - lltv)))`, rounding down.
    pub fn incentive_factor(lltv: U256) -> Option<U256> {
        let wad = U256::from(WAD);
        let gap = wad.checked_sub(lltv)?;
        let denom = wad.checked_sub(w_mul_down(U256::from(CURSOR), gap)?)?;
        let lif = mul_div_down(wad, wad, denom)?;
        Some(lif.min(U256::from(MAX_LIF)))
    }

    /// `SharesMathLib.toAssetsUp`.
    pub fn borrow_assets_up(shares: U256, total_assets: U256, total_shares: U256) -> Option<U256> {
        mul_div_up(
            shares,
            total_assets.checked_add(U256::from(VIRTUAL_ASSETS))?,
            total_shares.checked_add(U256::from(VIRTUAL_SHARES))?,
        )
    }

    /// `Morpho._isHealthy`.
    pub fn is_healthy(collateral: U256, borrowed: U256, price: U256, lltv: U256) -> Option<bool> {
        let max_borrow = w_mul_down(mul_div_down(collateral, price, U256::from(SCALE))?, lltv)?;
        Some(max_borrow >= borrowed)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Size {
        pub seized: U256,
        /// Loan assets the liquidation repays at the read totals (`toAssetsUp(repaidShares)`).
        pub repaid: U256,
        /// Loan assets to flash-borrow: covers the repayment whenever the liquidation lands.
        pub flash: U256,
    }

    /// `SharesMathLib.toSharesUp`.
    pub fn to_shares_up(assets: U256, total_assets: U256, total_shares: U256) -> Option<U256> {
        mul_div_up(
            assets,
            total_shares.checked_add(U256::from(VIRTUAL_SHARES))?,
            total_assets.checked_add(U256::from(VIRTUAL_ASSETS))?,
        )
    }

    /// Shares `Morpho.liquidate(seizedAssets = seized, repaidShares = 0)` repays:
    /// `toSharesUp(wDivUp(mulDivUp(seized, price, 1e36), LIF))`.
    fn repaid_shares(seized: U256, price: U256, lif: U256, ta: U256, ts: U256) -> Option<U256> {
        to_shares_up(
            w_div_up(mul_div_up(seized, price, U256::from(SCALE))?, lif)?,
            ta,
            ts,
        )
    }

    /// Largest liquidation, exactly: the most collateral (up to all of it) whose repaid shares fit in
    /// the borrower's `borrow_shares` (`liquidate` reverts beyond that). Checked at the read totals
    /// `ta`/`ts`: interest accrued before the liquidation lands raises assets per share, so it only
    /// lowers the shares a given seize repays, and this bound stays valid.
    ///
    /// Flash amount: `toAssetsUp(toSharesUp(q))` exceeds the quoted repay `q` by less than one
    /// share's worth of assets plus one unit (two round-ups). Assets per share grow by interest only
    /// (~1e-9 per second), so twice its read value bounds it at any realistic landing time.
    pub fn size(
        collateral: U256,
        borrow_shares: U256,
        ta: U256,
        ts: U256,
        price: U256,
        lltv: U256,
    ) -> Option<Size> {
        if collateral.is_zero() || borrow_shares.is_zero() || price.is_zero() {
            return None;
        }
        let lif = incentive_factor(lltv)?;
        let fits =
            |s: U256| repaid_shares(s, price, lif, ta, ts).is_some_and(|r| r <= borrow_shares);
        let (mut lo, mut hi) = (U256::ZERO, collateral);
        if fits(hi) {
            lo = hi;
        } else {
            // Invariant: fits(lo) (zero repays zero shares), !fits(hi).
            while hi - lo > U256::from(1u8) {
                let mid = lo + (hi - lo) / U256::from(2u8);
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        let seized = lo;
        if seized.is_zero() {
            return None;
        }
        let quoted = w_div_up(mul_div_up(seized, price, U256::from(SCALE))?, lif)?;
        let shares = to_shares_up(quoted, ta, ts)?;
        let per_share = mul_div_up(
            U256::from(1u8),
            ta.checked_add(U256::from(VIRTUAL_ASSETS))?,
            ts.checked_add(U256::from(VIRTUAL_SHARES))?,
        )?;
        Some(Size {
            seized,
            repaid: borrow_assets_up(shares, ta, ts)?,
            flash: quoted
                .checked_add(per_share.checked_mul(U256::from(2u8))?)?
                .checked_add(U256::from(1u8))?,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn u(n: u128) -> U256 {
            U256::from(n)
        }

        #[test]
        fn the_incentive_factor_is_capped_and_matches_the_formula() {
            // lltv 0.86: 1 / (1 - 0.3*0.14) = 1.043841336...
            assert_eq!(
                incentive_factor(u(860_000_000_000_000_000)),
                Some(u(1_043_841_336_116_910_229))
            );
            assert_eq!(incentive_factor(U256::ZERO), Some(u(MAX_LIF)));
            assert_eq!(incentive_factor(u(WAD + 1)), None);
        }

        #[test]
        fn health_uses_the_contract_rounding_at_the_boundary() {
            let price = u(2 * 10u128.pow(27)); // 1 collateral (18 dec) = $2000 of a 6-dec loan token
            let lltv = u(860_000_000_000_000_000);
            assert_eq!(
                is_healthy(u(10u128.pow(18)), u(1_720_000_000), price, lltv),
                Some(true)
            );
            assert_eq!(
                is_healthy(u(10u128.pow(18)), u(1_720_000_001), price, lltv),
                Some(false)
            );
        }

        #[test]
        fn shares_round_up_with_virtual_offsets() {
            assert_eq!(
                borrow_assets_up(u(1_000_000), u(1_000), u(1_000_000_000)),
                Some(u(1))
            );
        }

        #[test]
        fn size_is_the_exact_maximum_the_contract_accepts() {
            let price = u(2 * 10u128.pow(27)); // 1 collateral (18 dec) = 2000 loan units (6 dec)
            let lltv = u(860_000_000_000_000_000);
            let lif = incentive_factor(lltv).unwrap();
            // One borrower owes the whole market: 1,750 loan tokens, shares at 1e6 per asset.
            let (ta, ts) = (u(1_750_000_000), u(1_750_000_000 * 1_000_000));
            // Debt binds: the seize is maximal (one more unit would repay more shares than owed).
            let s = size(u(10 * 10u128.pow(18)), ts, ta, ts, price, lltv).unwrap();
            assert!(repaid_shares(s.seized, price, lif, ta, ts).unwrap() <= ts);
            assert!(repaid_shares(s.seized + u(1), price, lif, ta, ts).unwrap() > ts);
            assert!(s.repaid <= ta + u(1), "never repays more than the debt");
            // Collateral binds: all of it is seized, no haircut.
            let t = size(u(5 * 10u128.pow(17)), ts, ta, ts, price, lltv).unwrap();
            assert_eq!(t.seized, u(5 * 10u128.pow(17)));
            // The flash covers the repayment, within two round-ups of the quote.
            let q = w_div_up(mul_div_up(t.seized, price, u(SCALE)).unwrap(), lif).unwrap();
            assert!(t.flash >= t.repaid && t.repaid >= q);
            assert_eq!(size(U256::ZERO, ts, ta, ts, price, lltv), None);
        }
    }
}
