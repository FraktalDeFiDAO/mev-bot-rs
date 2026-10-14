//! Retry, circuit breaker, quota, cache and single-flight around [`crate::Transport`]
//! (JEV-LIQ-010). Advisory only: every failure mode degrades to a typed [`Unavailable`] that the
//! caller maps to the deterministic baseline; nothing here blocks past the caller's deadline.

mod backoff;
mod breaker;
mod cache;
mod client;
mod clock;
mod metrics;
mod quota;

use std::time::Duration;

use crate::error::TransportError;

pub use backoff::{RetryConfig, backoff_delay};
pub use breaker::{Admission, Breaker, BreakerConfig, CircuitState, RatioTrip};
pub use cache::TtlLru;
pub use client::{CacheConfig, InflightPolicy, Outcome, ResilienceConfig, ResilientClient, Source};
pub use clock::{Clock, JitterSource, SeededRng, SleepPurpose, TokioClock};
pub use metrics::{DURATION_BUCKETS_SECS, DurationStat, MetricsSnapshot};
pub use quota::{Quota, QuotaConfig, TokenPrices};

/// Why a call returned no advisory. The label is the `copper_jev_fallback_total{reason}` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FallbackReason {
    /// Breaker open (or half-open with the probe taken): no network was touched.
    CircuitOpen,
    /// Auth rejected; sticky until [`ResilientClient::config_changed`].
    Misconfigured,
    RequestBudget,
    TokenBudget,
    CostBudget,
    /// In-flight cap reached (fail-fast policy, or waiting ran out the deadline).
    InflightSaturated,
    /// The caller's deadline (work item TTL) is spent or the next wait would pass it.
    DeadlineExceeded,
    Cancelled,
    /// Retryable failures used up `max_attempts`.
    RetriesExhausted,
    /// A non-retryable rejection (400/401/403/422, schema drift, wrong model, ...).
    NonRetryable,
    /// A recent identical request failed schema validation; not re-sent until the negative TTL.
    NegativeCached,
}

impl FallbackReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::CircuitOpen => "circuit_open",
            Self::Misconfigured => "misconfigured",
            Self::RequestBudget => "request_budget",
            Self::TokenBudget => "token_budget",
            Self::CostBudget => "cost_budget",
            Self::InflightSaturated => "inflight_saturated",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled => "cancelled",
            Self::RetriesExhausted => "retries_exhausted",
            Self::NonRetryable => "non_retryable",
            Self::NegativeCached => "negative_cached",
        }
    }
}

/// The typed "no advisory" result. Always safe to treat as "use the deterministic baseline".
#[derive(Debug, Clone, PartialEq)]
pub struct Unavailable {
    pub reason: FallbackReason,
    /// The last transport error seen by the call that produced this, if any network was tried.
    pub last_error: Option<TransportError>,
}

/// Invalid [`ResilienceConfig`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid resilience config: {0}")]
pub struct ConfigError(pub &'static str);

/// Longest single wait the layer ever schedules is bounded by the caller's deadline; this only
/// bounds config values.
pub(crate) const MAX_CONFIG_DURATION: Duration = Duration::from_secs(86_400);
