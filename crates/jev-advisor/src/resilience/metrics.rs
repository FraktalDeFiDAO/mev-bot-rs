//! In-crate counters matching the spec section 11 metric names. Task 013 maps a
//! [`MetricsSnapshot`] onto the exporter. Labels are fixed enums: never a borrower, account,
//! request key or any other unbounded value.

use std::collections::BTreeMap;
use std::time::Duration;

use super::FallbackReason;
use super::breaker::CircuitState;
use crate::error::{RetryableKind, TransportError};

/// Upper bounds (seconds) of the `copper_jev_request_duration_seconds` buckets.
pub const DURATION_BUCKETS_SECS: [f64; 8] = [0.05, 0.1, 0.25, 0.5, 1.0, 1.5, 2.5, 5.0];

/// Label for `copper_jev_requests_total{result}` and `..._duration_seconds{status}`.
pub fn result_label(r: &Result<(), &TransportError>) -> &'static str {
    match r {
        Ok(()) => "ok",
        Err(TransportError::Retryable { kind, .. }) => match kind {
            RetryableKind::RateLimited => "rate_limited",
            RetryableKind::Overloaded => "overloaded",
            RetryableKind::Server(_) => "server_error",
            RetryableKind::Connection => "connection_error",
        },
        Err(TransportError::Timeout(_)) => "timeout",
        Err(TransportError::Auth(_)) => "auth_error",
        Err(TransportError::BadRequest(_)) => "bad_request",
        Err(TransportError::Cancelled) => "cancelled",
        Err(
            TransportError::SchemaDrift(_)
            | TransportError::WrongModel { .. }
            | TransportError::NotJson
            | TransportError::MalformedJson
            | TransportError::Oversized { .. },
        ) => "schema_error",
        Err(TransportError::Config(_) | TransportError::UnexpectedStatus(_)) => "other_error",
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DurationStat {
    pub count: u64,
    pub sum_seconds: f64,
    /// Per-bucket (non-cumulative) counts, aligned with [`DURATION_BUCKETS_SECS`]; the last extra
    /// slot is the `+Inf` overflow.
    pub buckets: [u64; 9],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricsSnapshot {
    /// `copper_jev_requests_total{result}` (one per HTTP attempt).
    pub requests_total: BTreeMap<&'static str, u64>,
    /// `copper_jev_request_duration_seconds{status}`.
    pub request_duration: BTreeMap<&'static str, DurationStat>,
    /// `copper_jev_circuit_state` (0 closed, 1 half-open, 2 open or misconfigured).
    pub circuit_state: u8,
    pub circuit_misconfigured: bool,
    /// `copper_jev_fallback_total{reason}` (one per call that returned `Unavailable`).
    pub fallback_total: BTreeMap<&'static str, u64>,
    /// `copper_jev_input_tokens_total`.
    pub input_tokens_total: u64,
    pub output_tokens_total: u64,
    /// `copper_jev_estimated_cost_usd_total` in micro-USD (divide by 1e6 for the exporter).
    pub estimated_cost_micro_usd_total: u64,
    pub cache_hits_total: u64,
    pub coalesced_total: u64,
    /// Gauge: requests currently on the wire.
    pub inflight: usize,
    /// Gauges for the current budget window.
    pub window_requests_used: u32,
    pub window_tokens_used: u64,
    pub window_micro_usd_used: u64,
}

#[derive(Debug, Default)]
pub(crate) struct MetricsInner {
    requests_total: BTreeMap<&'static str, u64>,
    request_duration: BTreeMap<&'static str, DurationStat>,
    fallback_total: BTreeMap<&'static str, u64>,
    input_tokens: u64,
    output_tokens: u64,
    cost_micro_usd: u64,
    cache_hits: u64,
    coalesced: u64,
}

impl MetricsInner {
    pub(crate) fn attempt(&mut self, label: &'static str, took: Duration) {
        *self.requests_total.entry(label).or_default() += 1;
        let d = self.request_duration.entry(label).or_default();
        d.count += 1;
        let secs = took.as_secs_f64();
        d.sum_seconds += secs;
        let i = DURATION_BUCKETS_SECS
            .iter()
            .position(|b| secs <= *b)
            .unwrap_or(DURATION_BUCKETS_SECS.len());
        d.buckets[i] += 1;
    }
    pub(crate) fn fallback(&mut self, reason: FallbackReason) {
        *self.fallback_total.entry(reason.label()).or_default() += 1;
    }
    pub(crate) fn usage(&mut self, input: u64, output: u64, cost_micro_usd: u64) {
        self.input_tokens = self.input_tokens.saturating_add(input);
        self.output_tokens = self.output_tokens.saturating_add(output);
        self.cost_micro_usd = self.cost_micro_usd.saturating_add(cost_micro_usd);
    }
    pub(crate) fn cache_hit(&mut self) {
        self.cache_hits += 1;
    }
    pub(crate) fn coalesced(&mut self) {
        self.coalesced += 1;
    }

    pub(crate) fn snapshot(
        &self,
        state: CircuitState,
        inflight: usize,
        requests: u32,
        tokens: u64,
        micro_usd: u64,
    ) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.clone(),
            request_duration: self.request_duration.clone(),
            circuit_state: state.gauge(),
            circuit_misconfigured: state == CircuitState::Misconfigured,
            fallback_total: self.fallback_total.clone(),
            input_tokens_total: self.input_tokens,
            output_tokens_total: self.output_tokens,
            estimated_cost_micro_usd_total: self.cost_micro_usd,
            cache_hits_total: self.cache_hits,
            coalesced_total: self.coalesced,
            inflight,
            window_requests_used: requests,
            window_tokens_used: tokens,
            window_micro_usd_used: micro_usd,
        }
    }
}
