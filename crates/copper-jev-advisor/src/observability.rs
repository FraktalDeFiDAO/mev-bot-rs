//! Prometheus rendering of the advisor's own state (JEV-LIQ-013). The exporter never queries anything:
//! a refresher task copies [`Worker::jev_metrics`], [`Worker::queue_stats`] and [`Worker::counters`] into a
//! [`SnapshotCache`] every few seconds, and the scrape collector only formats that cache. A scrape therefore
//! cannot block the worker loop, the SQLite handle or the HTTP client.
//!
//! Labels are fixed enums only (`result`, `status`, `reason`, `state`, `event`); no queue row, request key,
//! account or address ever reaches a label.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use jev_advisor::{CancellationToken, DURATION_BUCKETS_SECS, MetricsSnapshot};
use mev_liq_state::QueueStats;
use mev_metrics::{Exposition, Kind, Registry};

use crate::worker::{CountersSnapshot, Worker};

/// Every family [`render_advisor`] can produce (all of them once the snapshot is fully populated).
pub const ADVISOR_FAMILIES: &[&str] = &[
    "copper_jev_requests_total",
    "copper_jev_request_duration_seconds",
    "copper_jev_circuit_state",
    "copper_jev_circuit_misconfigured",
    "copper_jev_fallback_total",
    "copper_jev_stale_response_total",
    "copper_jev_input_tokens_total",
    "copper_jev_output_tokens_total",
    "copper_jev_estimated_cost_usd_total",
    "copper_jev_cache_hits_total",
    "copper_jev_coalesced_total",
    "copper_jev_inflight",
    "copper_jev_window_requests_used",
    "copper_jev_window_tokens_used",
    "copper_jev_window_cost_usd_used",
    "copper_jev_advisor_queue_rows",
    "copper_jev_advisor_queue_missed_total",
    "copper_jev_advisor_queue_stats_ok",
    "copper_jev_advisor_events_total",
];

#[derive(Debug, Clone, Default)]
pub struct AdvisorSnapshot {
    pub jev: MetricsSnapshot,
    /// `None` when the queue statistics query failed.
    pub queue: Option<QueueStats>,
    pub counters: CountersSnapshot,
}

/// Latest snapshot, written by the refresher and read by the scrape collector.
#[derive(Clone, Default)]
pub struct SnapshotCache(Arc<Mutex<Option<AdvisorSnapshot>>>);

impl SnapshotCache {
    pub fn store(&self, s: AdvisorSnapshot) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(s);
    }
    pub fn load(&self) -> Option<AdvisorSnapshot> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Register the advisor collector on `registry`; it renders nothing until the first snapshot is stored.
pub fn install(registry: &Registry, cache: &SnapshotCache) {
    let cache = cache.clone();
    registry.add_collector(move |ex| {
        if let Some(s) = cache.load() {
            render_advisor(ex, &s);
        }
    });
}

/// Copy the worker's state into `cache` every `every` until `stop`. The blocking SQLite query runs on the
/// blocking pool, so a slow disk delays the next snapshot, not the worker or a scrape.
pub async fn refresh_loop(
    worker: Arc<Worker>,
    cache: SnapshotCache,
    every: Duration,
    stop: CancellationToken,
) {
    let mut tick = tokio::time::interval(every.max(Duration::from_millis(250)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
        }
        let w = Arc::clone(&worker);
        let queue = tokio::task::spawn_blocking(move || w.queue_stats().ok())
            .await
            .unwrap_or(None);
        cache.store(AdvisorSnapshot {
            jev: worker.jev_metrics(),
            queue,
            counters: worker.counters(),
        });
    }
}

fn micro_to_usd(micro: u64) -> f64 {
    micro as f64 / 1_000_000.0
}

pub fn render_advisor(ex: &mut Exposition, s: &AdvisorSnapshot) {
    let j = &s.jev;
    if !j.requests_total.is_empty() {
        ex.family(
            "copper_jev_requests_total",
            "Jev HTTP attempts by result (retries included).",
            Kind::Counter,
        );
        for (k, v) in &j.requests_total {
            ex.sample_u64(&[("result", k)], *v);
        }
    }
    if !j.request_duration.is_empty() {
        ex.family(
            "copper_jev_request_duration_seconds",
            "Jev HTTP attempt duration by status.",
            Kind::Histogram,
        );
        for (k, d) in &j.request_duration {
            ex.histogram(
                &[("status", k)],
                &DURATION_BUCKETS_SECS,
                &d.buckets,
                d.sum_seconds,
            );
        }
    }
    ex.family(
        "copper_jev_circuit_state",
        "Jev circuit breaker: 0 closed, 1 half-open, 2 open or misconfigured.",
        Kind::Gauge,
    );
    ex.sample_u64(&[], u64::from(j.circuit_state));
    ex.family(
        "copper_jev_circuit_misconfigured",
        "1 when the breaker is open because Jev rejected our credentials (401/403).",
        Kind::Gauge,
    );
    ex.sample_u64(&[], u64::from(j.circuit_misconfigured));
    if !j.fallback_total.is_empty() {
        ex.family(
            "copper_jev_fallback_total",
            "Calls that returned no advisory (baseline used), by reason.",
            Kind::Counter,
        );
        for (k, v) in &j.fallback_total {
            ex.sample_u64(&[("reason", k)], *v);
        }
    }
    ex.family(
        "copper_jev_stale_response_total",
        "Answers dropped because the decision was stale or expired.",
        Kind::Counter,
    );
    ex.sample_u64(&[], s.counters.discarded_stale_response);
    ex.family(
        "copper_jev_input_tokens_total",
        "Input tokens billed by Jev.",
        Kind::Counter,
    );
    ex.sample_u64(&[], j.input_tokens_total);
    ex.family(
        "copper_jev_output_tokens_total",
        "Output tokens billed by Jev.",
        Kind::Counter,
    );
    ex.sample_u64(&[], j.output_tokens_total);
    ex.family(
        "copper_jev_estimated_cost_usd_total",
        "Estimated Jev spend in USD (zero unless prices are configured).",
        Kind::Counter,
    );
    ex.sample(&[], micro_to_usd(j.estimated_cost_micro_usd_total));
    ex.family(
        "copper_jev_cache_hits_total",
        "Answers served from the cache.",
        Kind::Counter,
    );
    ex.sample_u64(&[], j.cache_hits_total);
    ex.family(
        "copper_jev_coalesced_total",
        "Requests coalesced onto an in-flight call.",
        Kind::Counter,
    );
    ex.sample_u64(&[], j.coalesced_total);
    ex.family(
        "copper_jev_inflight",
        "Jev requests currently on the wire.",
        Kind::Gauge,
    );
    ex.sample_u64(&[], j.inflight as u64);
    ex.family(
        "copper_jev_window_requests_used",
        "HTTP attempts used in the current budget window.",
        Kind::Gauge,
    );
    ex.sample_u64(&[], u64::from(j.window_requests_used));
    ex.family(
        "copper_jev_window_tokens_used",
        "Tokens used in the current budget window.",
        Kind::Gauge,
    );
    ex.sample_u64(&[], j.window_tokens_used);
    ex.family(
        "copper_jev_window_cost_usd_used",
        "Estimated USD used in the current budget window.",
        Kind::Gauge,
    );
    ex.sample(&[], micro_to_usd(j.window_micro_usd_used));

    ex.family(
        "copper_jev_advisor_queue_stats_ok",
        "1 when the advisor queue statistics query succeeded.",
        Kind::Gauge,
    );
    ex.sample_u64(&[], u64::from(s.queue.is_some()));
    if let Some(q) = &s.queue {
        ex.family(
            "copper_jev_advisor_queue_rows",
            "Advisor outbox rows by state.",
            Kind::Gauge,
        );
        for (state, n) in [
            ("pending", q.pending),
            ("in_flight", q.in_flight),
            ("done", q.done),
            ("dead", q.dead),
        ] {
            ex.sample_u64(&[("state", state)], n);
        }
        ex.family(
            "copper_jev_advisor_queue_missed_total",
            "Export rows shed by the producer before the advisor copied them.",
            Kind::Counter,
        );
        ex.sample_u64(&[], q.missed_total);
    }
    let c = &s.counters;
    ex.family(
        "copper_jev_advisor_events_total",
        "Advisor worker counters by event.",
        Kind::Counter,
    );
    for (event, n) in [
        ("pulled", c.pulled),
        ("claimed", c.claimed),
        ("applied", c.applied),
        ("superseded", c.superseded),
        ("discarded_stale_response", c.discarded_stale_response),
        ("dead_invalid_features", c.dead_invalid_features),
        ("dead_invalid_response", c.dead_invalid_response),
        ("dead_non_retryable", c.dead_non_retryable),
        ("failed_retry", c.failed_retry),
        ("released", c.released),
        ("lease_lost", c.lease_lost),
        ("export_rows_missed", c.export_rows_missed),
    ] {
        ex.sample_u64(&[("event", event)], n);
    }
}
