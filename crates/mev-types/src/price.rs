//! Positive rational prices. Integer rounding is explicit; unsupported widths fail closed.
use alloy::primitives::{U256, U512};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ratio {
    numerator: U512,
    denominator: U512,
}

fn gcd(mut a: U512, mut b: U512) -> U512 {
    while !b.is_zero() {
        (a, b) = (b, a % b);
    }
    a
}

fn narrow(n: U512) -> Option<U256> {
    U256::checked_from_limbs_slice(n.as_limbs())
}

impl Ratio {
    pub fn new(numerator: U512, denominator: U512) -> Option<Self> {
        if numerator.is_zero() || denominator.is_zero() {
            return None;
        }
        let g = gcd(numerator, denominator);
        Some(Self {
            numerator: numerator / g,
            denominator: denominator / g,
        })
    }

    pub fn inverse(&self) -> Self {
        Self {
            numerator: self.denominator,
            denominator: self.numerator,
        }
    }

    pub fn multiply(&self, other: &Self) -> Option<Self> {
        let a = gcd(self.numerator, other.denominator);
        let b = gcd(other.numerator, self.denominator);
        Self::new(
            (self.numerator / a).checked_mul(other.numerator / b)?,
            (self.denominator / b).checked_mul(other.denominator / a)?,
        )
    }

    /// Output units rounded down for proceeds or up for liabilities.
    pub fn value(&self, amount: U256, round_up: bool) -> Option<U256> {
        let amount = U512::from(amount);
        let g = gcd(amount, self.denominator);
        let numerator = (amount / g).checked_mul(self.numerator)?;
        let denominator = self.denominator / g;
        let (whole, remainder) = numerator.div_rem(denominator);
        narrow(whole.checked_add(U512::from(u8::from(round_up && !remainder.is_zero())))?)
    }

    pub fn microusd(&self, amount: U256, round_up: bool) -> Option<u64> {
        u64::try_from(self.value(amount, round_up)?).ok()
    }

    /// Raw token units needed for an integer USD cost, rounded upward.
    pub fn raw_for_cost(&self, cost_microusd: u64) -> Option<U256> {
        self.inverse().value(U256::from(cost_microusd), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_floors_and_proceeds_round_in_opposite_directions() {
        let px = Ratio::new(U512::from(2u8), U512::from(3u8)).unwrap();
        assert_eq!(px.microusd(U256::from(1u8), false), Some(0));
        assert_eq!(px.microusd(U256::from(1u8), true), Some(1));
        assert_eq!(px.raw_for_cost(1), Some(U256::from(2u8)));
        assert_eq!(Ratio::new(U512::ZERO, U512::from(1u8)), None);
        assert_eq!(Ratio::new(U512::from(1u8), U512::ZERO), None);
    }
    #[test]
    fn values_above_float_precision_and_cross_cancellation_remain_exact() {
        let amount = U256::from(9_007_199_254_740_993u64);
        let unit = Ratio::new(U512::from(1u8), U512::from(1u8)).unwrap();
        assert_eq!(unit.value(amount, false), Some(amount));
        let huge = U512::from(1u8) << 400;
        let a = Ratio::new(huge, U512::from(3u8)).unwrap();
        let b = Ratio::new(U512::from(3u8), huge).unwrap();
        assert_eq!(a.multiply(&b), Some(unit));
    }
}
