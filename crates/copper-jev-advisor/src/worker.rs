//! The advisor loop: pull export rows, claim with a lease, re-validate, ask Jev through the
//! resilient client, store the decision in shadow mode, settle the queue row.
//!
//! Safety properties, all advisory-side only:
//! * Nothing here can reach the hot path: the only outputs are rows in the advisory DB.
//! * Every Jev failure mode is a typed [`FallbackReason`]; none of them stops the loop.
//! * Every claimed row is settled exactly once (`ack`, `fail`, `dead_letter` or `release`), and
//!   rows still leased at shutdown are released so a restart replays them.
//! * Logs carry the queue row id, a request id and a 16-hex prefix of the feature digest. They
//!   never carry the borrower/pool address, the API key or any auth header.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::B256;
use jev_advisor::{
    AdvisoryAnswer, CancellationToken, ClientConfig, FallbackReason, MetricsSnapshot,
    ResilientClient, SeededRng, TokioClock, TransportError, TypeSafeClient, build_request,
    prompt_digest,
};
use mev_liq_features::SCHEMA_VERSION as FEATURE_SCHEMA;
use mev_liq_state::{
    AdvisorError, AdvisorStore, AdvisoryDecision, Claimed, DecisionConfig, DecisionMode,
    QueueConfig, QueueStats, WatchTier as StoredTier,
};
use tokio::task::JoinSet;
use tracing::{Instrument, error, info, info_span, warn};

use crate::config::Config;
use crate::features::parse_redacted;

/// Export rows copied into the claimable queue per tick.
const PULL_BATCH: u32 = 256;
/// After the grace period and the hard cancel, how long to wait for cancelled calls to release
/// their rows before aborting them.
const RELEASE_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("advisor storage: {0}")]
    Store(#[from] AdvisorError),
    #[error("jev client configuration: {0}")]
    Client(#[from] TransportError),
    #[error("resilience configuration: {0}")]
    Resilience(#[from] jev_advisor::ConfigError),
}

/// Monotonic counters of what the worker did. Fixed names, no per-account labels.
#[derive(Debug, Default)]
struct Counters {
    claimed: AtomicU64,
    applied: AtomicU64,
    superseded: AtomicU64,
    discarded_stale_response: AtomicU64,
    dead_invalid_features: AtomicU64,
    dead_invalid_response: AtomicU64,
    dead_non_retryable: AtomicU64,
    failed_retry: AtomicU64,
    released: AtomicU64,
    lease_lost: AtomicU64,
    pulled: AtomicU64,
    export_rows_missed: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CountersSnapshot {
    pub claimed: u64,
    /// Decisions stored (and rows acked).
    pub applied: u64,
    /// Rows acked without a request because the account had moved on.
    pub superseded: u64,
    /// Answers dropped by `apply_decision` as stale/expired (row acked).
    pub discarded_stale_response: u64,
    pub dead_invalid_features: u64,
    pub dead_invalid_response: u64,
    pub dead_non_retryable: u64,
    /// Rows returned to the queue with backoff.
    pub failed_retry: u64,
    /// Rows given back untouched (shutdown or systemic fallback).
    pub released: u64,
    pub lease_lost: u64,
    pub pulled: u64,
    pub export_rows_missed: u64,
}

impl Counters {
    fn snapshot(&self) -> CountersSnapshot {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        CountersSnapshot {
            claimed: g(&self.claimed),
            applied: g(&self.applied),
            superseded: g(&self.superseded),
            discarded_stale_response: g(&self.discarded_stale_response),
            dead_invalid_features: g(&self.dead_invalid_features),
            dead_invalid_response: g(&self.dead_invalid_response),
            dead_non_retryable: g(&self.dead_non_retryable),
            failed_retry: g(&self.failed_retry),
            released: g(&self.released),
            lease_lost: g(&self.lease_lost),
            pulled: g(&self.pulled),
            export_rows_missed: g(&self.export_rows_missed),
        }
    }
}

fn bump(a: &AtomicU64) {
    a.fetch_add(1, Ordering::Relaxed);
}

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn short(h: &B256) -> String {
    h.as_slice()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

type Client = ResilientClient<TypeSafeClient, TokioClock, SeededRng>;

pub struct Worker {
    cfg: Config,
    store: Arc<AdvisorStore>,
    client: Client,
    decision_cfg: DecisionConfig,
    owner: String,
    counters: Counters,
    leased: Mutex<HashSet<i64>>,
    pause_until: Mutex<Option<tokio::time::Instant>>,
    inflight_cancel: CancellationToken,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("owner", &self.owner)
            .finish()
    }
}

enum Verdict {
    /// Consume the row: nothing more to do with it.
    Ack(&'static str),
    Dead(String),
    Retry(String),
    Release,
}

fn classify_apply(e: &AdvisorError) -> Verdict {
    use AdvisorError::*;
    match e {
        Expired { .. }
        | IssuedInFuture
        | StaleRevision { .. }
        | FutureRevision { .. }
        | UnknownAccount
        | NoCurrentExport
        | DigestMismatch
        | SupersededResponse => Verdict::Ack("stale_response"),
        Malformed(_)
        | PolicyVersionMismatch
        | FeatureSchemaMismatch
        | ChainMismatch
        | ModelNotAllowed => Verdict::Dead(format!("response rejected: {e}")),
        other => Verdict::Retry(format!("store: {other}")),
    }
}

impl Worker {
    /// Open storage (read-only snapshot, read-write advisory) and build the resilient client.
    /// `client_cfg` is separate so the composition root (and tests) own transport settings.
    pub fn new(cfg: Config, client_cfg: ClientConfig) -> Result<Arc<Self>, WorkerError> {
        let queue = QueueConfig {
            max_pending: cfg.max_pending,
            max_in_flight: cfg.resilience.max_inflight as u64,
            max_attempts: cfg.max_attempts,
            base_backoff_ns: u64::try_from(cfg.retry_base.as_nanos()).unwrap_or(u64::MAX),
            max_backoff_ns: u64::try_from(cfg.retry_max.as_nanos()).unwrap_or(u64::MAX),
        };
        let store = Arc::new(AdvisorStore::open(
            &cfg.advisor_db,
            &cfg.snapshot_db,
            queue,
        )?);
        let transport = TypeSafeClient::new(client_cfg)?;
        let pid = u64::from(std::process::id());
        let start = now_ns();
        let client = ResilientClient::new(
            transport,
            TokioClock::new(),
            SeededRng::new(start ^ (pid << 32)),
            cfg.resilience.clone(),
        )?;
        let decision_cfg = DecisionConfig {
            chain_id: cfg.chain_id,
            policy_version: cfg.policy_version.clone(),
            feature_schema: FEATURE_SCHEMA.to_string(),
            allowed_models: cfg.allowed_models.clone(),
        };
        Ok(Arc::new(Self {
            owner: format!("copper-jev-advisor-{pid}-{start}"),
            cfg,
            store,
            client,
            decision_cfg,
            counters: Counters::default(),
            leased: Mutex::new(HashSet::new()),
            pause_until: Mutex::new(None),
            inflight_cancel: CancellationToken::new(),
        }))
    }

    pub fn counters(&self) -> CountersSnapshot {
        self.counters.snapshot()
    }

    pub fn jev_metrics(&self) -> MetricsSnapshot {
        self.client.snapshot()
    }

    pub fn queue_stats(&self) -> Result<QueueStats, AdvisorError> {
        self.store.stats(now_ns())
    }

    /// The storage handle (for readiness and tests).
    pub fn store(&self) -> &AdvisorStore {
        &self.store
    }

    async fn db<R: Send + 'static>(
        &self,
        f: impl FnOnce(&AdvisorStore) -> Result<R, AdvisorError> + Send + 'static,
    ) -> Result<R, AdvisorError> {
        let s = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || f(&s))
            .await
            .unwrap_or_else(|_| Err(AdvisorError::Corrupt("storage task panicked".into())))
    }

    fn pause(&self) {
        if !self.cfg.fallback_pause.is_zero() {
            *lock(&self.pause_until) = Some(tokio::time::Instant::now() + self.cfg.fallback_pause);
        }
    }

    fn paused(&self) -> bool {
        lock(&self.pause_until).is_some_and(|t| tokio::time::Instant::now() < t)
    }

    /// Run until `shutdown` fires, then drain gracefully. Returns the final counters.
    pub async fn run(
        self: &Arc<Self>,
        shutdown: CancellationToken,
    ) -> Result<CountersSnapshot, WorkerError> {
        info!(
            owner = %self.owner,
            mode = "shadow",
            model = %self.cfg.model,
            policy_version = %self.cfg.policy_version,
            max_inflight = self.cfg.resilience.max_inflight,
            ttl_seconds = self.cfg.ttl.as_secs(),
            "advisor worker started"
        );
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut metrics_tick = tokio::time::interval(self.cfg.metrics_interval);
        metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        metrics_tick.reset();

        loop {
            if shutdown.is_cancelled() {
                break;
            }
            while let Some(r) = tasks.try_join_next() {
                Self::reap(r);
            }
            if !self.paused() {
                let free = self.cfg.resilience.max_inflight.saturating_sub(tasks.len());
                if free > 0 {
                    self.fill(&mut tasks, free).await;
                }
            }
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                r = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(r) = r {
                        Self::reap(r);
                    }
                }
                _ = metrics_tick.tick() => self.log_metrics().await,
                () = tokio::time::sleep(self.cfg.poll) => {}
            }
        }

        info!(
            in_flight = tasks.len(),
            grace_seconds = self.cfg.shutdown_grace.as_secs(),
            "shutdown requested: no further claims, draining in-flight work"
        );
        if tokio::time::timeout(self.cfg.shutdown_grace, Self::drain(&mut tasks))
            .await
            .is_err()
        {
            warn!(
                in_flight = tasks.len(),
                "grace period elapsed: cancelling in-flight requests"
            );
            self.inflight_cancel.cancel();
            if tokio::time::timeout(RELEASE_WAIT, Self::drain(&mut tasks))
                .await
                .is_err()
            {
                tasks.abort_all();
                Self::drain(&mut tasks).await;
            }
        }
        self.release_leftovers().await;
        self.log_metrics().await;
        info!("advisor worker stopped");
        Ok(self.counters.snapshot())
    }

    fn reap(r: Result<(), tokio::task::JoinError>) {
        if let Err(e) = r
            && !e.is_cancelled()
        {
            error!(error = %e, "advisor task panicked");
        }
    }

    async fn drain(tasks: &mut JoinSet<()>) {
        while let Some(r) = tasks.join_next().await {
            Self::reap(r);
        }
    }

    async fn fill(self: &Arc<Self>, tasks: &mut JoinSet<()>, free: usize) {
        match self.db(|s| s.pull(now_ns(), PULL_BATCH)).await {
            Ok(p) => {
                self.counters.pulled.fetch_add(p.copied, Ordering::Relaxed);
                if p.missed > 0 {
                    self.counters
                        .export_rows_missed
                        .fetch_add(p.missed, Ordering::Relaxed);
                    warn!(
                        missed = p.missed,
                        "export rows were shed before they were copied"
                    );
                }
            }
            Err(e) => {
                error!(error = %e, "pull from the export log failed");
                return;
            }
        }
        let owner = self.owner.clone();
        let lease_ns = u64::try_from(self.cfg.ttl.as_nanos()).unwrap_or(u64::MAX);
        let items = match self
            .db(move |s| s.claim(&owner, now_ns(), lease_ns, free))
            .await
        {
            Ok(i) => i,
            Err(e) => {
                error!(error = %e, "claim failed");
                return;
            }
        };
        for item in items {
            bump(&self.counters.claimed);
            lock(&self.leased).insert(item.id);
            let w = Arc::clone(self);
            tasks.spawn(async move {
                let id = item.id;
                w.process(item).await;
                lock(&w.leased).remove(&id);
            });
        }
    }

    async fn release_leftovers(&self) {
        let ids: Vec<i64> = lock(&self.leased).drain().collect();
        for id in ids {
            self.settle(id, Verdict::Release).await;
        }
    }

    async fn settle(&self, id: i64, v: Verdict) {
        let owner = self.owner.clone();
        let r = match v {
            Verdict::Ack(reason) => {
                let r = self.db(move |s| s.ack(&owner, id, now_ns())).await;
                if r.is_ok() && reason == "superseded" {
                    bump(&self.counters.superseded);
                }
                r
            }
            Verdict::Dead(why) => {
                let w = why.clone();
                let r = self
                    .db(move |s| s.dead_letter(&owner, id, now_ns(), &w))
                    .await;
                if r.is_ok() {
                    warn!(item = id, reason = %why, "work item dead-lettered");
                }
                r
            }
            Verdict::Retry(why) => {
                let w = why.clone();
                let r = self.db(move |s| s.fail(&owner, id, now_ns(), &w)).await;
                if let Ok(out) = &r {
                    bump(&self.counters.failed_retry);
                    info!(item = id, reason = %why, outcome = ?out, "work item will be retried");
                }
                r.map(|_| ())
            }
            Verdict::Release => {
                let r = self.db(move |s| s.release(&owner, id, now_ns())).await;
                if r.is_ok() {
                    bump(&self.counters.released);
                }
                r
            }
        };
        match r {
            Ok(()) => {}
            Err(AdvisorError::LeaseLost(_)) => {
                bump(&self.counters.lease_lost);
                warn!(item = id, "lease lost before the row could be settled");
            }
            Err(e) => error!(item = id, error = %e, "could not settle work item"),
        }
    }

    async fn process(self: &Arc<Self>, item: Claimed) {
        let id = item.id;
        let digest = short(&item.feature_digest);
        let features = match parse_redacted(&item.feature_json, item.feature_digest) {
            Ok(f) => f,
            Err(e) => {
                bump(&self.counters.dead_invalid_features);
                warn!(item = id, feature_digest = %digest, reason = %e, "exported features failed re-validation");
                self.settle(id, Verdict::Dead(format!("invalid features: {e}")))
                    .await;
                return;
            }
        };

        // Do not spend a request on an account that has already moved to a newer revision.
        let key = item.key;
        match self.db(move |s| s.reader().current_export(&key)).await {
            Ok(Some((rev, d))) if rev == item.revision && d == item.feature_digest => {}
            Ok(_) => {
                info!(item = id, feature_digest = %digest, "export superseded before the request; skipped");
                self.settle(id, Verdict::Ack("superseded")).await;
                return;
            }
            Err(e) => {
                self.settle(id, Verdict::Retry(format!("snapshot read: {e}")))
                    .await;
                return;
            }
        }

        let req = match build_request(&features, &self.cfg.model) {
            Ok(r) => r,
            Err(e) => {
                bump(&self.counters.dead_invalid_features);
                warn!(item = id, feature_digest = %digest, reason = %e, "request could not be built");
                self.settle(id, Verdict::Dead(format!("request rejected: {e}")))
                    .await;
                return;
            }
        };

        // A separate trace, linked to the execution side only by identifiers (request id and
        // feature digest), never parented on a hot-path span.
        let span = info_span!(
            parent: None,
            "jev_request",
            request_id = %short(&req.key().id()),
            feature_digest = %digest,
            item = id,
            attempt = item.attempts,
        );
        async {
            let remaining_ns = item.lease_until_ns.saturating_sub(now_ns());
            let deadline = Duration::from_nanos(remaining_ns);
            let out = self
                .client
                .call(&req, deadline, &self.inflight_cancel)
                .await;
            let source = format!("{:?}", out.source).to_ascii_lowercase();
            let verdict = match out.result {
                Ok(answer) => {
                    info!(
                        source = %source,
                        http_attempts = out.attempts,
                        elapsed_ms = u64::try_from(out.elapsed.as_millis()).unwrap_or(u64::MAX),
                        "advisory received"
                    );
                    self.apply(&item, &answer).await
                }
                Err(u) => {
                    let reason = u.reason;
                    let detail = u.last_error.as_ref().map(ToString::to_string);
                    info!(
                        fallback = reason.label(),
                        http_attempts = out.attempts,
                        elapsed_ms = u64::try_from(out.elapsed.as_millis()).unwrap_or(u64::MAX),
                        error = detail.as_deref().unwrap_or(""),
                        "no advisory; deterministic baseline continues"
                    );
                    self.verdict_for_fallback(reason, detail)
                }
            };
            self.settle(id, verdict).await;
        }
        .instrument(span)
        .await;
    }

    fn verdict_for_fallback(&self, reason: FallbackReason, detail: Option<String>) -> Verdict {
        let text = match &detail {
            Some(d) => format!("{}: {d}", reason.label()),
            None => reason.label().to_string(),
        };
        match reason {
            FallbackReason::Cancelled => Verdict::Release,
            FallbackReason::CircuitOpen
            | FallbackReason::Misconfigured
            | FallbackReason::RequestBudget
            | FallbackReason::TokenBudget
            | FallbackReason::CostBudget
            | FallbackReason::InflightSaturated => {
                // Systemic: give the row back untouched and stop claiming for a moment.
                self.pause();
                Verdict::Release
            }
            FallbackReason::NonRetryable => {
                bump(&self.counters.dead_non_retryable);
                Verdict::Dead(text)
            }
            FallbackReason::DeadlineExceeded
            | FallbackReason::RetriesExhausted
            | FallbackReason::NegativeCached => Verdict::Retry(text),
        }
    }

    async fn apply(&self, item: &Claimed, a: &AdvisoryAnswer) -> Verdict {
        let now = now_ns();
        let ttl_ns = u64::try_from(self.cfg.ttl.as_nanos()).unwrap_or(u64::MAX);
        let tier = match a.watch_tier.choice {
            jev_advisor::WatchTier::High => StoredTier::High,
            jev_advisor::WatchTier::Normal => StoredTier::Normal,
            jev_advisor::WatchTier::Low => StoredTier::Low,
        };
        let d = AdvisoryDecision {
            key: item.key,
            revision: item.revision,
            feature_digest: item.feature_digest,
            feature_schema: FEATURE_SCHEMA.to_string(),
            policy_version: self.cfg.policy_version.clone(),
            prompt_digest: prompt_digest(),
            model: a.model.clone(),
            watch_tier: tier,
            // The weakest of the two confidence-carrying answers.
            confidence: a.watch_tier.confidence.min(a.monitoring_value.confidence),
            prep_probability: a.extra_preparation,
            value_score: a.monitoring_value.score,
            issued_at_ns: now,
            expires_at_ns: now.saturating_add(ttl_ns),
            input_tokens: Some(a.usage.input_tokens),
            output_tokens: Some(a.usage.output_tokens),
            response_digest: a.response_digest,
            mode: DecisionMode::Shadow,
        };
        let dc = self.decision_cfg.clone();
        match self.db(move |s| s.apply_decision(&d, now, &dc)).await {
            Ok(outcome) => {
                bump(&self.counters.applied);
                info!(outcome = ?outcome, "shadow decision stored");
                Verdict::Ack("applied")
            }
            Err(e) => match classify_apply(&e) {
                Verdict::Ack(why) => {
                    bump(&self.counters.discarded_stale_response);
                    info!(reason = %e, "advisory discarded");
                    Verdict::Ack(why)
                }
                Verdict::Dead(w) => {
                    bump(&self.counters.dead_invalid_response);
                    Verdict::Dead(w)
                }
                v => v,
            },
        }
    }

    async fn log_metrics(&self) {
        let m = self.client.snapshot();
        let q = self.db(|s| s.stats(now_ns())).await.unwrap_or_default();
        let c = self.counters.snapshot();
        let map = |m: &std::collections::BTreeMap<&'static str, u64>| {
            serde_json::to_string(m).unwrap_or_default()
        };
        info!(
            target: "copper_jev_advisor::metrics",
            circuit_state = m.circuit_state,
            circuit_misconfigured = m.circuit_misconfigured,
            inflight = m.inflight,
            requests_total = %map(&m.requests_total),
            fallback_total = %map(&m.fallback_total),
            input_tokens_total = m.input_tokens_total,
            output_tokens_total = m.output_tokens_total,
            estimated_cost_micro_usd_total = m.estimated_cost_micro_usd_total,
            cache_hits_total = m.cache_hits_total,
            coalesced_total = m.coalesced_total,
            window_requests_used = m.window_requests_used,
            queue_pending = q.pending,
            queue_in_flight = q.in_flight,
            queue_done = q.done,
            queue_dead = q.dead,
            queue_missed_total = q.missed_total,
            claimed = c.claimed,
            applied = c.applied,
            superseded = c.superseded,
            discarded_stale_response = c.discarded_stale_response,
            dead_invalid_features = c.dead_invalid_features,
            dead_invalid_response = c.dead_invalid_response,
            dead_non_retryable = c.dead_non_retryable,
            failed_retry = c.failed_retry,
            released = c.released,
            lease_lost = c.lease_lost,
            "metrics"
        );
    }
}
