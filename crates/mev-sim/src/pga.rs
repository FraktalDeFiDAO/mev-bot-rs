//! Arbitrum One Priority Gas Auction model for backtests (DECISIONS #59).
//!
//! In a PGA round the Sequencer orders transactions by effective priority fee, ties by arrival
//! (docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/pga). For one contested opportunity
//! the first transaction to take it wins; the others execute after it and revert (paying gas and
//! their tip). The model:
//! - `win_probability`: chance a bid beats the competition, from the empirical distribution of
//!   past winning tips (non-decreasing in the bid; a tie counts half, since arrival order breaks it).
//! - `replay`: run a `Strategy` over recorded or synthetic rounds. The strategy sees only a
//!   `PastView` of tips revealed by earlier rounds — never the current round's competing bids.

/// Probability a bid of `tip_wei` wins against a competitor drawn from `past_winning_tips`.
/// No history: 0.5 (no information either way).
pub fn win_probability(tip_wei: u128, past_winning_tips: &[u128]) -> f64 {
    if past_winning_tips.is_empty() {
        return 0.5;
    }
    let below = past_winning_tips.iter().filter(|t| **t < tip_wei).count() as f64;
    let ties = past_winning_tips.iter().filter(|t| **t == tip_wei).count() as f64;
    (below + 0.5 * ties) / past_winning_tips.len() as f64
}

/// One contested opportunity in one round.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    /// Our expected profit before any gas, USD, and the gas our transaction uses.
    pub gross_usd: f64,
    pub gas_units: u64,
    pub base_fee_wei: u128,
    pub usd_per_wei: f64,
    /// Competing bids for the same opportunity (unknown to the strategy until revealed).
    pub competitor_tips: Vec<u128>,
    /// True if a competitor arrived before us (wins a tie).
    pub competitor_first: bool,
}

/// What a strategy may look at: the winning tips of earlier rounds only.
#[derive(Debug, Clone, Default)]
pub struct PastView {
    revealed: Vec<u128>,
}

impl PastView {
    pub fn winning_tips(&self) -> &[u128] {
        &self.revealed
    }
}

/// The opportunity as the strategy sees it (no competitor bids).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub gross_usd: f64,
    pub gas_units: u64,
    pub base_fee_wei: u128,
    pub usd_per_wei: f64,
}

pub trait Strategy {
    /// A tip (wei/gas) to send with, or `None` to skip.
    fn bid(&mut self, offer: &Offer, past: &PastView) -> Option<u128>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayResult {
    pub rounds: usize,
    pub bids: usize,
    pub wins: usize,
    pub losses: usize,
    pub skipped: usize,
    pub tips_paid_usd: f64,
    pub gas_paid_usd: f64,
    /// Profit won minus all gas and tips, USD.
    pub net_usd: f64,
}

/// Replay `rounds` through `strategy`, revealing each round's winning tip only afterwards.
pub fn replay(rounds: &[Round], strategy: &mut dyn Strategy) -> ReplayResult {
    let mut past = PastView::default();
    let mut r = ReplayResult {
        rounds: rounds.len(),
        ..Default::default()
    };
    for round in rounds {
        let offer = Offer {
            gross_usd: round.gross_usd,
            gas_units: round.gas_units,
            base_fee_wei: round.base_fee_wei,
            usd_per_wei: round.usd_per_wei,
        };
        let best_rival = round.competitor_tips.iter().copied().max();
        match strategy.bid(&offer, &past) {
            None => r.skipped += 1,
            Some(tip) => {
                r.bids += 1;
                let gas = round.gas_units as f64;
                let gas_usd = gas * round.base_fee_wei as f64 * round.usd_per_wei;
                let tip_usd = gas * tip as f64 * round.usd_per_wei;
                let won = match best_rival {
                    None => true,
                    Some(b) => tip > b || (tip == b && !round.competitor_first),
                };
                r.gas_paid_usd += gas_usd;
                r.tips_paid_usd += tip_usd;
                // A loser still lands (after the winner) and reverts: pays gas and its tip.
                r.net_usd += if won { round.gross_usd } else { 0.0 } - gas_usd - tip_usd;
                if won {
                    r.wins += 1
                } else {
                    r.losses += 1
                }
                past.revealed.push(best_rival.map_or(tip, |b| b.max(tip)));
                continue;
            }
        }
        // Skipped: the winning rival tip is still observable on-chain afterwards.
        if let Some(b) = best_rival {
            past.revealed.push(b);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_probability_rises_with_the_bid() {
        let past = [10u128, 20, 30, 40];
        let ps: Vec<f64> = [0u128, 10, 15, 30, 41, 1000]
            .iter()
            .map(|b| win_probability(*b, &past))
            .collect();
        assert!(ps.windows(2).all(|w| w[0] <= w[1]), "{ps:?}");
        assert_eq!(ps[0], 0.0);
        assert_eq!(ps[1], 0.125, "a tie counts half");
        assert_eq!(*ps.last().unwrap(), 1.0);
        assert_eq!(win_probability(5, &[]), 0.5);
    }

    /// Bids a fixed tip; records what it was shown to prove it never sees the current round.
    struct Fixed {
        tip: u128,
        seen: Vec<usize>,
    }

    impl Strategy for Fixed {
        fn bid(&mut self, _o: &Offer, past: &PastView) -> Option<u128> {
            self.seen.push(past.winning_tips().len());
            Some(self.tip)
        }
    }

    fn rounds(rivals: &[u128]) -> Vec<Round> {
        rivals
            .iter()
            .map(|r| Round {
                gross_usd: 1.0,
                gas_units: 300_000,
                base_fee_wei: 10_000_000,
                usd_per_wei: 3000.0 / 1e18,
                competitor_tips: vec![*r],
                competitor_first: false,
            })
            .collect()
    }

    #[test]
    fn replay_charges_losers_and_reveals_only_past_rounds() {
        let rs = rounds(&[1_000, 5_000_000, 0]);
        let mut s = Fixed {
            tip: 2_000_000,
            seen: vec![],
        };
        let r = replay(&rs, &mut s);
        assert_eq!((r.bids, r.wins, r.losses), (3, 2, 1));
        assert_eq!(s.seen, vec![0, 1, 2], "round i sees only i earlier reveals");
        // Net = 2 wins × $1 − 3 × (gas + tip).
        let per_tx = 300_000.0 * (10_000_000.0 + 2_000_000.0) * 3000.0 / 1e18;
        assert!((r.net_usd - (2.0 - 3.0 * per_tx)).abs() < 1e-9);
    }

    #[test]
    fn ties_go_to_the_earlier_arrival() {
        let mut rs = rounds(&[7]);
        rs[0].competitor_first = true;
        let r = replay(
            &rs,
            &mut Fixed {
                tip: 7,
                seen: vec![],
            },
        );
        assert_eq!((r.wins, r.losses), (0, 1));
        rs[0].competitor_first = false;
        let r = replay(
            &rs,
            &mut Fixed {
                tip: 7,
                seen: vec![],
            },
        );
        assert_eq!((r.wins, r.losses), (1, 0));
    }
}
