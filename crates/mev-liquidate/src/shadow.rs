//! Shadow mandatory-scheduler pass (JEV-LIQ-005).
//!
//! Additive: the `Runner` calls `ShadowSchedule::spawn_pass` from its existing `select!` loop on
//! a timer. The pass reads the durable repository on a blocking thread, builds the deterministic
//! schedule and only records it (tracing JSON plus events in the `mev-metrics` format through the
//! optional sink). It never changes what the runner polls, plans or sends.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mev_liq_sched::{ShadowParams, ShadowReport, run_shadow_pass};
use mev_liq_state::SnapshotRepository;
use tracing::{info, warn};

/// `(kind, data)`, the shape of `mev_metrics::Metrics::event`, so the engine can pass
/// `move |k, d| metrics.event(k, d)` without this crate depending on the metrics crate.
pub type ShadowSink = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

pub const EVENT_SCHEDULE: &str = "liq_sched_shadow";
pub const EVENT_COUNTERS: &str = "liq_sched_counters";

#[derive(Clone)]
pub struct ShadowSchedule {
    /// Off by default: a disabled schedule never runs.
    pub enabled: bool,
    pub every: Duration,
    pub params: ShadowParams,
    pub repo: Arc<SnapshotRepository>,
    pub sink: Option<ShadowSink>,
    busy: Arc<AtomicBool>,
}

impl ShadowSchedule {
    /// Disabled until `.enabled(true)`.
    pub fn new(repo: Arc<SnapshotRepository>, params: ShadowParams) -> Self {
        Self {
            enabled: false,
            every: Duration::from_secs(5),
            params,
            repo,
            sink: None,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn every(mut self, d: Duration) -> Self {
        self.every = d.max(Duration::from_millis(100));
        self
    }

    pub fn with_sink(mut self, sink: ShadowSink) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Run one pass on a blocking thread unless the previous one is still running. The caller
    /// never waits for it.
    pub fn spawn_pass(&self) {
        if !self.enabled || self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let now_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
            match run_shadow_pass(&me.repo, &me.params, now_ns) {
                Ok(report) => me.record(&report),
                Err(e) => warn!(error = %e, "liquidation shadow schedule failed"),
            }
            me.busy.store(false, Ordering::Release);
        });
    }

    fn record(&self, report: &ShadowReport) {
        let (Ok(full), Ok(counters)) = (
            serde_json::to_value(report),
            serde_json::to_value(&report.counters),
        ) else {
            warn!("liquidation shadow schedule not serializable");
            return;
        };
        info!(
            target: "liq_sched_shadow",
            p0 = report.counters.candidates.p0,
            p1 = report.counters.candidates.p1,
            p2 = report.counters.candidates.p2,
            p3 = report.counters.candidates.p3,
            dispatched = report.counters.dispatched,
            backlog_p0 = report.counters.backlog_p0,
            backlog_p1 = report.counters.backlog_p1,
            record = %full,
            "shadow schedule"
        );
        if report.needs_alert() {
            warn!(
                target: "liq_sched_shadow",
                misses_p0 = report.counters.deadline_misses_p0,
                misses_p1 = report.counters.deadline_misses_p1,
                coverage_violations = report.counters.coverage_violations,
                "ALERT liquidation scheduler deadline miss or coverage violation"
            );
        }
        if let Some(sink) = &self.sink {
            sink(EVENT_SCHEDULE, full);
            sink(EVENT_COUNTERS, counters);
        }
    }
}

/// Wait for the next shadow tick; pending forever when the pass is disabled.
pub(crate) async fn tick_opt(iv: &mut Option<tokio::time::Interval>) {
    match iv {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}
