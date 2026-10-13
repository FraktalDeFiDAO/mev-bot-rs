//! Circuit breaker state machine (pure: time is an argument).
//!
//! `Closed -> Open` after N consecutive retryable failures, or after the failure ratio in a
//! sliding window crosses the threshold. `Open -> HalfOpen` once the cooldown elapsed and a call
//! asks for admission; exactly one probe is admitted. A probe success closes the breaker, a probe
//! failure re-opens it (fresh cooldown). Authentication failures move it to `Misconfigured`, which
//! never auto-probes: only `config_changed` (the operator corrected the key or endpoint and the
//! owner reloaded the client) returns it to `Closed`.

use std::collections::VecDeque;
use std::time::Duration;

use super::FallbackReason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatioTrip {
    /// Trip when failures/total >= this (permille) within `window`.
    pub failure_permille: u16,
    pub window: Duration,
    /// Minimum samples in the window before the ratio is considered.
    pub min_samples: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakerConfig {
    pub consecutive_failures: u32,
    pub cooldown: Duration,
    pub ratio: Option<RatioTrip>,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_failures: 5,
            cooldown: Duration::from_secs(30),
            ratio: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    HalfOpen,
    Open,
    Misconfigured,
}

impl CircuitState {
    /// `copper_jev_circuit_state` gauge: 0 closed, 1 half-open, 2 open. `Misconfigured` reports 2
    /// (it is open); the snapshot carries a separate `misconfigured` flag.
    pub fn gauge(self) -> u8 {
        match self {
            Self::Closed => 0,
            Self::HalfOpen => 1,
            Self::Open | Self::Misconfigured => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Allowed,
    /// The single half-open probe; report its outcome with `probe = true`.
    Probe,
    Rejected(FallbackReason),
}

const MAX_SAMPLES: usize = 4096;

#[derive(Debug)]
pub struct Breaker {
    cfg: BreakerConfig,
    state: CircuitState,
    consecutive: u32,
    opened_at: Duration,
    probe_in_flight: bool,
    samples: VecDeque<(Duration, bool)>,
}

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            state: CircuitState::Closed,
            consecutive: 0,
            opened_at: Duration::ZERO,
            probe_in_flight: false,
            samples: VecDeque::new(),
        }
    }

    pub fn state(&self) -> CircuitState {
        self.state
    }

    pub fn admit(&mut self, now: Duration) -> Admission {
        match self.state {
            CircuitState::Closed => Admission::Allowed,
            CircuitState::Misconfigured => Admission::Rejected(FallbackReason::Misconfigured),
            CircuitState::Open => {
                if now.saturating_sub(self.opened_at) >= self.cfg.cooldown {
                    self.state = CircuitState::HalfOpen;
                    self.probe_in_flight = true;
                    Admission::Probe
                } else {
                    Admission::Rejected(FallbackReason::CircuitOpen)
                }
            }
            CircuitState::HalfOpen => {
                if self.probe_in_flight {
                    Admission::Rejected(FallbackReason::CircuitOpen)
                } else {
                    self.probe_in_flight = true;
                    Admission::Probe
                }
            }
        }
    }

    /// The admitted call never reached the service (cancelled, rejected locally): give the probe
    /// slot back without judging health.
    pub fn release(&mut self, probe: bool) {
        if probe && self.state == CircuitState::HalfOpen {
            self.probe_in_flight = false;
        }
    }

    /// The service answered (including a non-retryable rejection that proves reachability).
    pub fn on_reachable(&mut self, now: Duration, probe: bool) {
        match self.state {
            CircuitState::HalfOpen if probe => {
                self.state = CircuitState::Closed;
                self.probe_in_flight = false;
                self.consecutive = 0;
                self.samples.clear();
            }
            CircuitState::Closed => {
                self.consecutive = 0;
                self.record(now, false);
            }
            _ => {}
        }
    }

    /// A retryable failure (429/529/5xx/transport/timeout).
    pub fn on_failure(&mut self, now: Duration, probe: bool) {
        match self.state {
            CircuitState::HalfOpen if probe => self.open(now),
            CircuitState::Closed => {
                self.consecutive = self.consecutive.saturating_add(1);
                self.record(now, true);
                if self.consecutive >= self.cfg.consecutive_failures || self.ratio_tripped(now) {
                    self.open(now);
                }
            }
            _ => {}
        }
    }

    /// 401/403: the key or endpoint is wrong. Stays failed until `config_changed`.
    pub fn on_auth_failure(&mut self) {
        self.state = CircuitState::Misconfigured;
        self.probe_in_flight = false;
    }

    /// Operator corrected the configuration.
    pub fn config_changed(&mut self) {
        self.state = CircuitState::Closed;
        self.consecutive = 0;
        self.probe_in_flight = false;
        self.samples.clear();
    }

    fn open(&mut self, now: Duration) {
        self.state = CircuitState::Open;
        self.opened_at = now;
        self.probe_in_flight = false;
        self.consecutive = 0;
        self.samples.clear();
    }

    fn record(&mut self, now: Duration, failure: bool) {
        let Some(r) = &self.cfg.ratio else { return };
        let horizon = now.saturating_sub(r.window);
        while self.samples.front().is_some_and(|(t, _)| *t < horizon) {
            self.samples.pop_front();
        }
        if self.samples.len() >= MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back((now, failure));
    }

    fn ratio_tripped(&self, now: Duration) -> bool {
        let Some(r) = &self.cfg.ratio else {
            return false;
        };
        let horizon = now.saturating_sub(r.window);
        let (mut total, mut failed) = (0u64, 0u64);
        for (t, f) in &self.samples {
            if *t >= horizon {
                total += 1;
                failed += u64::from(*f);
            }
        }
        total >= u64::from(r.min_samples.max(1))
            && failed * 1000 >= total * u64::from(r.failure_permille)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    fn cfg() -> BreakerConfig {
        BreakerConfig {
            consecutive_failures: 3,
            cooldown: s(10),
            ratio: None,
        }
    }

    #[test]
    fn closed_open_halfopen_closed() {
        let mut b = Breaker::new(cfg());
        for _ in 0..2 {
            assert_eq!(b.admit(s(0)), Admission::Allowed);
            b.on_failure(s(0), false);
        }
        assert_eq!(b.state(), CircuitState::Closed);
        b.on_failure(s(0), false);
        assert_eq!(b.state(), CircuitState::Open);
        assert_eq!(
            b.admit(s(9)),
            Admission::Rejected(FallbackReason::CircuitOpen)
        );
        assert_eq!(b.admit(s(10)), Admission::Probe);
        assert_eq!(b.state(), CircuitState::HalfOpen);
        // Only one probe at a time.
        assert_eq!(
            b.admit(s(10)),
            Admission::Rejected(FallbackReason::CircuitOpen)
        );
        b.on_reachable(s(11), true);
        assert_eq!(b.state(), CircuitState::Closed);
        assert_eq!(b.admit(s(11)), Admission::Allowed);
    }

    #[test]
    fn failed_probe_reopens_with_fresh_cooldown() {
        let mut b = Breaker::new(cfg());
        for _ in 0..3 {
            b.on_failure(s(0), false);
        }
        assert_eq!(b.admit(s(10)), Admission::Probe);
        b.on_failure(s(12), true);
        assert_eq!(b.state(), CircuitState::Open);
        assert_eq!(
            b.admit(s(21)),
            Admission::Rejected(FallbackReason::CircuitOpen)
        );
        assert_eq!(b.admit(s(22)), Admission::Probe);
    }

    #[test]
    fn success_resets_the_consecutive_count() {
        let mut b = Breaker::new(cfg());
        for _ in 0..10 {
            b.on_failure(s(0), false);
            b.on_failure(s(0), false);
            b.on_reachable(s(0), false);
        }
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[test]
    fn released_probe_slot_can_be_retaken() {
        let mut b = Breaker::new(cfg());
        for _ in 0..3 {
            b.on_failure(s(0), false);
        }
        assert_eq!(b.admit(s(10)), Admission::Probe);
        b.release(true);
        assert_eq!(b.admit(s(10)), Admission::Probe);
    }

    #[test]
    fn auth_failure_is_sticky_until_config_changes() {
        let mut b = Breaker::new(cfg());
        b.on_auth_failure();
        assert_eq!(b.state().gauge(), 2);
        assert_eq!(
            b.admit(s(1_000_000)),
            Admission::Rejected(FallbackReason::Misconfigured)
        );
        b.on_failure(s(0), false);
        b.on_reachable(s(0), true);
        assert_eq!(b.state(), CircuitState::Misconfigured);
        b.config_changed();
        assert_eq!(b.admit(s(0)), Admission::Allowed);
    }

    #[test]
    fn failure_ratio_in_window_opens() {
        let mut b = Breaker::new(BreakerConfig {
            consecutive_failures: 100,
            cooldown: s(10),
            ratio: Some(RatioTrip {
                failure_permille: 500,
                window: s(60),
                min_samples: 6,
            }),
        });
        // Alternating failure/success never has 2 consecutive failures but is ~50% failing.
        for i in 0..6u64 {
            if i % 2 == 0 {
                b.on_failure(s(i), false);
            } else {
                b.on_reachable(s(i), false);
            }
        }
        assert_eq!(b.state(), CircuitState::Closed);
        b.on_failure(s(6), false); // 7 samples, 4 failed = 571 permille
        assert_eq!(b.state(), CircuitState::Open);
    }

    #[test]
    fn old_samples_leave_the_window() {
        let mut b = Breaker::new(BreakerConfig {
            consecutive_failures: 100,
            cooldown: s(10),
            ratio: Some(RatioTrip {
                failure_permille: 500,
                window: s(60),
                min_samples: 4,
            }),
        });
        for i in 0..3 {
            b.on_failure(s(i), false);
        }
        // The old failures are outside the window by now (1 sample < min 4).
        b.on_failure(s(1000), false);
        assert_eq!(b.state(), CircuitState::Closed);
    }
}
