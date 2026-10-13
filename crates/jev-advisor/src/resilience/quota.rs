//! Hourly request / token / estimated-USD budgets (pure: time is an argument).
//!
//! Windows are fixed, aligned to the injected clock's origin (`index = now / window`), so a reset
//! is a deterministic function of the clock. Money is integer micro-USD.

use std::time::Duration;

use super::FallbackReason;
use crate::response::Usage;

/// Per-token prices, supplied by configuration. No default exists: TypeSafe pricing is not
/// published in the spec and this crate never invents it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenPrices {
    /// Micro-USD per one million input tokens.
    pub input_micro_usd_per_mtok: u64,
    /// Micro-USD per one million output tokens.
    pub output_micro_usd_per_mtok: u64,
}

impl TokenPrices {
    pub fn cost_micro_usd(&self, usage: &Usage) -> u64 {
        let c = (u128::from(usage.input_tokens) * u128::from(self.input_micro_usd_per_mtok))
            .saturating_add(
                u128::from(usage.output_tokens) * u128::from(self.output_micro_usd_per_mtok),
            );
        u64::try_from(c / 1_000_000).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaConfig {
    /// Budget window (spec: hourly).
    pub window: Duration,
    /// HTTP requests (attempts, retries included) per window. Spec default 200.
    pub requests_per_window: u32,
    /// Input + output tokens per window. `None` = not enforced (default).
    pub tokens_per_window: Option<u64>,
    /// Estimated spend per window in micro-USD. Enforced only when `prices` is also set;
    /// `validate` rejects a budget without prices. `None` = disabled (default).
    pub usd_micro_per_window: Option<u64>,
    pub prices: Option<TokenPrices>,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(3600),
            requests_per_window: 200,
            tokens_per_window: None,
            usd_micro_per_window: None,
            prices: None,
        }
    }
}

#[derive(Debug)]
pub struct Quota {
    cfg: QuotaConfig,
    window_index: u64,
    requests: u32,
    tokens: u64,
    micro_usd: u64,
}

impl Quota {
    pub fn new(cfg: QuotaConfig) -> Self {
        Self {
            cfg,
            window_index: 0,
            requests: 0,
            tokens: 0,
            micro_usd: 0,
        }
    }

    fn roll(&mut self, now: Duration) {
        let w = self.cfg.window.as_nanos().max(1);
        let idx = u64::try_from(now.as_nanos() / w).unwrap_or(u64::MAX);
        if idx != self.window_index {
            self.window_index = idx;
            self.requests = 0;
            self.tokens = 0;
            self.micro_usd = 0;
        }
    }

    /// Reserve one HTTP request. Token and USD spend is known only after an answer arrives, so
    /// those budgets are checked against what is already spent (overshoot is bounded by the
    /// in-flight cap times one answer's usage).
    pub fn reserve(&mut self, now: Duration) -> Result<(), FallbackReason> {
        self.roll(now);
        if self.requests >= self.cfg.requests_per_window {
            return Err(FallbackReason::RequestBudget);
        }
        if self.cfg.tokens_per_window.is_some_and(|b| self.tokens >= b) {
            return Err(FallbackReason::TokenBudget);
        }
        if self.cfg.prices.is_some()
            && self
                .cfg
                .usd_micro_per_window
                .is_some_and(|b| self.micro_usd >= b)
        {
            return Err(FallbackReason::CostBudget);
        }
        self.requests += 1;
        Ok(())
    }

    /// Account a successful answer's usage to the window current at `now`. Returns the estimated
    /// micro-USD charged (0 without prices).
    pub fn record_usage(&mut self, now: Duration, usage: &Usage) -> u64 {
        self.roll(now);
        self.tokens = self
            .tokens
            .saturating_add(usage.input_tokens)
            .saturating_add(usage.output_tokens);
        let cost = self.cfg.prices.map_or(0, |p| p.cost_micro_usd(usage));
        self.micro_usd = self.micro_usd.saturating_add(cost);
        cost
    }

    pub fn requests_used(&self) -> u32 {
        self.requests
    }
    pub fn tokens_used(&self) -> u64 {
        self.tokens
    }
    pub fn micro_usd_used(&self) -> u64 {
        self.micro_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    fn usage(i: u64, o: u64) -> Usage {
        Usage {
            input_tokens: i,
            output_tokens: o,
        }
    }

    #[test]
    fn request_budget_and_reset_at_the_window_boundary() {
        let mut q = Quota::new(QuotaConfig {
            requests_per_window: 2,
            ..QuotaConfig::default()
        });
        assert!(q.reserve(s(0)).is_ok());
        assert!(q.reserve(s(3599)).is_ok());
        assert_eq!(q.reserve(s(3599)), Err(FallbackReason::RequestBudget));
        assert!(q.reserve(s(3600)).is_ok());
        assert_eq!(q.requests_used(), 1);
    }

    #[test]
    fn token_budget_uses_recorded_usage() {
        let mut q = Quota::new(QuotaConfig {
            tokens_per_window: Some(1000),
            ..QuotaConfig::default()
        });
        assert!(q.reserve(s(0)).is_ok());
        q.record_usage(s(0), &usage(600, 399));
        assert!(q.reserve(s(1)).is_ok());
        q.record_usage(s(1), &usage(1, 0));
        assert_eq!(q.reserve(s(2)), Err(FallbackReason::TokenBudget));
        assert!(q.reserve(s(3600)).is_ok());
    }

    #[test]
    fn cost_budget_needs_prices_and_is_integer_math() {
        let p = TokenPrices {
            input_micro_usd_per_mtok: 3_000_000,
            output_micro_usd_per_mtok: 15_000_000,
        };
        assert_eq!(p.cost_micro_usd(&usage(412, 31)), 1236 + 465);
        let mut q = Quota::new(QuotaConfig {
            usd_micro_per_window: Some(1000),
            prices: Some(p),
            ..QuotaConfig::default()
        });
        assert!(q.reserve(s(0)).is_ok());
        assert_eq!(q.record_usage(s(0), &usage(412, 31)), 1701);
        assert_eq!(q.reserve(s(1)), Err(FallbackReason::CostBudget));
    }

    #[test]
    fn usd_budget_without_prices_is_inert() {
        let mut q = Quota::new(QuotaConfig {
            usd_micro_per_window: Some(1),
            prices: None,
            ..QuotaConfig::default()
        });
        assert!(q.reserve(s(0)).is_ok());
        assert_eq!(q.record_usage(s(0), &usage(1_000_000, 1_000_000)), 0);
        assert!(q.reserve(s(1)).is_ok());
    }

    #[test]
    fn extreme_usage_saturates() {
        let p = TokenPrices {
            input_micro_usd_per_mtok: u64::MAX,
            output_micro_usd_per_mtok: u64::MAX,
        };
        assert_eq!(p.cost_micro_usd(&usage(u64::MAX, u64::MAX)), u64::MAX);
    }
}
