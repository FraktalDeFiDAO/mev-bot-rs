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

/// Integer admission inputs. Prices are micro-USD per native coin; gas and fees stay raw.
#[derive(Debug, Clone, Copy)]
pub struct IntegerBidInput {
    pub gross_profit_microusd: u64,
    pub gas_units: u64,
    pub base_fee_wei: u128,
    pub native_price_microusd: u64,
    pub competition_tip_wei: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct IntegerBidPolicy {
    pub max_fraction_bps: u32,
    pub max_tip_microusd: u64,
    pub min_net_microusd: u64,
    pub min_tip_wei: u128,
    pub default_tip_wei: u128,
    pub outbid_margin_bps: u64,
}

impl BidPolicy {
    /// Legacy config boundary. Compile once at startup; execution uses IntegerBidPolicy.
    pub fn integer(&self) -> Result<IntegerBidPolicy, crate::Block> {
        if !self.max_fraction_of_net.is_finite()
            || !(0.0..=1.0).contains(&self.max_fraction_of_net)
            || !self.outbid_margin.is_finite()
            || !(0.0..=1e8).contains(&self.outbid_margin)
        {
            return Err(crate::Block::InvalidInput);
        }
        Ok(IntegerBidPolicy {
            max_fraction_bps: (self.max_fraction_of_net * 10_000.0).floor() as u32,
            max_tip_microusd: crate::limit_microusd(self.max_tip_usd_per_tx)?,
            min_net_microusd: crate::cost_microusd(self.min_net_usd)?,
            min_tip_wei: self.min_tip_wei,
            default_tip_wei: self.default_tip_wei,
            outbid_margin_bps: (self.outbid_margin * 10_000.0).ceil() as u64,
        })
    }

    /// Compatibility for research and old callers. Not used by live engine execution.
    pub fn decide(&self, x: &BidInput) -> Decision {
        let (Ok(policy), Ok(price), Ok(gross)) = (
            self.integer(),
            crate::cost_microusd(x.usd_per_wei * 1e18),
            crate::limit_microusd(x.gross_profit_usd),
        ) else {
            return Decision::Skip(Skip::InvalidInput);
        };
        policy.decide(&IntegerBidInput {
            gross_profit_microusd: gross,
            gas_units: x.gas_units,
            base_fee_wei: x.base_fee_wei,
            native_price_microusd: price,
            competition_tip_wei: x.competition_tip_wei,
        })
    }
}

impl IntegerBidPolicy {
    pub fn decide(&self, x: &IntegerBidInput) -> Decision {
        if x.gas_units == 0 || x.native_price_microusd == 0 || self.max_fraction_bps > 10_000 {
            return Decision::Skip(Skip::InvalidInput);
        }
        let Ok(base_cost) =
            crate::ledger::native_cost(x.gas_units, x.base_fee_wei, x.native_price_microusd)
        else {
            return Decision::Skip(Skip::InvalidInput);
        };
        let net = i128::from(x.gross_profit_microusd) - i128::from(base_cost);
        if net <= i128::from(self.min_net_microusd) {
            return Decision::Skip(Skip::NotProfitable {
                net_usd: net as f64 / 1_000_000.0,
            });
        }
        let budget = (net * i128::from(self.max_fraction_bps) / 10_000)
            .min(i128::from(self.max_tip_microusd))
            .min(net - i128::from(self.min_net_microusd)) as u128;
        let denominator = u128::from(x.gas_units) * u128::from(x.native_price_microusd);
        let max_wei = budget * 1_000_000_000_000_000_000u128 / denominator;
        let tip = match x.competition_tip_wei {
            Some(c) => {
                let Some(scaled) = c
                    .checked_mul(10_000 + u128::from(self.outbid_margin_bps))
                    .and_then(|n| n.div_ceil(10_000).checked_add(1))
                else {
                    return Decision::Skip(Skip::InvalidInput);
                };
                let needed = scaled.max(self.min_tip_wei);
                if needed > max_wei {
                    return Decision::Skip(Skip::OutbidTooExpensive {
                        needed_wei: needed,
                        max_wei,
                    });
                }
                needed
            }
            None => self.default_tip_wei.max(self.min_tip_wei).min(max_wei),
        };
        if tip < self.min_tip_wei {
            return Decision::Skip(Skip::OutbidTooExpensive {
                needed_wei: self.min_tip_wei,
                max_wei,
            });
        }
        let Ok(tip_cost) = crate::ledger::native_cost(x.gas_units, tip, x.native_price_microusd)
        else {
            return Decision::Skip(Skip::InvalidInput);
        };
        let Some(max_fee) = x
            .base_fee_wei
            .checked_mul(2)
            .and_then(|base| base.checked_add(tip))
        else {
            return Decision::Skip(Skip::InvalidInput);
        };
        Decision::Bid(Bid {
            max_priority_fee_per_gas: tip,
            max_fee_per_gas: max_fee,
            base_cost_usd: base_cost as f64 / 1_000_000.0,
            tip_cost_usd: tip_cost as f64 / 1_000_000.0,
            net_after_tip_usd: (net - i128::from(tip_cost)) as f64 / 1_000_000.0,
            fraction_of_net: tip_cost as f64 / net as f64,
        })
    }
}

/// The higher of two optional competition estimates (None if neither is known): bid against the
/// strongest evidence of what ordering costs.
pub fn strongest(a: Option<u128>, b: Option<u128>) -> Option<u128> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

/// Recently observed winning priority fees (wei/gas), for the competition estimate. Entries
/// older than `horizon_secs` or beyond `capacity` are dropped.
/// The competition a bid on one route has to beat. A route with at least `min_obs` recorded winners
/// is priced from its own winners only: the market is bimodal (about 40 % of races are won by
/// near-zero tips, a fifth by 0.3 to 3 gwei) and one network-wide high quantile prices every route as
/// the worst. A route without that history starts from the market's central tendency (`market`, a
/// median of winners and of bot-like feed tips), not its top.
pub fn route_competition(
    route_obs: usize,
    min_obs: usize,
    route: Option<u128>,
    market: Option<u128>,
) -> Option<u128> {
    if route_obs >= min_obs.max(1) {
        route.or(market)
    } else {
        market
    }
}

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

#[cfg(test)]
mod tests {

    #[test]
    fn a_route_with_history_is_priced_by_its_own_winners() {
        let m = Some(20_000_000);
        // Enough winners on the route: its own estimate, even when the market is much higher or lower.
        assert_eq!(
            route_competition(5, 3, Some(1_000), Some(1_000_000_000)),
            Some(1_000)
        );
        assert_eq!(
            route_competition(3, 3, Some(2_000_000_000), m),
            Some(2_000_000_000)
        );
        // Too few winners: the market median.
        assert_eq!(route_competition(2, 3, Some(1_000_000_000), m), m);
        assert_eq!(route_competition(0, 3, None, m), m);
        // Nothing known anywhere: no competition data (the default tip applies).
        assert_eq!(route_competition(0, 3, None, None), None);
        // History present but its estimate unavailable (all expired): fall back to the market.
        assert_eq!(route_competition(4, 3, None, m), m);
    }

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
    fn strongest_takes_the_higher_estimate() {
        assert_eq!(strongest(Some(3), Some(9)), Some(9));
        assert_eq!(strongest(None, Some(9)), Some(9));
        assert_eq!(strongest(Some(3), None), Some(3));
        assert_eq!(strongest(None, None), None);
    }
}
