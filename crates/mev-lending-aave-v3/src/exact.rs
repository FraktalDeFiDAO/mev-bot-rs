//! Integer-exact Aave v3 liquidation payout (invariant 1): a port of
//! `LiquidationLogic._calculateAvailableCollateralToLiquidate` with `PercentageMath` rounding
//! (half-up `percentMul` / `percentDiv`). Oracle prices are the raw base-currency integers the
//! oracle returns. `None` is where the contract would revert (zero price, overflow).

use alloy::primitives::U256;

const PERCENTAGE_FACTOR: u64 = 10_000;

/// `PercentageMath.percentMul`: `(value * pct + 5000) / 10000`.
pub fn percent_mul(value: U256, pct: U256) -> Option<U256> {
    if pct.is_zero() {
        return Some(U256::ZERO);
    }
    value
        .checked_mul(pct)?
        .checked_add(U256::from(PERCENTAGE_FACTOR / 2))?
        .checked_div(U256::from(PERCENTAGE_FACTOR))
}

/// `PercentageMath.percentDiv`: `(value * 10000 + pct/2) / pct`.
pub fn percent_div(value: U256, pct: U256) -> Option<U256> {
    if pct.is_zero() {
        return None;
    }
    value
        .checked_mul(U256::from(PERCENTAGE_FACTOR))?
        .checked_add(pct / U256::from(2u8))?
        .checked_div(pct)
}

const RAY: u128 = 1_000_000_000_000_000_000_000_000_000;
/// `MathUtils.SECONDS_PER_YEAR` (365 days).
const SECONDS_PER_YEAR: u64 = 365 * 24 * 3600;

/// `WadRayMath.rayMul`: half-up.
pub fn ray_mul(a: U256, b: U256) -> Option<U256> {
    a.checked_mul(b)?
        .checked_add(U256::from(RAY / 2))?
        .checked_div(U256::from(RAY))
}

/// `rayMul` rounded up: at least what any `rayMul` rounding gives (for amounts that must cover).
pub fn ray_mul_up(a: U256, b: U256) -> Option<U256> {
    Some(a.checked_mul(b)?.div_ceil(U256::from(RAY)))
}

/// `MathUtils.calculateCompoundedInterest(rate, lastUpdate, now)` (ray), as in aave-v3-origin:
/// `x = rate·exp / SECONDS_PER_YEAR; RAY + x + rayMul(x, x/2 + rayMul(x, x/6))` (checked against
/// the Arbitrum pool's `getReserveNormalizedVariableDebt`, 2026-10-23; the older three-term
/// binomial form was off by ~2e-14).
pub fn compounded_interest(rate: U256, last_update: u64, now: u64) -> Option<U256> {
    let exp = U256::from(now.checked_sub(last_update)?);
    if exp.is_zero() {
        return Some(U256::from(RAY));
    }
    let x = rate.checked_mul(exp)? / U256::from(SECONDS_PER_YEAR);
    let inner = (x / U256::from(2u8)).checked_add(ray_mul(x, x / U256::from(6u8))?)?;
    U256::from(RAY)
        .checked_add(x)?
        .checked_add(ray_mul(x, inner)?)
}

/// Variable debt reserve state (`AaveProtocolDataProvider.getReserveData`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariableDebtState {
    pub rate: U256,
    pub index: U256,
    pub last_update: u64,
}

impl VariableDebtState {
    /// `ReserveLogic.getNormalizedDebt` at `now`.
    pub fn normalized_at(&self, now: u64) -> Option<U256> {
        if now <= self.last_update {
            return Some(self.index);
        }
        ray_mul(
            compounded_interest(self.rate, self.last_update, now)?,
            self.index,
        )
    }
}

/// Debt a position will owe at `at` (unix s): scaled variable debt at the normalized debt then,
/// plus stable principal compounded at the user's stable rate. Rounded up, so it covers what
/// `liquidationCall` can pull when it lands by `at`.
pub fn debt_at(
    scaled_variable: U256,
    reserve: &VariableDebtState,
    stable_principal: U256,
    stable_rate: U256,
    stable_last_update: u64,
    at: u64,
) -> Option<U256> {
    let variable = ray_mul_up(scaled_variable, reserve.normalized_at(at)?)?;
    let stable = if stable_principal.is_zero() {
        U256::ZERO
    } else {
        ray_mul_up(
            stable_principal,
            compounded_interest(stable_rate, stable_last_update, at.max(stable_last_update))?,
        )?
    };
    variable.checked_add(stable)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payout {
    /// Collateral the liquidator receives (after the protocol fee).
    pub collateral_to_liquidator: U256,
    /// Debt actually repaid (less than requested when the collateral runs out).
    pub debt_needed: U256,
    pub protocol_fee: U256,
}

pub struct Inputs {
    pub collateral_price: U256,
    pub debt_price: U256,
    pub collateral_decimals: u8,
    pub debt_decimals: u8,
    /// E.g. 10500 for a 5% bonus.
    pub liquidation_bonus: U256,
    /// Share of the bonus collateral the protocol keeps, in bps (the reserve's `liquidationProtocolFee`).
    pub protocol_fee_bps: U256,
    pub user_collateral_balance: U256,
    pub debt_to_cover: U256,
}

pub fn available_collateral(i: &Inputs) -> Option<Payout> {
    if i.collateral_price.is_zero()
        || i.debt_price.is_zero()
        || i.collateral_decimals > 77
        || i.debt_decimals > 77
    {
        return None;
    }
    let ten = U256::from(10u8);
    let col_unit = ten.checked_pow(U256::from(i.collateral_decimals))?;
    let debt_unit = ten.checked_pow(U256::from(i.debt_decimals))?;
    let base = i
        .debt_price
        .checked_mul(i.debt_to_cover)?
        .checked_mul(col_unit)?
        .checked_div(i.collateral_price.checked_mul(debt_unit)?)?;
    let max_collateral = percent_mul(base, i.liquidation_bonus)?;
    let (collateral_amount, debt_needed) = if max_collateral > i.user_collateral_balance {
        let needed = i
            .collateral_price
            .checked_mul(i.user_collateral_balance)?
            .checked_mul(debt_unit)?
            .checked_div(i.debt_price.checked_mul(col_unit)?)?;
        (
            i.user_collateral_balance,
            percent_div(needed, i.liquidation_bonus)?,
        )
    } else {
        (max_collateral, i.debt_to_cover)
    };
    if i.protocol_fee_bps.is_zero() {
        return Some(Payout {
            collateral_to_liquidator: collateral_amount,
            debt_needed,
            protocol_fee: U256::ZERO,
        });
    }
    let bonus_collateral =
        collateral_amount.checked_sub(percent_div(collateral_amount, i.liquidation_bonus)?)?;
    let protocol_fee = percent_mul(bonus_collateral, i.protocol_fee_bps)?;
    Some(Payout {
        collateral_to_liquidator: collateral_amount.checked_sub(protocol_fee)?,
        debt_needed,
        protocol_fee,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> U256 {
        U256::from(n)
    }

    fn base() -> Inputs {
        Inputs {
            collateral_price: u(2_000 * 10u128.pow(8)), // WETH $2000, 8-decimal oracle
            debt_price: u(10u128.pow(8)),               // USDC $1
            collateral_decimals: 18,
            debt_decimals: 6,
            liquidation_bonus: u(10_500),
            protocol_fee_bps: u(1_000),
            user_collateral_balance: u(10 * 10u128.pow(18)),
            debt_to_cover: u(1_000 * 10u128.pow(6)),
        }
    }

    #[test]
    fn percent_math_rounds_half_up_like_the_library() {
        assert_eq!(percent_mul(u(1), u(5_000)), Some(u(1))); // 0.5 rounds up
        assert_eq!(percent_mul(u(1), u(4_999)), Some(u(0)));
        assert_eq!(percent_div(u(10_500), u(10_500)), Some(u(10_000)));
        assert_eq!(percent_div(u(1), U256::ZERO), None);
    }

    #[test]
    fn a_thousand_dollars_of_debt_pays_out_the_bonus_less_the_protocol_cut() {
        let p = available_collateral(&base()).unwrap();
        // $1000 / $2000 = 0.5 WETH base; x1.05 = 0.525 WETH; bonus part = 0.525 - 0.5 = 0.025; protocol keeps 10% = 0.0025.
        assert_eq!(p.debt_needed, u(1_000 * 10u128.pow(6)));
        assert_eq!(p.protocol_fee, u(25 * 10u128.pow(14)));
        assert_eq!(p.collateral_to_liquidator, u(5_225 * 10u128.pow(14)));
    }

    #[test]
    fn when_collateral_runs_out_the_repayment_shrinks_to_match() {
        let mut i = base();
        i.user_collateral_balance = u(21 * 10u128.pow(16)); // 0.21 WETH = $420 of collateral
        let p = available_collateral(&i).unwrap();
        // 0.21 WETH is worth $420; at a 5% bonus that covers $400 of debt.
        assert_eq!(p.debt_needed, u(400 * 10u128.pow(6)));
        assert!(p.collateral_to_liquidator < i.user_collateral_balance);
    }

    #[test]
    fn zero_prices_revert() {
        let mut i = base();
        i.collateral_price = U256::ZERO;
        assert_eq!(available_collateral(&i), None);
    }
}

#[cfg(test)]
mod debt_tests {
    use super::*;

    #[test]
    fn debt_at_rounds_up_and_grows_with_time() {
        let ray = U256::from(RAY);
        let reserve = VariableDebtState {
            rate: ray / U256::from(20u8),        // 5% a year
            index: ray + ray / U256::from(10u8), // 1.1
            last_update: 1_000,
        };
        // No time elapsed: scaled x index, rounded up (1_000_001 x 1.1 = 1_100_001.1 -> ..02).
        let now = debt_at(
            U256::from(1_000_001u64),
            &reserve,
            U256::ZERO,
            U256::ZERO,
            0,
            1_000,
        );
        assert_eq!(now, Some(U256::from(1_100_002u64)));
        // A year later at 5%: about 1.0513 x (continuous compounding, Aave's expansion).
        let later = debt_at(
            U256::from(1_000_000u64),
            &reserve,
            U256::ZERO,
            U256::ZERO,
            0,
            1_000 + SECONDS_PER_YEAR,
        )
        .unwrap();
        assert!(
            later > U256::from(1_156_390u64) && later < U256::from(1_156_400u64),
            "{later}"
        );
        // Stable principal adds its own compounding.
        let with_stable = debt_at(
            U256::from(1_000_000u64),
            &reserve,
            U256::from(500u64),
            ray,
            1_000,
            1_000,
        )
        .unwrap();
        assert_eq!(with_stable, U256::from(1_100_000u64 + 500));
    }
}
