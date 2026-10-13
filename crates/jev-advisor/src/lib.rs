//! TypeSafe (Jev) REST client for the liquidation advisor (JEV-LIQ-009).
//!
//! Transport and strict validation only. Advisory output is never eligibility, profit or safety
//! evidence and has no veto authority (spec section 0). Retry, circuit breaker and quotas are
//! JEV-LIQ-010 and wrap [`Transport`] ([`ResilientClient`]).

mod client;
mod error;
mod request;
mod resilience;
mod response;
mod secret;

pub use client::{
    ClientConfig, MAX_RETRY_AFTER, Transport, TypeSafeClient, parse_retry_after, validate_endpoint,
};
pub use error::{ErrorClass, RetryableKind, TimeoutKind, TransportError, ValidationError};
pub use request::{
    ANSWER_IDS, AdvisoryRequest, QUESTIONS_JSON, build_request, prompt_digest, request_key,
};
pub use resilience::{
    Admission, Breaker, BreakerConfig, CacheConfig, CircuitState, Clock, ConfigError,
    DURATION_BUCKETS_SECS, DurationStat, FallbackReason, InflightPolicy, JitterSource,
    MetricsSnapshot, Outcome, Quota, QuotaConfig, RatioTrip, ResilienceConfig, ResilientClient,
    RetryConfig, SeededRng, SleepPurpose, Source, TokenPrices, TokioClock, TtlLru, Unavailable,
    backoff_delay,
};
pub use response::{
    AdvisoryAnswer, MonitoringValueAnswer, ResponsePolicy, UnknownTopLevel, Usage, WatchTier,
    WatchTierAnswer, parse_response, response_digest,
};
pub use secret::{Secret, SecretError};
pub use tokio_util::sync::CancellationToken;
