//! Exact decimal boundaries for token floors. Strategy/price ingestion must supply exact prices
//! before the legacy floating point strategy can be retired.
use crate::Block;
use mev_types::U256;

pub struct Decimal {
    pub numerator: U256,
    pub denominator: U256,
}

impl Decimal {
    pub fn parse(value: &str) -> Result<Self, Block> {
        if value.len() > 160 {
            return Err(Block::InvalidInput);
        }
        let (mantissa, exponent) = match value.split_once(['e', 'E']) {
            Some((m, e)) => (m, e.parse::<i32>().map_err(|_| Block::InvalidInput)?),
            None => (value, 0),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(Block::InvalidInput);
        }
        let digits = format!("{whole}{fraction}");
        let mut numerator = U256::from_str_radix(&digits, 10).map_err(|_| Block::InvalidInput)?;
        let power = exponent
            .checked_sub(fraction.len() as i32)
            .ok_or(Block::InvalidInput)?;
        if !(-77..=77).contains(&power) {
            return Err(Block::InvalidInput);
        }
        let scale = U256::from(10)
            .checked_pow(U256::from(power.unsigned_abs()))
            .ok_or(Block::InvalidInput)?;
        let denominator = if power < 0 {
            scale
        } else {
            numerator = numerator.checked_mul(scale).ok_or(Block::InvalidInput)?;
            U256::ONE
        };
        Ok(Self {
            numerator,
            denominator,
        })
    }
}

pub fn profit_floor(cost_microusd: u64, usd_per_raw: &str) -> Result<U256, Block> {
    let price = Decimal::parse(usd_per_raw)?;
    if price.numerator.is_zero() {
        return Err(Block::InvalidInput);
    }
    let numerator = U256::from(cost_microusd)
        .checked_mul(price.denominator)
        .ok_or(Block::InvalidInput)?;
    let denominator = price
        .numerator
        .checked_mul(U256::from(1_000_000))
        .ok_or(Block::InvalidInput)?;
    let (q, r) = numerator.div_rem(denominator);
    let floor = q
        .checked_add(U256::from(u8::from(!r.is_zero())))
        .ok_or(Block::InvalidInput)?;
    // The packed arbitrage format carries uint128 profit floors.
    if floor > U256::from(u128::MAX) {
        return Err(Block::InvalidInput);
    }
    Ok(floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_liability_rounds_up_and_large_values_remain_exact() {
        assert_eq!(
            profit_floor(20_000, "0.000001").unwrap(),
            U256::from(20_000)
        );
        assert_eq!(profit_floor(1, "0.1").unwrap(), U256::ONE);
        assert_eq!(
            profit_floor(1_000_000, "1e-18").unwrap(),
            U256::from(10).pow(U256::from(18))
        );
        assert_eq!(
            profit_floor(1_000_001, "1e-18").unwrap(),
            U256::from(1_000_001u64) * U256::from(10).pow(U256::from(12))
        );
        for price in ["nan", "inf", "-1", "0", "1e-100", "1.0.0"] {
            assert!(profit_floor(1, price).is_err());
        }
    }
}
