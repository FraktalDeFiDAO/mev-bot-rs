//! Risk limits: profit floor, daily gas cap, failure-streak pause, kill switch. Pure logic.

pub mod bid;
pub mod gate;
pub mod ledger;
pub mod money;
pub mod persist;

use std::path::PathBuf;

use serde::Deserialize;

use crate::ledger::{Ledger, Reservation};

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
    InvalidInput,
    Persistence,
    Unresolved,
    TipCap {
        window: &'static str,
        exposure_microusd: u64,
        limit_microusd: u64,
    },
    Paused {
        consecutive_failures: u32,
    },
    InFlight,
    BelowMinProfit {
        net_usd: f64,
    },
    DailyGasCap {
        spent_usd: f64,
    },
}

impl std::fmt::Display for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Block {}

/// Compatibility boundary for the legacy strategy estimates. The ledger compares integer
/// micro-dollars; invalid estimates block admission and costs round upward.
pub fn cost_microusd(usd: f64) -> Result<u64, Block> {
    if !usd.is_finite() || usd < 0.0 || usd > (u64::MAX / 1_000_000) as f64 {
        return Err(Block::InvalidInput);
    }
    Ok((usd * 1_000_000.0).ceil() as u64)
}

/// Authorized ceilings round down; liabilities round up.
pub fn limit_microusd(usd: f64) -> Result<u64, Block> {
    cost_microusd(usd)?;
    Ok((usd * 1_000_000.0).floor() as u64)
}

pub struct RiskGuard {
    limits: RiskLimits,
    kill_file: PathBuf,
    max_in_flight: u32,
    ledger: Ledger,
    tip_caps: ledger::TipCaps,
    #[cfg(test)]
    last_check: Option<(f64, bool, u64)>,
    #[cfg(test)]
    legacy: std::collections::VecDeque<Reservation>,
}

impl RiskGuard {
    pub fn kill_file(&self) -> PathBuf {
        self.kill_file.clone()
    }
    pub fn new(limits: RiskLimits, kill_file: PathBuf) -> Self {
        Self {
            limits,
            kill_file,
            max_in_flight: 1,
            ledger: Ledger::default(),
            tip_caps: Default::default(),
            #[cfg(test)]
            last_check: None,
            #[cfg(test)]
            legacy: Default::default(),
        }
    }

    pub fn with_persistence(mut self, path: PathBuf, now_secs: u64) -> Self {
        self.ledger = Ledger::persistent(path, now_secs);
        self
    }

    pub fn with_existing_persistence(
        mut self,
        path: PathBuf,
        now_secs: u64,
        chain_id: u64,
        owner: mev_types::Address,
    ) -> Self {
        self.ledger = Ledger::persistent_existing(path, now_secs);
        let _ = self.ledger.require_identity(chain_id, owner);
        self
    }

    pub fn with_tip_caps(mut self, caps: ledger::TipCaps) -> Self {
        self.tip_caps = caps;
        self
    }

    pub fn with_max_in_flight(mut self, n: u32) -> Self {
        self.max_in_flight = n.max(1);
        self
    }

    pub fn check(&mut self, net_profit_usd: f64, gas_usd: f64, now: u64) -> Result<(), Block> {
        self.check_inner(net_profit_usd, gas_usd, now, false)
    }

    /// Backruns share the global exposure bound; urgency cannot create additional capital.
    pub fn check_concurrent(
        &mut self,
        net_profit_usd: f64,
        gas_usd: f64,
        now: u64,
    ) -> Result<(), Block> {
        self.check(net_profit_usd, gas_usd, now)
    }

    pub fn check_arb(&mut self, net_profit_usd: f64, gas_usd: f64, now: u64) -> Result<(), Block> {
        self.check_inner(net_profit_usd, gas_usd, now, true)
    }

    fn check_inner(&mut self, profit: f64, gas: f64, now: u64, arb: bool) -> Result<(), Block> {
        #[cfg(test)]
        {
            self.last_check = None;
        }
        let gas_int = cost_microusd(gas)?;
        let min_profit = cost_microusd(self.limits.min_profit_usd)?;
        let total = limit_microusd(self.limits.max_gas_usd_per_day)?;
        let arb_cap = limit_microusd(self.limits.max_arb_gas_usd_per_day)?;
        if !profit.is_finite() || profit < 0.0 {
            return Err(Block::InvalidInput);
        }
        if self.kill_file.exists() {
            return Err(Block::KillSwitch);
        }
        self.ledger.check(
            now,
            gas_int,
            arb,
            total,
            arb_cap,
            self.max_in_flight,
            self.limits.max_consecutive_failures,
        )?;
        if ((profit * 1_000_000.0).floor() as u64) < min_profit {
            return Err(Block::BelowMinProfit { net_usd: profit });
        }
        #[cfg(test)]
        {
            self.last_check = Some((gas, arb, now));
        }
        Ok(())
    }

    pub fn reserve(
        &mut self,
        profit: f64,
        bound: f64,
        arb: bool,
        now: u64,
    ) -> Result<Reservation, Block> {
        self.check_inner(profit, bound, now, arb)?;
        self.ledger.reserve(now, cost_microusd(bound)?, arb)
    }

    pub fn reserve_integer(
        &mut self,
        profit: u64,
        bound: u64,
        tip: u64,
        arb: bool,
        now: u64,
    ) -> Result<Reservation, Block> {
        if self.kill_file.exists() {
            return Err(Block::KillSwitch);
        }
        self.ledger.check(
            now,
            bound,
            arb,
            limit_microusd(self.limits.max_gas_usd_per_day)?,
            limit_microusd(self.limits.max_arb_gas_usd_per_day)?,
            self.max_in_flight,
            self.limits.max_consecutive_failures,
        )?;
        if profit < cost_microusd(self.limits.min_profit_usd)? {
            return Err(Block::BelowMinProfit {
                net_usd: profit as f64 / 1_000_000.0,
            });
        }
        self.ledger.check_tip(now, tip, self.tip_caps)?;
        self.ledger.reserve_with_tip(now, bound, tip, arb)
    }

    pub fn unresolved(&self) -> Vec<(Reservation, ledger::Pending)> {
        self.ledger.unresolved()
    }

    pub fn nonce_window(&self) -> Result<(u64, u64), Block> {
        self.ledger.nonce_window()
    }

    pub fn arb_spent_today_usd(&self) -> f64 {
        self.ledger.arb_spent() as f64 / 1_000_000.0
    }

    pub fn settled_spend(&self) -> (u64, u64) {
        (self.ledger.spent(), self.ledger.arb_spent())
    }

    /// Legacy bookkeeping only. New execution must use a Reservation passed through signing.
    #[cfg(test)]
    pub fn mark_sent(&mut self) {
        let (gas, arb, now) = self.last_check.take().unwrap_or((0.0, false, 0));
        if let Ok(r) = self
            .ledger
            .reserve(now, cost_microusd(gas).unwrap_or(u64::MAX), arb)
        {
            self.legacy.push_back(r);
        }
    }

    /// Estimated profit never changes the approved daily limits.
    #[cfg(test)]
    pub fn record_profit(&mut self, _usd: f64, _now: u64) {}

    #[cfg(test)]
    pub fn record_arb_gas(&mut self, usd: f64, now: u64) {
        self.ledger
            .add_arb_cost(now, cost_microusd(usd).unwrap_or(u64::MAX));
    }

    #[cfg(test)]
    pub fn record(&mut self, success: bool, usd: f64, now: u64) {
        self.record_inner(success, usd, now, false)
    }

    #[cfg(test)]
    pub fn record_lost_race(&mut self, usd: f64, now: u64) {
        self.record_inner(false, usd, now, true)
    }

    #[cfg(test)]
    fn record_inner(&mut self, success: bool, usd: f64, now: u64, lost_race: bool) {
        let cost = cost_microusd(usd).unwrap_or(u64::MAX);
        if let Some(r) = self.legacy.pop_front() {
            let _ = r.settle(now, success, cost, lost_race);
        } else {
            self.ledger
                .record_legacy(now, success, cost, false, lost_race);
        }
    }

    pub fn spent_today_usd(&self) -> f64 {
        self.ledger.spent() as f64 / 1_000_000.0
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
    fn caps_and_failure_streak_survive_a_restart() {
        let path = std::env::temp_dir().join(format!("risk-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut g = guard().with_persistence(path.clone(), 1_000);
        g.mark_sent();
        g.record(false, 0.6, 1_000);
        g.mark_sent();
        g.record(false, 0.5, 1_000); // $1.1 spent, 2 failures: paused and over the cap
        drop(g);
        let mut g2 = guard().with_persistence(path.clone(), 2_000);
        assert!((g2.spent_today_usd() - 1.1).abs() < 1e-9);
        assert!(matches!(
            g2.check(1.0, 0.01, 2_000),
            Err(Block::Paused { .. })
        ));
        // The next UTC day starts clean.
        drop(g2);
        let mut g3 = guard().with_persistence(path.clone(), 90_000);
        assert!(g3.check(1.0, 0.01, 90_000).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_state_file_blocks_admission() {
        let path = std::env::temp_dir().join(format!("risk-bad-{}.json", std::process::id()));
        std::fs::write(&path, b"{not json").unwrap();
        let mut g = guard().with_persistence(path.clone(), 0);
        assert_eq!(g.check(1.0, 0.01, 0), Err(Block::Persistence));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn backruns_share_a_bounded_pending_allowance() {
        let mut g = guard().with_max_in_flight(2);
        g.check(1.0, 0.01, 0).unwrap();
        g.mark_sent();
        assert!(g.check(1.0, 0.01, 0).is_ok());
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
    fn estimates_do_not_expand_approved_budgets() {
        let mut g = RiskGuard::new(
            RiskLimits {
                min_profit_usd: 0.1,
                max_gas_usd_per_day: 1.0,
                max_arb_gas_usd_per_day: 1.0,
                max_consecutive_failures: 10,
            },
            PathBuf::from("/nonexistent/STOP"),
        );
        g.record(false, 0.8, 0);
        g.record_profit(2.0, 0);
        assert!(matches!(
            g.check(1.0, 0.5, 0),
            Err(Block::DailyGasCap { .. })
        ));
        assert!(g.check(1.0, 0.1, 90_000).is_ok());
    }
}
