//! How much earlier does the paid Fast Feed deliver a transaction than the free standard feed?
//!
//! Both feeds carry the same ordered transactions. This module pairs the two arrivals of each
//! transaction (by hash) and measures the gap, so the decision to pay for the Fast Feed rests on
//! data (`docs/runbooks/fast-feed-experiment.md`). Pure logic with an injected clock: no I/O.
//! Floats here are diagnostics only; nothing in this module feeds a trading decision.

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use alloy::primitives::B256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Standard,
    Fast,
}

/// One transaction seen on both feeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadSample {
    pub hash: B256,
    pub first: Feed,
    /// Milliseconds the earlier feed was ahead of the later one (>= 0).
    pub lead_ms: f64,
}

impl LeadSample {
    /// `standard arrival − fast arrival`: positive when the Fast Feed was earlier.
    pub fn fast_advantage_ms(&self) -> f64 {
        match self.first {
            Feed::Fast => self.lead_ms,
            Feed::Standard => -self.lead_ms,
        }
    }
}

/// Pairs arrivals of the same transaction from the two feeds.
#[derive(Debug)]
pub struct LeadTracker {
    pending: HashMap<B256, (Feed, Instant)>,
    order: VecDeque<(Instant, B256)>,
    window: Duration,
    /// Seen on the Fast Feed, never on the standard feed within `window`.
    pub only_fast: u64,
    /// Seen on the standard feed, never on the Fast Feed within `window`: the Fast Feed missed it.
    pub only_standard: u64,
}

impl LeadTracker {
    pub fn new(window: Duration) -> Self {
        Self {
            pending: HashMap::new(),
            order: VecDeque::new(),
            window,
            only_fast: 0,
            only_standard: 0,
        }
    }

    /// Record an arrival. Returns the sample when this is the second feed to deliver the hash.
    /// A repeat from the same feed (reconnect replay) is ignored.
    pub fn observe(&mut self, hash: B256, feed: Feed, at: Instant) -> Option<LeadSample> {
        self.expire(at);
        match self.pending.get(&hash) {
            Some((f, _)) if *f == feed => None,
            Some((first, first_at)) => {
                let lead_ms = at.saturating_duration_since(*first_at).as_secs_f64() * 1e3;
                let sample = LeadSample {
                    hash,
                    first: *first,
                    lead_ms,
                };
                self.pending.remove(&hash);
                Some(sample)
            }
            None => {
                self.pending.insert(hash, (feed, at));
                self.order.push_back((at, hash));
                None
            }
        }
    }

    /// Count and forget arrivals older than the window whose other copy never came.
    pub fn expire(&mut self, now: Instant) {
        while let Some((t, h)) = self.order.front().copied() {
            if now.saturating_duration_since(t) < self.window {
                break;
            }
            self.order.pop_front();
            if let Some((feed, at)) = self.pending.get(&h).copied()
                && at == t
            {
                self.pending.remove(&h);
                match feed {
                    Feed::Fast => self.only_fast += 1,
                    Feed::Standard => self.only_standard += 1,
                }
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// Accumulated `fast_advantage_ms` samples.
#[derive(Debug, Default, Clone)]
pub struct LeadStats {
    advantage: Vec<f64>,
}

/// What the experiment reports.
#[derive(Debug, Clone, PartialEq)]
pub struct LeadSummary {
    pub n: usize,
    /// Share of transactions the Fast Feed delivered first.
    pub fast_first: f64,
    pub p10: f64,
    pub median: f64,
    pub p90: f64,
    pub mean: f64,
    /// Share where the Fast Feed led by at least one PGA round (125 ms).
    pub ge_one_round: f64,
}

impl LeadStats {
    pub fn add(&mut self, s: &LeadSample) {
        self.advantage.push(s.fast_advantage_ms());
    }

    pub fn len(&self) -> usize {
        self.advantage.len()
    }

    pub fn is_empty(&self) -> bool {
        self.advantage.is_empty()
    }

    pub fn summary(&self) -> Option<LeadSummary> {
        if self.advantage.is_empty() {
            return None;
        }
        let mut v = self.advantage.clone();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let q = |p: f64| v[((n - 1) as f64 * p).round() as usize];
        let share =
            |pred: &dyn Fn(f64) -> bool| v.iter().filter(|x| pred(**x)).count() as f64 / n as f64;
        Some(LeadSummary {
            n,
            fast_first: share(&|x| x > 0.0),
            p10: q(0.1),
            median: q(0.5),
            p90: q(0.9),
            mean: v.iter().sum::<f64>() / n as f64,
            ge_one_round: share(&|x| x >= 125.0),
        })
    }
}

impl LeadSummary {
    /// The experiment's decision rule (runbook): the premise holds if the Fast Feed's median lead is
    /// at least 100 ms and it was first for most transactions.
    pub fn premise_holds(&self) -> bool {
        self.median >= 100.0 && self.fast_first > 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> B256 {
        B256::repeat_byte(n)
    }

    #[test]
    fn the_second_arrival_completes_a_sample_with_the_gap() {
        let mut t = LeadTracker::new(Duration::from_secs(30));
        let t0 = Instant::now();
        assert!(t.observe(h(1), Feed::Fast, t0).is_none());
        let s = t
            .observe(h(1), Feed::Standard, t0 + Duration::from_millis(180))
            .expect("pair");
        assert_eq!(s.first, Feed::Fast);
        assert!((s.lead_ms - 180.0).abs() < 1e-6);
        assert!((s.fast_advantage_ms() - 180.0).abs() < 1e-6);
        assert_eq!(t.pending(), 0);
        // Standard first gives a negative advantage.
        t.observe(h(2), Feed::Standard, t0);
        let s = t
            .observe(h(2), Feed::Fast, t0 + Duration::from_millis(40))
            .unwrap();
        assert!((s.fast_advantage_ms() + 40.0).abs() < 1e-6);
    }

    #[test]
    fn a_repeat_from_the_same_feed_is_ignored() {
        let mut t = LeadTracker::new(Duration::from_secs(30));
        let t0 = Instant::now();
        t.observe(h(1), Feed::Fast, t0);
        assert!(
            t.observe(h(1), Feed::Fast, t0 + Duration::from_millis(5))
                .is_none()
        );
        let s = t
            .observe(h(1), Feed::Standard, t0 + Duration::from_millis(100))
            .unwrap();
        assert!(
            (s.lead_ms - 100.0).abs() < 1e-6,
            "measured from the first arrival"
        );
    }

    #[test]
    fn arrivals_never_matched_are_counted_per_feed_when_they_expire() {
        let mut t = LeadTracker::new(Duration::from_secs(10));
        let t0 = Instant::now();
        t.observe(h(1), Feed::Fast, t0);
        t.observe(h(2), Feed::Standard, t0);
        t.observe(h(3), Feed::Standard, t0 + Duration::from_secs(1));
        t.expire(t0 + Duration::from_millis(10_500));
        assert_eq!(
            (t.only_fast, t.only_standard),
            (1, 1),
            "older than the window; the third is 9.5 s old"
        );
        t.expire(t0 + Duration::from_secs(12));
        assert_eq!((t.only_fast, t.only_standard), (1, 2));
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_late_second_copy_after_expiry_is_a_fresh_first_arrival() {
        let mut t = LeadTracker::new(Duration::from_secs(5));
        let t0 = Instant::now();
        t.observe(h(1), Feed::Fast, t0);
        let late = t0 + Duration::from_secs(9);
        assert!(t.observe(h(1), Feed::Standard, late).is_none());
        assert_eq!(t.only_fast, 1);
    }

    #[test]
    fn the_summary_reports_quantiles_and_the_decision() {
        let mut s = LeadStats::default();
        assert!(s.summary().is_none());
        let t0 = Instant::now();
        let mut t = LeadTracker::new(Duration::from_secs(60));
        // Fast Feed ahead by 20, 120, 130, 140, 150 ms and behind by 10 ms.
        for (i, adv) in [20.0, 120.0, 130.0, 140.0, 150.0, -10.0].iter().enumerate() {
            let hash = h(i as u8 + 1);
            let (first, second, gap) = if *adv >= 0.0 {
                (Feed::Fast, Feed::Standard, *adv)
            } else {
                (Feed::Standard, Feed::Fast, -*adv)
            };
            t.observe(hash, first, t0);
            let sample = t
                .observe(
                    hash,
                    second,
                    t0 + Duration::from_micros((gap * 1000.0) as u64),
                )
                .unwrap();
            s.add(&sample);
        }
        let sum = s.summary().unwrap();
        assert_eq!(sum.n, 6);
        assert!((sum.fast_first - 5.0 / 6.0).abs() < 1e-9);
        assert!((sum.median - 130.0).abs() < 1.0, "median {}", sum.median);
        assert!(
            (sum.ge_one_round - 3.0 / 6.0).abs() < 1e-9,
            "130, 140 and 150 ms reach a 125 ms round"
        );
        assert!(sum.premise_holds());
        // A small lead does not justify the build.
        let mut weak = LeadStats::default();
        let mut t = LeadTracker::new(Duration::from_secs(60));
        for i in 0..10u8 {
            t.observe(h(i + 1), Feed::Fast, t0);
            weak.add(
                &t.observe(h(i + 1), Feed::Standard, t0 + Duration::from_millis(30))
                    .unwrap(),
            );
        }
        assert!(!weak.summary().unwrap().premise_holds());
    }
}
