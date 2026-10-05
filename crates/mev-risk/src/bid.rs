//! Priority-fee bidding for Arbitrum One's Priority Gas Auction (PGA). Pure: no I/O, no clocks
//! (callers pass `now_secs`), so every rule is unit-testable.
//!
//! PGA (docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/pga): the Sequencer orders each
//! 125 ms round (two per 250 ms block) by `min(maxPriorityFeePerGas, maxFeePerGas - baseFee)`,
//! ties by arrival; the mempool is private. So a transaction's position is bought with its own
//! priority fee, and that fee is paid whether or not the trade still profits when it executes.
//!
//! Policy: bid only from expected *net* profit (gross minus base gas), at most a configured
//! fraction of it and a hard per-tx cap, and never so much that net profit after the tip falls
//! below the floor. Competition is estimated from tips recently observed winning ordering
//! (`TipBook`). Decide at submission time: inputs are the opportunity's numbers and the base fee
//! at that moment, never a value cached from detection.

use std::collections::VecDeque;

/// Static bidding parameters (from config).
#[derive(Debug, Clone, PartialEq)]
pub struct BidPolicy {
    /// Largest share of expected net profit spent on the priority fee (0..=1).
    pub max_fraction_of_net: f64,
    /// Hard cap on one transaction's priority-fee spend, USD.
    pub max_tip_usd_per_tx: f64,
    /// Net profit (after base gas and tip) a bid must leave, USD.
    pub min_net_usd: f64,
    /// Floor and default priority fee, wei per gas (default used when no competition is known).
    pub min_tip_wei: u128,
    pub default_tip_wei: u128,
    /// Bid this much above the competition estimate (0.1 = +10 %).
    pub outbid_margin: f64,
}

/// One opportunity at submission time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BidInput {
    /// Expected profit before any gas, USD.
    pub gross_profit_usd: f64,
    /// Gas the transaction is expected to use (not its limit).
    pub gas_units: u64,
    /// Current L2 base fee, wei per gas.
    pub base_fee_wei: u128,
    /// USD per wei of the native token.
    pub usd_per_wei: f64,
    /// Priority fee (wei/gas) recently needed to win ordering, if known.
    pub competition_tip_wei: Option<u128>,
}

/// A go decision: the fee fields to put on the transaction and what they cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bid {
    pub max_priority_fee_per_gas: u128,
    /// `2 × base fee + tip`: headroom for a base-fee rise; the effective tip stays the bid.
    pub max_fee_per_gas: u128,
    pub base_cost_usd: f64,
    pub tip_cost_usd: f64,
    pub net_after_tip_usd: f64,
    /// Tip spend as a fraction of net profit before the tip.
    pub fraction_of_net: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Skip {
    /// Bad inputs (no gas, no price).
    InvalidInput,
    /// Net profit before any tip doesn't clear the floor.
    NotProfitable { net_usd: f64 },
    /// Winning would need a tip above what the policy allows.
    OutbidTooExpensive { needed_wei: u128, max_wei: u128 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Bid(Bid),
    Skip(Skip),
}

impl BidPolicy {
    /// Decide whether and how much to bid for `x`.
    pub fn decide(&self, x: &BidInput) -> Decision {
        if x.gas_units == 0 || !(x.usd_per_wei.is_finite() && x.usd_per_wei > 0.0) {
            return Decision::Skip(Skip::InvalidInput);
        }
        let gas = x.gas_units as f64;
        let base_cost_usd = gas * x.base_fee_wei as f64 * x.usd_per_wei;
        let net_usd = x.gross_profit_usd - base_cost_usd;
        if net_usd.is_nan() || net_usd <= self.min_net_usd {
            return Decision::Skip(Skip::NotProfitable { net_usd });
        }
        // Budget: a share of net, the hard cap, and never below the profit floor after the tip.
        let budget_usd = (net_usd * self.max_fraction_of_net.clamp(0.0, 1.0))
            .min(self.max_tip_usd_per_tx)
            .min(net_usd - self.min_net_usd)
            .max(0.0);
        let max_wei = (budget_usd / (gas * x.usd_per_wei)).floor() as u128;
        let tip = match x.competition_tip_wei {
            Some(c) => {
                let needed = ((c as f64) * (1.0 + self.outbid_margin.max(0.0))).ceil() as u128 + 1;
                let needed = needed.max(self.min_tip_wei);
                if needed > max_wei {
                    return Decision::Skip(Skip::OutbidTooExpensive {
                        needed_wei: needed,
                        max_wei,
                    });
                }
                needed
            }
            // No competition data: the default, within budget.
            None => self.default_tip_wei.max(self.min_tip_wei).min(max_wei),
        };
        if tip < self.min_tip_wei {
            return Decision::Skip(Skip::OutbidTooExpensive {
                needed_wei: self.min_tip_wei,
                max_wei,
            });
        }
        let tip_cost_usd = gas * tip as f64 * x.usd_per_wei;
        Decision::Bid(Bid {
            max_priority_fee_per_gas: tip,
            max_fee_per_gas: x.base_fee_wei.saturating_mul(2).saturating_add(tip),
            base_cost_usd,
            tip_cost_usd,
            net_after_tip_usd: net_usd - tip_cost_usd,
            fraction_of_net: tip_cost_usd / net_usd,
        })
    }
}

/// Recently observed winning priority fees (wei/gas), for the competition estimate. Entries
/// older than `horizon_secs` or beyond `capacity` are dropped.
#[derive(Debug, Clone)]
pub struct TipBook {
    seen: VecDeque<(u64, u128)>,
    capacity: usize,
    horizon_secs: u64,
}

impl TipBook {
    pub fn new(capacity: usize, horizon_secs: u64) -> Self {
        Self {
            seen: VecDeque::new(),
            capacity: capacity.max(1),
            horizon_secs,
        }
    }

    /// Record a priority fee that won ordering ahead of (or instead of) us.
    pub fn record(&mut self, tip_wei: u128, now_secs: u64) {
        self.prune(now_secs);
        self.seen.push_back((now_secs, tip_wei));
        while self.seen.len() > self.capacity {
            self.seen.pop_front();
        }
    }

    /// The `q`-quantile (0..=1) of recent winning tips, or `None` without recent data.
    pub fn estimate(&mut self, q: f64, now_secs: u64) -> Option<u128> {
        self.prune(now_secs);
        if self.seen.is_empty() {
            return None;
        }
        let mut v: Vec<u128> = self.seen.iter().map(|(_, t)| *t).collect();
        v.sort_unstable();
        let i = ((v.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
        Some(v[i])
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn prune(&mut self, now_secs: u64) {
        while self
            .seen
            .front()
            .is_some_and(|(t, _)| now_secs.saturating_sub(*t) > self.horizon_secs)
        {
            self.seen.pop_front();
        }
    }
}

/// Priority-fee spend caps per rolling minute and per UTC day, USD.
#[derive(Debug, Clone)]
pub struct TipBudget {
    per_minute_usd: f64,
    per_day_usd: f64,
    minute: VecDeque<(u64, f64)>,
    day: u64,
    day_spent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TipCap {
    PerMinute { spent_usd: f64 },
    PerDay { spent_usd: f64 },
}

impl TipBudget {
    pub fn new(per_minute_usd: f64, per_day_usd: f64) -> Self {
        Self {
            per_minute_usd,
            per_day_usd,
            minute: VecDeque::new(),
            day: 0,
            day_spent: 0.0,
        }
    }

    /// May a bid costing `tip_usd` be placed now?
    pub fn allow(&mut self, tip_usd: f64, now_secs: u64) -> Result<(), TipCap> {
        self.roll(now_secs);
        let minute: f64 = self.minute.iter().map(|(_, u)| u).sum();
        if minute + tip_usd > self.per_minute_usd {
            return Err(TipCap::PerMinute { spent_usd: minute });
        }
        if self.day_spent + tip_usd > self.per_day_usd {
            return Err(TipCap::PerDay {
                spent_usd: self.day_spent,
            });
        }
        Ok(())
    }

    /// Count `tip_usd` as spent (at placement: a placed bid's fee is paid if it is included).
    pub fn record(&mut self, tip_usd: f64, now_secs: u64) {
        self.roll(now_secs);
        self.minute.push_back((now_secs, tip_usd));
        self.day_spent += tip_usd;
    }

    fn roll(&mut self, now_secs: u64) {
        while self
            .minute
            .front()
            .is_some_and(|(t, _)| now_secs.saturating_sub(*t) >= 60)
        {
            self.minute.pop_front();
        }
        let day = now_secs / 86_400;
        if day != self.day {
            self.day = day;
            self.day_spent = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GWEI: u128 = 1_000_000_000;
    /// ETH at $3,000: USD per wei.
    const USD_PER_WEI: f64 = 3000.0 / 1e18;

    fn policy() -> BidPolicy {
        BidPolicy {
            max_fraction_of_net: 0.5,
            max_tip_usd_per_tx: 1.0,
            min_net_usd: 0.02,
            min_tip_wei: 0,
            default_tip_wei: GWEI / 100, // 0.01 gwei
            outbid_margin: 0.1,
        }
    }

    fn input(gross: f64, competition: Option<u128>) -> BidInput {
        BidInput {
            gross_profit_usd: gross,
            gas_units: 300_000,
            base_fee_wei: GWEI / 100, // 0.01 gwei → base cost $0.009
            usd_per_wei: USD_PER_WEI,
            competition_tip_wei: competition,
        }
    }

    #[test]
    fn zero_profit_never_bids() {
        assert!(matches!(
            policy().decide(&input(0.0, None)),
            Decision::Skip(Skip::NotProfitable { .. })
        ));
        // Profit that only covers base gas: still no.
        assert!(matches!(
            policy().decide(&input(0.01, None)),
            Decision::Skip(Skip::NotProfitable { .. })
        ));
    }

    #[test]
    fn bid_never_makes_expected_profit_negative_or_below_floor() {
        let p = policy();
        for gross in [0.03, 0.05, 0.1, 1.0, 10.0, 1000.0] {
            for comp in [None, Some(GWEI / 1000), Some(GWEI / 10), Some(5 * GWEI)] {
                if let Decision::Bid(b) = p.decide(&input(gross, comp)) {
                    assert!(
                        b.net_after_tip_usd >= p.min_net_usd - 1e-12,
                        "{gross} {comp:?}"
                    );
                    assert!(b.fraction_of_net <= p.max_fraction_of_net + 1e-12);
                    assert!(b.tip_cost_usd <= p.max_tip_usd_per_tx + 1e-12);
                }
            }
        }
    }

    #[test]
    fn hard_per_tx_cap_binds_on_large_profits() {
        // $1,000 net: 50 % would be $500, the cap is $1.
        let Decision::Bid(b) = policy().decide(&input(1000.0, None)) else {
            panic!()
        };
        assert!(b.tip_cost_usd <= 1.0);
        // Competition above what $1 buys at 300k gas → skip, not overpay.
        let one_dollar_wei = (1.0 / (300_000.0 * USD_PER_WEI)) as u128;
        assert!(matches!(
            policy().decide(&input(1000.0, Some(one_dollar_wei * 2))),
            Decision::Skip(Skip::OutbidTooExpensive { .. })
        ));
    }

    #[test]
    fn outbids_competition_by_the_margin_and_sets_consistent_fee_fields() {
        let comp = GWEI / 100; // 0.01 gwei
        let Decision::Bid(b) = policy().decide(&input(5.0, Some(comp))) else {
            panic!()
        };
        assert_eq!(
            b.max_priority_fee_per_gas,
            (comp as f64 * 1.1).ceil() as u128 + 1
        );
        // maxFee covers base + tip with headroom, so the effective tip equals the bid.
        let base = GWEI / 100;
        assert_eq!(b.max_fee_per_gas, 2 * base + b.max_priority_fee_per_gas);
        assert!(b.max_fee_per_gas - base >= b.max_priority_fee_per_gas);
    }

    #[test]
    fn invalid_inputs_skip() {
        let mut x = input(5.0, None);
        x.gas_units = 0;
        assert_eq!(policy().decide(&x), Decision::Skip(Skip::InvalidInput));
        let mut x = input(5.0, None);
        x.usd_per_wei = 0.0;
        assert_eq!(policy().decide(&x), Decision::Skip(Skip::InvalidInput));
    }

    #[test]
    fn tipbook_quantiles_and_expiry() {
        let mut t = TipBook::new(100, 600);
        assert_eq!(t.estimate(0.9, 0), None);
        for (i, tip) in [1u128, 2, 3, 4, 100].into_iter().enumerate() {
            t.record(tip, i as u64);
        }
        assert_eq!(t.estimate(0.5, 10), Some(3));
        assert_eq!(t.estimate(1.0, 10), Some(100));
        // All older than the horizon: no estimate (fall back to the default tip).
        assert_eq!(t.estimate(0.5, 10_000), None);
    }

    #[test]
    fn tip_budget_caps_per_minute_and_per_day() {
        let mut b = TipBudget::new(1.0, 2.5);
        let t0 = 86_400 * 10;
        assert!(b.allow(0.6, t0).is_ok());
        b.record(0.6, t0);
        assert!(matches!(
            b.allow(0.6, t0 + 30),
            Err(TipCap::PerMinute { .. })
        ));
        // A minute later the window has rolled.
        assert!(b.allow(0.6, t0 + 61).is_ok());
        b.record(0.6, t0 + 61);
        b.record(0.9, t0 + 130);
        assert!(matches!(b.allow(0.5, t0 + 200), Err(TipCap::PerDay { .. })));
        // Next UTC day resets.
        assert!(b.allow(0.5, t0 + 86_400).is_ok());
    }
}
