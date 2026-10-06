//! Risk limits: profit floor, daily gas cap, failure-streak pause, kill switch. Pure logic.

pub mod bid;
pub mod gate;

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RiskLimits {
    pub min_profit_usd: f64,
    pub max_gas_usd_per_day: f64,
    pub max_consecutive_failures: u32,
    /// Share of `max_gas_usd_per_day` arbitrage may use, so lost arb races can't spend the gas
    /// liquidations need.
    pub max_arb_gas_usd_per_day: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    KillSwitch,
    Paused { consecutive_failures: u32 },
    InFlight,
    BelowMinProfit { net_usd: f64 },
    DailyGasCap { spent_usd: f64 },
}

/// Share of today's landed profit added to the daily gas budgets.
const PROFIT_REINVEST_SHARE: f64 = 0.5;

pub struct RiskGuard {
    limits: RiskLimits,
    kill_file: PathBuf,
    day: u64,
    spent_usd: f64,
    arb_spent_usd: f64,
    /// Profit landed today: half of it is added to the gas budgets, so a profitable day (a crash's
    /// liquidation cascade) isn't stopped by the fixed cap while losses stay bounded by it.
    earned_usd: f64,
    consecutive_failures: u32,
    /// Transactions sent and not yet landed, and how many may be.
    in_flight: u32,
    max_in_flight: u32,
}

impl RiskGuard {
    pub fn new(limits: RiskLimits, kill_file: PathBuf) -> Self {
        Self {
            limits,
            kill_file,
            day: 0,
            spent_usd: 0.0,
            arb_spent_usd: 0.0,
            earned_usd: 0.0,
            consecutive_failures: 0,
            in_flight: 0,
            max_in_flight: 1,
        }
    }

    /// Allow up to `n` transactions in flight at once (default 1). The engine keeps concurrent
    /// arbitrages on disjoint pools, so they don't race each other.
    pub fn with_max_in_flight(mut self, n: u32) -> Self {
        self.max_in_flight = n.max(1);
        self
    }

    /// Decide whether a trade may be sent. `now_secs` = unix time (for daily rollover).
    pub fn check(&mut self, net_profit_usd: f64, gas_usd: f64, now_secs: u64) -> Result<(), Block> {
        self.check_inner(net_profit_usd, gas_usd, now_secs, false)
    }

    /// Like `check`, but allowed while another transaction is in flight: backruns must land in
    /// the same block as a price update and can't wait for a pending arbitrage's receipt.
    /// Kill switch, failure pause, profit floor and the daily gas cap still apply.
    pub fn check_concurrent(
        &mut self,
        net_profit_usd: f64,
        gas_usd: f64,
        now_secs: u64,
    ) -> Result<(), Block> {
        self.check_inner(net_profit_usd, gas_usd, now_secs, true)
    }

    fn check_inner(
        &mut self,
        net_profit_usd: f64,
        gas_usd: f64,
        now_secs: u64,
        concurrent: bool,
    ) -> Result<(), Block> {
        self.roll_day(now_secs);
        if self.kill_file.exists() {
            return Err(Block::KillSwitch);
        }
        if self.consecutive_failures >= self.limits.max_consecutive_failures {
            return Err(Block::Paused {
                consecutive_failures: self.consecutive_failures,
            });
        }
        if self.in_flight >= self.max_in_flight && !concurrent {
            return Err(Block::InFlight);
        }
        if net_profit_usd < self.limits.min_profit_usd {
            return Err(Block::BelowMinProfit {
                net_usd: net_profit_usd,
            });
        }
        if self.spent_usd + gas_usd > self.limits.max_gas_usd_per_day + self.reinvested() {
            return Err(Block::DailyGasCap {
                spent_usd: self.spent_usd,
            });
        }
        Ok(())
    }

    /// `check` plus the arbitrage gas sub-budget.
    pub fn check_arb(
        &mut self,
        net_profit_usd: f64,
        gas_usd: f64,
        now_secs: u64,
    ) -> Result<(), Block> {
        self.check(net_profit_usd, gas_usd, now_secs)?;
        if self.arb_spent_usd + gas_usd > self.limits.max_arb_gas_usd_per_day + self.reinvested() {
            return Err(Block::DailyGasCap {
                spent_usd: self.arb_spent_usd,
            });
        }
        Ok(())
    }

    /// Profit landed today (USD), part of which extends the gas budgets.
    pub fn record_profit(&mut self, usd: f64, now_secs: u64) {
        self.roll_day(now_secs);
        if usd > 0.0 {
            self.earned_usd += usd;
        }
    }

    /// Gas budget added by today's profit.
    fn reinvested(&self) -> f64 {
        self.earned_usd * PROFIT_REINVEST_SHARE
    }

    /// Gas spent on arbitrage (counts toward both the total and the arbitrage budget).
    pub fn record_arb_gas(&mut self, gas_usd: f64, now_secs: u64) {
        self.roll_day(now_secs);
        self.arb_spent_usd += gas_usd;
    }

    pub fn mark_sent(&mut self) {
        self.in_flight += 1;
    }

    /// Record the outcome of a sent tx.
    pub fn record(&mut self, success: bool, gas_usd: f64, now_secs: u64) {
        self.roll_day(now_secs);
        self.in_flight = self.in_flight.saturating_sub(1);
        self.spent_usd += gas_usd;
        if success {
            self.consecutive_failures = 0
        } else {
            self.consecutive_failures += 1
        }
    }

    /// A race lost on-chain (unsimulated or front-run tx reverted by the executor's profit
    /// check): costs gas, counted against the daily cap, but not a sign anything is broken, so it
    /// doesn't advance the failure streak that pauses all sending (seen live: 10 lost arb races
    /// paused liquidations too).
    pub fn record_lost_race(&mut self, gas_usd: f64, now_secs: u64) {
        self.roll_day(now_secs);
        self.in_flight = self.in_flight.saturating_sub(1);
        self.spent_usd += gas_usd;
    }

    pub fn spent_today_usd(&self) -> f64 {
        self.spent_usd
    }

    fn roll_day(&mut self, now_secs: u64) {
        let day = now_secs / 86_400;
        if day != self.day {
            self.day = day;
            self.spent_usd = 0.0;
            self.arb_spent_usd = 0.0;
            self.earned_usd = 0.0;
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
            max_arb_gas_usd_per_day: 0.5,
        };
        RiskGuard::new(limits, PathBuf::from("/nonexistent/STOP"))
    }

    #[test]
    fn backruns_may_overlap_a_pending_tx_but_keep_other_limits() {
        let mut g = guard();
        g.check(1.0, 0.01, 0).unwrap();
        g.mark_sent();
        assert!(matches!(g.check(1.0, 0.01, 0), Err(Block::InFlight)));
        assert!(g.check_concurrent(1.0, 0.01, 0).is_ok());
        assert!(matches!(
            g.check_concurrent(0.05, 0.01, 0),
            Err(Block::BelowMinProfit { .. })
        ));
        g.record(true, 1.0, 0); // daily gas cap ($1) reached
        g.mark_sent();
        assert!(matches!(
            g.check_concurrent(1.0, 0.01, 0),
            Err(Block::DailyGasCap { .. })
        ));
    }

    #[test]
    fn lost_races_cost_gas_but_never_pause() {
        let mut g = guard(); // pauses after 2 failures, $1/day
        for _ in 0..5 {
            g.mark_sent();
            g.record_lost_race(0.1, 0);
        }
        assert!(g.check(1.0, 0.01, 0).is_ok(), "5 lost races: still sending");
        g.record_lost_race(0.5, 0); // $1.0 spent
        assert!(matches!(
            g.check(1.0, 0.01, 0),
            Err(Block::DailyGasCap { .. })
        ));
    }

    #[test]
    fn arbitrage_has_its_own_slice_of_the_gas_budget() {
        let mut g = guard(); // $1/day total, $0.5 for arbs
        g.record_lost_race(0.45, 0);
        g.record_arb_gas(0.45, 0);
        assert!(matches!(
            g.check_arb(1.0, 0.1, 0),
            Err(Block::DailyGasCap { .. })
        ));
        assert!(g.check(1.0, 0.1, 0).is_ok(), "liquidations keep the rest");
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

    #[test]
    fn concurrent_sends_up_to_the_limit() {
        let mut g = guard().with_max_in_flight(2);
        g.mark_sent();
        assert!(g.check(1.0, 0.01, 0).is_ok());
        g.mark_sent();
        assert_eq!(g.check(1.0, 0.01, 0), Err(Block::InFlight));
        g.record_lost_race(0.01, 0);
        assert!(g.check(1.0, 0.01, 0).is_ok(), "a landed tx frees a slot");
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn approved_sends_never_exceed_the_daily_gas_cap(gas in proptest::collection::vec(0.0f64..0.5, 1..60)) {
            let limits = RiskLimits { min_profit_usd: 0.0, max_gas_usd_per_day: 3.0, max_consecutive_failures: u32::MAX, max_arb_gas_usd_per_day: 3.0 };
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

    #[test]
    fn profit_extends_the_gas_budget_by_half() {
        let limits = RiskLimits {
            min_profit_usd: 0.0,
            max_gas_usd_per_day: 1.0,
            max_consecutive_failures: u32::MAX,
            max_arb_gas_usd_per_day: 1.0,
        };
        let mut g = RiskGuard::new(limits, PathBuf::from("/nonexistent/kill"));
        let t = 86_400 * 100;
        g.record(true, 1.0, t);
        assert!(matches!(
            g.check(1.0, 0.5, t),
            Err(Block::DailyGasCap { .. })
        ));
        g.record_profit(2.0, t); // +1.0 of budget
        assert!(g.check(1.0, 0.5, t).is_ok());
        assert!(g.check(1.0, 1.5, t).is_err(), "still bounded");
        // Next day: budget and earnings reset.
        g.record(true, 1.0, t + 86_400);
        assert!(g.check(1.0, 0.5, t + 86_400).is_err());
    }
}
