//! Risk limits: profit floor, daily gas cap, failure-streak pause, kill switch. Pure logic.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RiskLimits {
    pub min_profit_usd: f64,
    pub max_gas_usd_per_day: f64,
    pub max_consecutive_failures: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    KillSwitch,
    Paused { consecutive_failures: u32 },
    InFlight,
    BelowMinProfit { net_usd: f64 },
    DailyGasCap { spent_usd: f64 },
}

pub struct RiskGuard {
    limits: RiskLimits,
    kill_file: PathBuf,
    day: u64,
    spent_usd: f64,
    consecutive_failures: u32,
    in_flight: bool,
}

impl RiskGuard {
    pub fn new(limits: RiskLimits, kill_file: PathBuf) -> Self {
        Self {
            limits,
            kill_file,
            day: 0,
            spent_usd: 0.0,
            consecutive_failures: 0,
            in_flight: false,
        }
    }

    /// Decide whether a trade may be sent. `now_secs` = unix time (for daily rollover).
    pub fn check(&mut self, net_profit_usd: f64, gas_usd: f64, now_secs: u64) -> Result<(), Block> {
        self.roll_day(now_secs);
        if self.kill_file.exists() {
            return Err(Block::KillSwitch);
        }
        if self.consecutive_failures >= self.limits.max_consecutive_failures {
            return Err(Block::Paused {
                consecutive_failures: self.consecutive_failures,
            });
        }
        if self.in_flight {
            return Err(Block::InFlight);
        }
        if net_profit_usd < self.limits.min_profit_usd {
            return Err(Block::BelowMinProfit {
                net_usd: net_profit_usd,
            });
        }
        if self.spent_usd + gas_usd > self.limits.max_gas_usd_per_day {
            return Err(Block::DailyGasCap {
                spent_usd: self.spent_usd,
            });
        }
        Ok(())
    }

    pub fn mark_sent(&mut self) {
        self.in_flight = true;
    }

    /// Record the outcome of a sent tx.
    pub fn record(&mut self, success: bool, gas_usd: f64, now_secs: u64) {
        self.roll_day(now_secs);
        self.in_flight = false;
        self.spent_usd += gas_usd;
        if success {
            self.consecutive_failures = 0
        } else {
            self.consecutive_failures += 1
        }
    }

    pub fn spent_today_usd(&self) -> f64 {
        self.spent_usd
    }

    fn roll_day(&mut self, now_secs: u64) {
        let day = now_secs / 86_400;
        if day != self.day {
            self.day = day;
            self.spent_usd = 0.0;
        }
    }
}

/// Largest flash loan to request, in raw token units: the USD cap, but never more than
/// `vault_share` of what the lender holds (so the loan can't fail for lack of liquidity).
pub fn loan_cap(
    max_loan_usd: f64,
    usd_per_raw: f64,
    lender_balance_raw: f64,
    vault_share: f64,
) -> f64 {
    if usd_per_raw <= 0.0 {
        return 0.0;
    }
    (max_loan_usd / usd_per_raw)
        .min(lender_balance_raw * vault_share)
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loan_cap_takes_the_tighter_limit() {
        // $50k cap at $2 per unit = 25k units; lender holds 10k -> 50% = 5k units.
        assert_eq!(loan_cap(50_000.0, 2.0, 10_000.0, 0.5), 5_000.0);
        assert_eq!(loan_cap(1_000.0, 2.0, 10_000.0, 0.5), 500.0);
        assert_eq!(loan_cap(1_000.0, 0.0, 10_000.0, 0.5), 0.0);
    }

    fn guard() -> RiskGuard {
        let limits = RiskLimits {
            min_profit_usd: 0.1,
            max_gas_usd_per_day: 1.0,
            max_consecutive_failures: 2,
        };
        RiskGuard::new(limits, PathBuf::from("/nonexistent/STOP"))
    }

    #[test]
    fn blocks_below_min_profit() {
        assert!(matches!(
            guard().check(0.05, 0.01, 0),
            Err(Block::BelowMinProfit { .. })
        ));
    }

    #[test]
    fn pauses_after_failure_streak_and_resets_on_success() {
        let mut g = guard();
        g.record(false, 0.01, 0);
        g.record(false, 0.01, 0);
        assert!(matches!(g.check(1.0, 0.01, 0), Err(Block::Paused { .. })));
        g.record(true, 0.01, 0);
        assert!(g.check(1.0, 0.01, 0).is_ok());
    }

    #[test]
    fn daily_gas_cap_rolls_over() {
        let mut g = guard();
        g.record(true, 0.95, 10);
        assert!(matches!(
            g.check(1.0, 0.1, 10),
            Err(Block::DailyGasCap { .. })
        ));
        assert!(g.check(1.0, 0.1, 86_400 + 10).is_ok());
    }

    #[test]
    fn one_tx_in_flight() {
        let mut g = guard();
        g.mark_sent();
        assert_eq!(g.check(1.0, 0.01, 0), Err(Block::InFlight));
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn approved_sends_never_exceed_the_daily_gas_cap(gas in proptest::collection::vec(0.0f64..0.5, 1..60)) {
            let limits = RiskLimits { min_profit_usd: 0.0, max_gas_usd_per_day: 3.0, max_consecutive_failures: u32::MAX };
            let mut g = RiskGuard::new(limits, PathBuf::from("/nonexistent/STOP"));
            for cost in gas {
                if g.check(1.0, cost, 0).is_ok() {
                    g.mark_sent();
                    g.record(true, cost, 0);
                }
                prop_assert!(g.spent_today_usd() <= 3.0 + 1e-9);
            }
        }
    }
}
