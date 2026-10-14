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

use mev_liq_sched::{
    AdvisoryInputs, AdvisoryPolicy, JevHealth, LoadedInputs, ShadowAdvisoryReport, ShadowParams,
    ShadowReport, run_shadow_pass, run_shadow_pass_advisory,
};
use mev_liq_state::SnapshotRepository;
use tracing::{info, warn};

/// `(kind, data)`, the shape of `mev_metrics::Metrics::event`, so the engine can pass
/// `move |k, d| metrics.event(k, d)` without this crate depending on the metrics crate.
pub type ShadowSink = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

pub const EVENT_SCHEDULE: &str = "liq_sched_shadow";
pub const EVENT_COUNTERS: &str = "liq_sched_counters";
/// Advisory (Jev) shadow description: what the active ordering would have done (JEV-LIQ-011).
pub const EVENT_ADVISORY: &str = "liq_advisory_shadow";

/// Supplies already-stored advisory decisions for the loaded state (e.g. from 008's
/// `AdvisorStore::valid_decision_for`). The policy re-validates every clause.
pub type DecisionFn = Arc<dyn Fn(&LoadedInputs, u64) -> AdvisoryInputs + Send + Sync>;
/// Current Jev breaker health (010).
pub type HealthFn = Arc<dyn Fn() -> JevHealth + Send + Sync>;

/// Optional advisory add-on of the shadow pass. Dispatch stays the baseline schedule.
#[derive(Clone)]
pub struct AdvisoryShadow {
    pub policy: AdvisoryPolicy,
    pub decisions: DecisionFn,
    pub health: HealthFn,
}

#[derive(Clone)]
pub struct ShadowSchedule {
    /// Off by default: a disabled schedule never runs.
    pub enabled: bool,
    pub every: Duration,
    pub params: ShadowParams,
    pub repo: Arc<SnapshotRepository>,
    pub sink: Option<ShadowSink>,
    pub advisory: Option<AdvisoryShadow>,
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
            advisory: None,
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

    /// Also record the advisory shadow report (nothing extra when the policy mode is `Off`).
    pub fn with_advisory(mut self, a: AdvisoryShadow) -> Self {
        self.advisory = Some(a);
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
            // One span per pass: chain, pass id (the scheduler clock) and, once known, the newest block and
            // highest revision among the recorded jobs. Never an account or pool address.
            let span = pass_span(me.params.chain_id, now_ns);
            let _entered = span.enter();
            match &me.advisory {
                None => match run_shadow_pass(&me.repo, &me.params, now_ns) {
                    Ok(report) => {
                        record_pass_span(&span, &report);
                        me.record(&report)
                    }
                    Err(e) => warn!(error = %e, "liquidation shadow schedule failed"),
                },
                Some(a) => {
                    let policy = a.policy.clone().with_health((a.health)());
                    let src = |l: &LoadedInputs, n: u64| (a.decisions)(l, n);
                    match run_shadow_pass_advisory(&me.repo, &me.params, now_ns, &policy, &src) {
                        Ok((report, adv)) => {
                            record_pass_span(&span, &report);
                            me.record(&report);
                            if let Some(adv) = adv {
                                me.record_advisory(&adv);
                            }
                        }
                        Err(e) => warn!(error = %e, "liquidation advisory shadow failed"),
                    }
                }
            }
            me.busy.store(false, Ordering::Release);
        });
    }

    fn record_advisory(&self, report: &ShadowAdvisoryReport) {
        let Ok(full) = serde_json::to_value(report) else {
            warn!("liquidation advisory shadow not serializable");
            return;
        };
        info!(
            target: "liq_advisory_shadow",
            p2_jobs = report.p2_jobs,
            applied = report.applied.len(),
            order_changed = report.order_changed,
            stale = report.counters.copper_jev_stale_response_total,
            mandatory_unchanged = report.mandatory_unchanged,
            record = %full,
            "advisory shadow"
        );
        if !report.mandatory_unchanged {
            warn!(
                target: "liq_advisory_shadow",
                "ALERT advisory ordering changed P0/P1 output (invariant breach)"
            );
        }
        if let Some(sink) = &self.sink {
            sink(EVENT_ADVISORY, full);
        }
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

/// Span of one shadow pass (`pass_id` is the scheduler clock in ns, the job id of the pass).
pub(crate) fn pass_span(chain_id: u64, now_ns: u64) -> tracing::Span {
    tracing::info_span!(
        "liq_shadow_pass",
        chain_id,
        pass_id = now_ns,
        block = tracing::field::Empty,
        revision = tracing::field::Empty,
        jobs = tracing::field::Empty,
    )
}

/// Attach what the pass saw: newest snapshot block, highest snapshot revision (over the recorded jobs
/// and backlog) and the dispatched count.
pub(crate) fn record_pass_span(span: &tracing::Span, report: &ShadowReport) {
    let versions = report
        .jobs
        .iter()
        .chain(report.backlog.iter())
        .map(|j| &j.decision.source_version);
    if let Some(b) = versions.clone().filter_map(|v| v.snapshot_block).max() {
        span.record("block", b);
    }
    if let Some(r) = versions.map(|v| v.snapshot_revision).max() {
        span.record("revision", r);
    }
    span.record("jobs", report.counters.dispatched);
}

#[cfg(test)]
#[path = "shadow_span_tests.rs"]
mod span_tests;

/// Wait for the next shadow tick; pending forever when the pass is disabled.
pub(crate) async fn tick_opt(iv: &mut Option<tokio::time::Interval>) {
    match iv {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}
