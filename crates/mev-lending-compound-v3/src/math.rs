//! Comet liquidation math. Pure — no I/O.
//!
//! Prices are in the Comet's unit (USD for USD-based Comets, ETH for the WETH Comet) as plain
//! numbers (8-decimal feed / 1e8); `usd_per_unit` converts to USD for reporting and thresholds.

use alloy::primitives::Address;

/// buyCollateral reverts if the quote exceeds available collateral; stay just under it.
const QUOTE_HAIRCUT: f64 = 0.999;

#[derive(Debug, Clone)]
pub struct AssetParams {
    pub asset: Address,
    pub scale: f64,
    pub price: f64,
    /// liquidateCollateralFactor (e.g. 0.93).
    pub liquidate_cf: f64,
    /// liquidationFactor (e.g. 0.96); discount = storefront × (1 − this).
    pub liquidation_factor: f64,
}

#[derive(Debug, Clone)]
pub struct Market {
    pub base_scale: f64,
    pub base_price: f64,
    pub usd_per_unit: f64,
    pub storefront_factor: f64,
    pub assets: Vec<AssetParams>,
}

/// A borrower: base borrow balance and collateral balances, raw units.
#[derive(Debug, Clone, Default)]
pub struct Position {
    pub borrow: f64,
    pub collateral: Vec<(Address, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthOut {
    pub health_factor: f64,
    pub debt_usd: f64,
    pub collateral_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    /// Base to pay (raw units) for `amount` of the asset at the storefront discount.
    pub base_amount: f64,
    /// Discount captured, USD.
    pub discount_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Storefront {
    pub value_usd: f64,
    pub discount_usd: f64,
}

impl Market {
    fn asset(&self, a: Address) -> Option<&AssetParams> {
        self.assets.iter().find(|x| x.asset == a)
    }

    fn value_units(&self, a: &AssetParams, amount: f64) -> f64 {
        amount / a.scale * a.price
    }

    /// Comet's liquidation condition as a health factor: < 1 ⇔ `isLiquidatable`.
    pub fn health(&self, pos: &Position) -> HealthOut {
        let debt_units = pos.borrow / self.base_scale * self.base_price;
        let (mut coll_units, mut liq_units) = (0.0, 0.0);
        for (asset, amount) in &pos.collateral {
            if let Some(a) = self.asset(*asset) {
                let v = self.value_units(a, *amount);
                coll_units += v;
                liq_units += v * a.liquidate_cf;
            }
        }
        let health_factor = if debt_units > 0.0 {
            liq_units / debt_units
        } else {
            f64::INFINITY
        };
        HealthOut {
            health_factor,
            debt_usd: debt_units * self.usd_per_unit,
            collateral_usd: coll_units * self.usd_per_unit,
        }
    }

    /// Largest collateral by value: what to buy back after `absorb`.
    pub fn best_collateral(&self, pos: &Position) -> Option<(Address, f64)> {
        pos.collateral
            .iter()
            .filter(|(_, amt)| *amt > 0.0)
            .filter_map(|(asset, amt)| {
                self.asset(*asset)
                    .map(|a| (*asset, *amt, self.value_units(a, *amt)))
            })
            .max_by(|x, y| x.2.total_cmp(&y.2))
            .map(|(asset, amt, _)| (asset, amt))
    }

    pub fn discount(&self, asset: Address) -> Option<f64> {
        self.asset(asset)
            .map(|a| self.storefront_factor * (1.0 - a.liquidation_factor))
    }

    /// Base needed to buy `amount` of `asset` at the storefront price (mirrors `quoteCollateral`).
    pub fn quote(&self, asset: Address, amount: f64) -> Option<Quote> {
        let a = self.asset(asset)?;
        if self.base_price <= 0.0 || a.price <= 0.0 || amount <= 0.0 {
            return None;
        }
        let d = self.discount(asset)?;
        let value = self.value_units(a, amount);
        let base_amount = value * (1.0 - d) / self.base_price * self.base_scale * QUOTE_HAIRCUT;
        Some(Quote {
            base_amount,
            discount_usd: value * d * self.usd_per_unit,
        })
    }

    pub fn storefront(&self, asset: Address, amount: f64) -> Storefront {
        match (self.asset(asset), self.discount(asset)) {
            (Some(a), Some(d)) => {
                let value_usd = self.value_units(a, amount) * self.usd_per_unit;
                Storefront {
                    value_usd,
                    discount_usd: value_usd * d,
                }
            }
            _ => Storefront {
                value_usd: 0.0,
                discount_usd: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARB: Address = Address::with_last_byte(1);
    const WETH: Address = Address::with_last_byte(2);

    /// USDC Comet-like market: ARB at $0.50 (lcf 0.8, lf 0.9), WETH at $2,000 (lcf 0.93, lf 0.96).
    fn usdc_comet() -> Market {
        Market {
            base_scale: 1e6,
            base_price: 1.0,
            usd_per_unit: 1.0,
            storefront_factor: 0.6,
            assets: vec![
                AssetParams {
                    asset: ARB,
                    scale: 1e18,
                    price: 0.5,
                    liquidate_cf: 0.8,
                    liquidation_factor: 0.9,
                },
                AssetParams {
                    asset: WETH,
                    scale: 1e18,
                    price: 2_000.0,
                    liquidate_cf: 0.93,
                    liquidation_factor: 0.96,
                },
            ],
        }
    }

    #[test]
    fn health_matches_comet_liquidation_condition() {
        let m = usdc_comet();
        // 1 WETH ($2,000) collateral, $1,900 borrowed: 2000*0.93/1900 = 0.979 -> liquidatable.
        let pos = Position {
            borrow: 1_900e6,
            collateral: vec![(WETH, 1e18)],
        };
        let h = m.health(&pos);
        assert!((h.health_factor - 2_000.0 * 0.93 / 1_900.0).abs() < 1e-9);
        assert!(h.health_factor < 1.0);
        assert!((h.debt_usd - 1_900.0).abs() < 1e-9);
        // Same collateral, $1,800 borrowed: healthy.
        assert!(
            m.health(&Position {
                borrow: 1_800e6,
                ..pos
            })
            .health_factor
                > 1.0
        );
    }

    #[test]
    fn no_borrow_is_infinitely_healthy() {
        let h = usdc_comet().health(&Position {
            borrow: 0.0,
            collateral: vec![(ARB, 1e18)],
        });
        assert!(h.health_factor.is_infinite());
    }

    #[test]
    fn quote_applies_storefront_discount() {
        let m = usdc_comet();
        // ARB discount = 0.6 * (1 - 0.9) = 6%. 1,000 ARB = $500 -> pay $470 (× haircut).
        let q = m.quote(ARB, 1_000e18).unwrap();
        assert!((q.base_amount - 470e6 * QUOTE_HAIRCUT).abs() < 1.0);
        assert!((q.discount_usd - 30.0).abs() < 1e-9);
        assert!(m.quote(ARB, 0.0).is_none());
    }

    #[test]
    fn best_collateral_is_highest_value() {
        let pos = Position {
            borrow: 1.0,
            collateral: vec![(ARB, 1_000e18), (WETH, 1e18)],
        };
        assert_eq!(usdc_comet().best_collateral(&pos), Some((WETH, 1e18)));
    }

    #[test]
    fn eth_priced_comet_converts_to_usd() {
        // WETH Comet: prices in ETH; base price 1.0; usd_per_unit = ETH/USD.
        let m = Market {
            base_scale: 1e18,
            base_price: 1.0,
            usd_per_unit: 2_500.0,
            storefront_factor: 0.7,
            assets: vec![AssetParams {
                asset: ARB,
                scale: 1e18,
                price: 1.05,
                liquidate_cf: 0.93,
                liquidation_factor: 0.96,
            }],
        };
        let h = m.health(&Position {
            borrow: 1e18,
            collateral: vec![(ARB, 1e18)],
        });
        assert!((h.debt_usd - 2_500.0).abs() < 1e-9);
        assert!((h.collateral_usd - 1.05 * 2_500.0).abs() < 1e-9);
        let s = m.storefront(ARB, 1e18);
        assert!((s.discount_usd - 1.05 * 2_500.0 * 0.7 * 0.04).abs() < 1e-6);
    }
}
