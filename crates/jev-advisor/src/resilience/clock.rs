//! Injectable monotonic clock. Every timing decision in the resilience layer goes through it:
//! there is no wall-clock read anywhere in decision logic.

use std::future::Future;
use std::time::Duration;

/// Why a sleep was requested. Lets test clocks advance only the sleeps they mean to (backoff)
/// without firing in-flight request timers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SleepPurpose {
    Backoff,
    AttemptTimeout,
    /// Bound on waiting for an in-flight slot, a coalesced leader, or any other deadline race.
    DeadlineWait,
}

/// Monotonic time source. `now` is the elapsed time since an arbitrary fixed origin; budget
/// windows are aligned to that origin, not to wall-clock hours.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, d: Duration, purpose: SleepPurpose) -> impl Future<Output = ()> + Send;
}

/// Production clock on tokio time (respects `tokio::time::pause`).
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    origin: tokio::time::Instant,
}

impl TokioClock {
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TokioClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
    fn sleep(&self, d: Duration, _purpose: SleepPurpose) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(d)
    }
}

/// Deterministic jitter source (SplitMix64). Seeded by the composition root; tests use fixed seeds.
pub trait JitterSource: Send {
    fn next_u64(&mut self) -> u64;
}

#[derive(Debug, Clone)]
pub struct SeededRng(u64);

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl JitterSource for SeededRng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_reference_vector() {
        // First outputs of SplitMix64 with seed 0 (published reference values).
        let mut r = SeededRng::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }
}
