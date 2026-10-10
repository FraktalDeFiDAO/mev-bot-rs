//! Per-route win-rate gate for arbitrage sends (DECISIONS #54). Every lost race costs gas and the
//! tip, and nothing else stops a route that always loses. Each route (buy pool, sell pool) keeps its
//! recent outcomes; a send goes out only if its expected value is positive:
//!
//!   p_win × net − (1 − p_win) × cost > 0
//!
//! `p_win` is the route's own record smoothed toward the global one (so a new route starts from what
//! the bot wins on average, not from nothing). Every route gets `min_attempts` tries before the
//! gate judges it, losing streaks back a route off exponentially, and old results age out so a
//! route that stopped being contested is retried. Pure: callers pass the clock.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::persist::Store;

#[derive(Debug, Clone)]
pub struct GateConfig {
    pub enabled: bool,
    /// Tries every route gets before the gate judges it.
    pub min_attempts: u32,
    /// Weight of the global win rate when smoothing a route's own (in pseudo-attempts).
    pub prior_strength: f64,
    /// Floor on the global win rate used as the prior (so 0 wins so far doesn't mean "never").
    pub prior_floor: f64,
    /// Consecutive losses after which a route is backed off.
    pub backoff_after: u32,
    /// First back-off, seconds; doubled per further loss, capped at `backoff_max_secs`.
    pub backoff_secs: u64,
    pub backoff_max_secs: u64,
    /// Results older than this are forgotten, seconds.
    pub memory_secs: u64,
    /// Global stop: after this many sends inside the memory window with no win at all, stop sending (routes
    /// still get their free tries otherwise, so a market the bot cannot win costs one loss per route, forever).
    /// 0 turns it off.
    pub global_min_attempts: u32,
    /// While stopped, one probe send is allowed this often (seconds), so a market that changed is noticed.
    pub global_probe_secs: u64,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_attempts: 3,
            prior_strength: 5.0,
            prior_floor: 0.02,
            backoff_after: 3,
            backoff_secs: 600,
            backoff_max_secs: 6 * 3600,
            memory_secs: 24 * 3600,
            global_min_attempts: 30,
            global_probe_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Go,
    /// Still inside the free tries (or the gate is off).
    Explore,
    /// Expected value is not positive.
    NegativeEv {
        p_win: f64,
        ev_usd: f64,
    },
    /// No win in `attempts` sends across all routes: stopped, next probe at `next_probe_secs` (unix).
    GlobalStop {
        attempts: u64,
        next_probe_secs: u64,
    },
    /// Too many losses in a row; retry after `until_secs` (unix).
    BackedOff {
        losses: u32,
        until_secs: u64,
    },
}

impl Verdict {
    pub fn allows(&self) -> bool {
        matches!(self, Verdict::Go | Verdict::Explore)
    }
}

#[derive(Debug, Clone, Default)]
struct Stats {
    attempts: u32,
    wins: u32,
    losses_in_row: u32,
    last_result: u64,
    /// When the last loss happened (for the back-off window).
    last_loss: u64,
}

#[derive(Debug, Default)]
pub struct RouteGate<K: std::hash::Hash + Eq + Copy> {
    routes: HashMap<K, Stats>,
    global_attempts: u64,
    global_wins: u64,
    /// When the global window started and when the last probe went out (unix seconds).
    global_since: u64,
    last_probe: u64,
    store: Option<Store>,
}

/// What survives a restart: the global record (routes are cheap to relearn; the global stop is not).
#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedGate {
    since: u64,
    attempts: u64,
    wins: u64,
    last_probe: u64,
}

impl<K: std::hash::Hash + Eq + Copy> RouteGate<K> {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            global_attempts: 0,
            global_wins: 0,
            global_since: 0,
            last_probe: 0,
            store: None,
        }
    }

    /// Keep the global record in `path` and resume from it (a window older than `memory_secs` is dropped).
    pub fn with_persistence(mut self, path: PathBuf, cfg: &GateConfig, now: u64) -> Self {
        let store = Store::new(path);
        if let Some(s) = store.load::<SavedGate>()
            && now.saturating_sub(s.since) < cfg.memory_secs
        {
            self.global_since = s.since;
            self.global_attempts = s.attempts;
            self.global_wins = s.wins;
            self.last_probe = s.last_probe;
            tracing::info!(
                attempts = s.attempts,
                wins = s.wins,
                "gate: resumed the global record from disk"
            );
        }
        self.store = Some(store);
        self
    }

    fn save(&self) {
        if let Some(store) = &self.store {
            store.save(&SavedGate {
                since: self.global_since,
                attempts: self.global_attempts,
                wins: self.global_wins,
                last_probe: self.last_probe,
            });
        }
    }

    /// The global win rate, floored.
    fn prior(&self, cfg: &GateConfig) -> f64 {
        let raw = (self.global_wins as f64 + 0.5) / (self.global_attempts as f64 + 10.0);
        raw.max(cfg.prior_floor)
    }

    /// Should a send on `route` go out? `net_usd` is the expected profit if it wins, `cost_usd`
    /// what a loss costs (gas plus the tip).
    pub fn check(
        &mut self,
        cfg: &GateConfig,
        route: K,
        net_usd: f64,
        cost_usd: f64,
        now: u64,
    ) -> Verdict {
        if !cfg.enabled {
            return Verdict::Explore;
        }
        self.forget(cfg, now);
        if cfg.global_min_attempts > 0
            && self.global_attempts >= u64::from(cfg.global_min_attempts)
            && self.global_wins == 0
        {
            let next = self.last_probe + cfg.global_probe_secs;
            if now < next {
                return Verdict::GlobalStop {
                    attempts: self.global_attempts,
                    next_probe_secs: next,
                };
            }
            // One probe: if it loses too, the stop holds for another period.
            self.last_probe = now;
            self.save();
            return Verdict::Explore;
        }
        let prior = self.prior(cfg);
        let Some(s) = self.routes.get(&route) else {
            return Verdict::Explore;
        };
        if s.losses_in_row >= cfg.backoff_after {
            let extra = s.losses_in_row - cfg.backoff_after;
            let wait = cfg
                .backoff_secs
                .saturating_mul(1u64 << extra.min(20))
                .min(cfg.backoff_max_secs);
            let until = s.last_loss + wait;
            if now < until {
                return Verdict::BackedOff {
                    losses: s.losses_in_row,
                    until_secs: until,
                };
            }
        }
        if s.attempts < cfg.min_attempts {
            return Verdict::Explore;
        }
        let p =
            (s.wins as f64 + prior * cfg.prior_strength) / (s.attempts as f64 + cfg.prior_strength);
        let ev = p * net_usd - (1.0 - p) * cost_usd;
        if ev > 0.0 {
            Verdict::Go
        } else {
            Verdict::NegativeEv {
                p_win: p,
                ev_usd: ev,
            }
        }
    }

    /// Record the outcome of a send on `route`.
    pub fn record(&mut self, route: K, won: bool, now: u64) {
        let s = self.routes.entry(route).or_default();
        s.attempts += 1;
        s.last_result = now;
        if self.global_attempts == 0 {
            self.global_since = now;
        }
        self.global_attempts += 1;
        if won {
            s.wins += 1;
            s.losses_in_row = 0;
            self.global_wins += 1;
        } else {
            s.losses_in_row += 1;
            s.last_loss = now;
        }
        self.save();
    }

    /// Drop routes with no result for `memory_secs`.
    fn forget(&mut self, cfg: &GateConfig, now: u64) {
        // The global window rolls with the same memory as the routes.
        if self.global_attempts > 0 && now.saturating_sub(self.global_since) >= cfg.memory_secs {
            self.global_attempts = 0;
            self.global_wins = 0;
            self.save();
        }
        self.routes
            .retain(|_, s| now.saturating_sub(s.last_result) < cfg.memory_secs);
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

/// Short-lived "don't retry this yet" marks (for example a route whose pre-send simulation just
/// reverted). Pure: callers pass the clock.
#[derive(Debug, Default)]
pub struct Cooldowns<K: std::hash::Hash + Eq + Copy> {
    marked: HashMap<K, u64>,
}

impl<K: std::hash::Hash + Eq + Copy> Cooldowns<K> {
    pub fn new() -> Self {
        Self {
            marked: HashMap::new(),
        }
    }

    pub fn mark(&mut self, key: K, now: u64) {
        self.marked.insert(key, now);
    }

    /// Was `key` marked less than `ttl_secs` ago?
    pub fn active(&self, key: &K, now: u64, ttl_secs: u64) -> bool {
        self.marked
            .get(key)
            .is_some_and(|t| now.saturating_sub(*t) < ttl_secs)
    }

    /// Forget marks older than `ttl_secs`.
    pub fn prune(&mut self, now: u64, ttl_secs: u64) {
        self.marked.retain(|_, t| now.saturating_sub(*t) < ttl_secs);
    }

    pub fn len(&self) -> usize {
        self.marked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000;

    fn cfg() -> GateConfig {
        GateConfig::default()
    }

    #[test]
    fn cooldowns_expire_and_prune() {
        let mut c = Cooldowns::<u8>::new();
        assert!(!c.active(&1, 100, 120));
        c.mark(1, 100);
        assert!(c.active(&1, 100, 120));
        assert!(c.active(&1, 219, 120));
        assert!(!c.active(&1, 220, 120), "expired exactly at the ttl");
        assert!(!c.active(&2, 100, 120), "other keys are unaffected");
        c.mark(2, 300);
        c.prune(330, 120);
        assert_eq!(c.len(), 1, "the old mark is dropped, the new one stays");
    }

    #[test]
    fn no_wins_anywhere_stops_all_sending_but_a_probe_each_period() {
        let c = GateConfig {
            global_min_attempts: 5,
            global_probe_secs: 100,
            ..cfg()
        };
        let mut g: RouteGate<u32> = RouteGate::new();
        for route in 0..5 {
            assert!(
                g.check(&c, route, 1.0, 0.1, 10).allows(),
                "free tries until the stop"
            );
            g.record(route, false, 10);
        }
        // A brand-new route is stopped too.
        assert!(matches!(
            g.check(&c, 99, 1.0, 0.1, 20),
            Verdict::GlobalStop { attempts: 5, .. }
        ));
        // After the probe period one send goes out, the next candidate is stopped again.
        assert_eq!(g.check(&c, 99, 1.0, 0.1, 130), Verdict::Explore);
        assert!(matches!(
            g.check(&c, 98, 1.0, 0.1, 131),
            Verdict::GlobalStop { .. }
        ));
        // A probe that wins lifts the stop.
        g.record(99, true, 140);
        assert!(g.check(&c, 97, 1.0, 0.1, 141).allows());
    }

    #[test]
    fn the_global_stop_is_off_at_zero_and_ages_out() {
        let off = GateConfig {
            global_min_attempts: 0,
            ..cfg()
        };
        let mut g: RouteGate<u32> = RouteGate::new();
        for route in 0..50 {
            g.record(route, false, 10);
        }
        assert!(g.check(&off, 1000, 1.0, 0.1, 20).allows());
        let on = GateConfig {
            global_min_attempts: 5,
            memory_secs: 1000,
            ..cfg()
        };
        assert!(matches!(
            g.check(&on, 1000, 1.0, 0.1, 20),
            Verdict::GlobalStop { .. }
        ));
        assert!(
            g.check(&on, 1000, 1.0, 0.1, 5000).allows(),
            "old losses are forgotten"
        );
    }

    #[test]
    fn the_global_record_survives_a_restart() {
        let c = GateConfig {
            global_min_attempts: 3,
            ..cfg()
        };
        let path = std::env::temp_dir().join(format!("gate-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut a: RouteGate<u32> = RouteGate::new().with_persistence(path.clone(), &c, 10);
        for route in 0..3 {
            a.record(route, false, 10);
        }
        let mut b: RouteGate<u32> = RouteGate::new().with_persistence(path.clone(), &c, 20);
        assert!(matches!(
            b.check(&c, 7, 1.0, 0.1, 20),
            Verdict::GlobalStop { .. }
        ));
        // A record from a previous window is not resumed.
        let b2: RouteGate<u32> =
            RouteGate::new().with_persistence(path.clone(), &c, 10 + c.memory_secs + 1);
        assert!(b2.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_new_route_gets_its_free_tries() {
        let mut g = RouteGate::<u8>::new();
        let c = cfg();
        assert_eq!(g.check(&c, 1, 0.05, 0.05, T0), Verdict::Explore);
        g.record(1, false, T0);
        g.record(1, false, T0 + 1);
        // Two losses, below the back-off and below min_attempts: still exploring.
        assert_eq!(g.check(&c, 1, 0.05, 0.05, T0 + 2), Verdict::Explore);
    }

    #[test]
    fn a_route_that_keeps_losing_is_backed_off_then_released() {
        let mut g = RouteGate::<u8>::new();
        let c = cfg();
        for i in 0..3 {
            g.record(1, false, T0 + i);
        }
        let v = g.check(&c, 1, 5.0, 0.04, T0 + 10);
        assert!(matches!(v, Verdict::BackedOff { losses: 3, .. }), "{v:?}");
        // After the first window it is judged again (and a fat profit still has positive EV).
        let v = g.check(&c, 1, 5.0, 0.04, T0 + 2 + 601);
        assert!(v.allows(), "{v:?}");
        // Another loss doubles the window.
        g.record(1, false, T0 + 700);
        let v = g.check(&c, 1, 5.0, 0.04, T0 + 700 + 601);
        assert!(matches!(v, Verdict::BackedOff { losses: 4, .. }), "{v:?}");
    }

    #[test]
    fn negative_ev_routes_are_skipped_and_big_wins_still_go() {
        let mut g = RouteGate::<u8>::new();
        let c = GateConfig {
            backoff_after: 99,
            ..cfg()
        };
        for i in 0..6 {
            g.record(1, false, T0 + i);
        }
        // p ~ prior-smoothed ~ 0.02: $0.05 net can't pay for $0.04 losses ...
        let v = g.check(&c, 1, 0.05, 0.04, T0 + 10);
        assert!(matches!(v, Verdict::NegativeEv { .. }), "{v:?}");
        // ... but a $5 opportunity can.
        assert_eq!(g.check(&c, 1, 5.0, 0.04, T0 + 10), Verdict::Go);
    }

    #[test]
    fn winning_routes_are_trusted() {
        let mut g = RouteGate::<u8>::new();
        let c = cfg();
        for i in 0..5 {
            g.record(7, i % 2 == 0, T0 + i); // 3 wins of 5
        }
        // 3 of 5 (smoothed to ~42 %) pays for itself at $0.10 against $0.04 losses ...
        assert_eq!(g.check(&c, 7, 0.10, 0.04, T0 + 10), Verdict::Go);
        // ... and not at $0.05: even a good route must clear its own costs.
        assert!(matches!(
            g.check(&c, 7, 0.05, 0.04, T0 + 10),
            Verdict::NegativeEv { .. }
        ));
    }

    #[test]
    fn a_new_route_inherits_the_global_record_after_its_tries() {
        let mut g = RouteGate::<u8>::new();
        let c = GateConfig {
            backoff_after: 99,
            global_min_attempts: 0, // this test is about the per-route judgement, not the global stop
            ..cfg()
        };
        // Everyone else loses a lot: the global prior sinks to its floor.
        for r in 10..20u8 {
            for i in 0..10 {
                g.record(r, false, T0 + i);
            }
        }
        for i in 0..3 {
            g.record(1, false, T0 + i);
        }
        assert!(matches!(
            g.check(&c, 1, 0.05, 0.04, T0 + 5),
            Verdict::NegativeEv { .. }
        ));
    }

    #[test]
    fn old_results_age_out_so_a_dead_route_is_retried_from_scratch() {
        let mut g = RouteGate::<u8>::new();
        let c = cfg();
        for i in 0..3 {
            g.record(1, false, T0 + i);
        }
        assert!(!g.check(&c, 1, 5.0, 0.04, T0 + 10).allows());
        let later = T0 + c.memory_secs + 100;
        assert_eq!(g.check(&c, 1, 0.05, 0.04, later), Verdict::Explore);
        assert!(g.is_empty());
    }

    #[test]
    fn disabled_gate_never_blocks() {
        let mut g = RouteGate::<u8>::new();
        let c = GateConfig {
            enabled: false,
            ..cfg()
        };
        for i in 0..50 {
            g.record(1, false, T0 + i);
        }
        assert_eq!(g.check(&c, 1, 0.0, 9.0, T0 + 60), Verdict::Explore);
    }

    #[test]
    fn a_win_resets_the_losing_streak() {
        let mut g = RouteGate::<u8>::new();
        let c = cfg();
        for i in 0..2 {
            g.record(1, false, T0 + i);
        }
        g.record(1, true, T0 + 3);
        g.record(1, false, T0 + 4);
        let v = g.check(&c, 1, 5.0, 0.04, T0 + 5);
        assert!(v.allows(), "streak is 1, not 3: {v:?}");
    }
}
