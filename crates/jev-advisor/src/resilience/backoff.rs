//! Exponential backoff with jitter, integer math only.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryConfig {
    /// Total network attempts per call, first attempt included.
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Fraction (permille, 0..=1000) of the delay that may be shaved off by jitter.
    /// 0 = deterministic schedule; 500 = each delay is uniform in [50%, 100%] of the cap.
    pub jitter_permille: u16,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            jitter_permille: 500,
        }
    }
}

/// Delay before the retry that follows the `failed_attempts`-th failure (1-based):
/// `cap = min(max_delay, base * 2^(n-1))`, `delay = cap - cap * jitter * r / 2^64`.
pub fn backoff_delay(cfg: &RetryConfig, failed_attempts: u32, r: u64) -> Duration {
    let exp = failed_attempts.saturating_sub(1).min(40);
    let base = cfg.base_delay.as_nanos();
    let raw = base.saturating_mul(1u128 << exp);
    let cap = raw.min(cfg.max_delay.as_nanos());
    let jp = u128::from(cfg.jitter_permille.min(1000));
    let cut = cap * jp / 1000;
    let sub = (cut * u128::from(r)) >> 64;
    Duration::from_nanos(u64::try_from(cap - sub).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_jitter_schedule_doubles_to_cap() {
        let c = RetryConfig {
            jitter_permille: 0,
            ..RetryConfig::default()
        };
        let ms: Vec<u128> = (1..=7)
            .map(|n| backoff_delay(&c, n, u64::MAX).as_millis())
            .collect();
        assert_eq!(ms, [100, 200, 400, 800, 1600, 2000, 2000]);
    }

    #[test]
    fn jitter_stays_within_band_and_hits_the_edges() {
        let c = RetryConfig::default();
        assert_eq!(backoff_delay(&c, 1, 0), Duration::from_millis(100));
        let lo = backoff_delay(&c, 1, u64::MAX);
        assert!(lo >= Duration::from_millis(50) && lo < Duration::from_millis(51));
    }

    #[test]
    fn huge_attempt_counts_do_not_overflow() {
        let c = RetryConfig::default();
        assert_eq!(backoff_delay(&c, u32::MAX, 0), c.max_delay);
    }
}
