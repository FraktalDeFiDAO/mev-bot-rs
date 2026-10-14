//! [`ResilientClient`]: the policy layer over a one-attempt [`Transport`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use alloy::primitives::B256;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;

use super::backoff::{RetryConfig, backoff_delay};
use super::breaker::{Admission, Breaker, BreakerConfig};
use super::cache::TtlLru;
use super::clock::{Clock, JitterSource, SleepPurpose};
use super::metrics::{MetricsInner, MetricsSnapshot, result_label};
use super::quota::{Quota, QuotaConfig};
use super::{ConfigError, FallbackReason, MAX_CONFIG_DURATION, Unavailable};
use crate::client::Transport;
use crate::error::{ErrorClass, TransportError};
use crate::request::AdvisoryRequest;
use crate::response::AdvisoryAnswer;

/// What to do when `max_inflight` requests are already on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflightPolicy {
    /// Wait for a slot, but never past the caller's deadline (then `InflightSaturated`).
    Wait,
    /// Return `InflightSaturated` immediately.
    FailFast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    /// `advisor_ttl_seconds` (spec 60). Zero disables caching.
    pub ttl: Duration,
    pub max_entries: usize,
    /// How long a schema-class failure (schema drift, wrong model, not JSON, malformed JSON,
    /// oversized) is remembered per request key. Zero disables the negative cache. No other
    /// failure is ever cached.
    pub negative_ttl: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(60),
            max_entries: 1024,
            negative_ttl: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResilienceConfig {
    /// `jev_timeout_ms` (spec 1500): the per-attempt timeout, always clamped to the time left
    /// before the caller's deadline.
    pub attempt_timeout: Duration,
    /// `jev_max_inflight` (spec 4).
    pub max_inflight: usize,
    pub inflight_policy: InflightPolicy,
    pub retry: RetryConfig,
    pub breaker: BreakerConfig,
    /// `jev_budget_per_hour` (spec 200) and the optional token / USD budgets.
    pub quota: QuotaConfig,
    pub cache: CacheConfig,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            attempt_timeout: Duration::from_millis(1500),
            max_inflight: 4,
            inflight_policy: InflightPolicy::Wait,
            retry: RetryConfig::default(),
            breaker: BreakerConfig::default(),
            quota: QuotaConfig::default(),
            cache: CacheConfig::default(),
        }
    }
}

impl ResilienceConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m| Err(ConfigError(m));
        if self.attempt_timeout.is_zero() || self.attempt_timeout > MAX_CONFIG_DURATION {
            return bad("attempt_timeout must be in (0, 1 day]");
        }
        if self.max_inflight == 0 || self.max_inflight > Semaphore::MAX_PERMITS {
            return bad("max_inflight must be at least 1");
        }
        if self.retry.max_attempts == 0 {
            return bad("retry.max_attempts must be at least 1");
        }
        if self.retry.jitter_permille > 1000 {
            return bad("retry.jitter_permille must be <= 1000");
        }
        if self.retry.base_delay > self.retry.max_delay
            || self.retry.max_delay > MAX_CONFIG_DURATION
        {
            return bad("retry delays must satisfy base <= max <= 1 day");
        }
        if self.breaker.consecutive_failures == 0 {
            return bad("breaker.consecutive_failures must be at least 1");
        }
        if self.breaker.cooldown > MAX_CONFIG_DURATION {
            return bad("breaker.cooldown must be <= 1 day");
        }
        if let Some(r) = &self.breaker.ratio
            && (r.failure_permille == 0 || r.failure_permille > 1000 || r.window.is_zero())
        {
            return bad("breaker.ratio needs 1..=1000 permille and a non-zero window");
        }
        if self.quota.window.is_zero() || self.quota.window > MAX_CONFIG_DURATION {
            return bad("quota.window must be in (0, 1 day]");
        }
        if self.quota.usd_micro_per_window.is_some() && self.quota.prices.is_none() {
            return bad("a USD budget needs configured token prices (none are assumed)");
        }
        Ok(())
    }
}

/// Where a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// This call made `attempts` HTTP requests.
    Network,
    Cache,
    /// Joined another caller's in-flight identical request.
    Coalesced,
    /// Decided locally with no network (breaker, quota, negative cache, deadline).
    Local,
}

/// The typed result of one [`ResilientClient::call`].
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub result: Result<Arc<AdvisoryAnswer>, Unavailable>,
    pub source: Source,
    /// HTTP requests this call made (0 for cache hits, coalesced followers and local fallbacks).
    pub attempts: u32,
    /// Time on the injected clock from entry to return. Never exceeds the deadline.
    pub elapsed: Duration,
}

type Settled = Result<Arc<AdvisoryAnswer>, Unavailable>;
type Flight = Arc<watch::Sender<Option<Settled>>>;

struct State {
    breaker: Breaker,
    quota: Quota,
    answers: TtlLru<B256, Arc<AdvisoryAnswer>>,
    negatives: TtlLru<B256, TransportError>,
    flights: HashMap<B256, Flight>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes the in-flight entry when the leader finishes or is dropped mid-call; followers whose
/// sender vanished without a value simply re-evaluate.
struct LeaderGuard<'a> {
    state: &'a Mutex<State>,
    key: B256,
    tx: Flight,
}

impl Drop for LeaderGuard<'_> {
    fn drop(&mut self) {
        let mut st = lock(self.state);
        if st
            .flights
            .get(&self.key)
            .is_some_and(|f| Arc::ptr_eq(f, &self.tx))
        {
            st.flights.remove(&self.key);
        }
    }
}

enum Role<'a> {
    Cached(Arc<AdvisoryAnswer>),
    Negative(TransportError),
    Lead(LeaderGuard<'a>),
    Follow(watch::Receiver<Option<Settled>>),
}

enum Followed {
    Settled(Settled),
    LeaderGone,
    DeadlineHit,
    Cancelled,
}

pub struct ResilientClient<T, C, J> {
    transport: T,
    clock: C,
    rng: Mutex<J>,
    cfg: ResilienceConfig,
    state: Mutex<State>,
    metrics: Mutex<MetricsInner>,
    slots: Arc<Semaphore>,
}

impl<T: Transport, C: Clock, J: JitterSource> ResilientClient<T, C, J> {
    pub fn new(transport: T, clock: C, rng: J, cfg: ResilienceConfig) -> Result<Self, ConfigError> {
        cfg.validate()?;
        let state = State {
            breaker: Breaker::new(cfg.breaker.clone()),
            quota: Quota::new(cfg.quota.clone()),
            answers: TtlLru::new(cfg.cache.max_entries, cfg.cache.ttl),
            negatives: TtlLru::new(cfg.cache.max_entries, cfg.cache.negative_ttl),
            flights: HashMap::new(),
        };
        Ok(Self {
            transport,
            clock,
            rng: Mutex::new(rng),
            slots: Arc::new(Semaphore::new(cfg.max_inflight)),
            cfg,
            state: Mutex::new(state),
            metrics: Mutex::new(MetricsInner::default()),
        })
    }

    pub fn config(&self) -> &ResilienceConfig {
        &self.cfg
    }

    /// The operator corrected the key/endpoint: leave the `Misconfigured` state. Deliberately
    /// manual; a bad credential is never retried by probing.
    pub fn config_changed(&self) {
        lock(&self.state).breaker.config_changed();
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let (state, req, tok, usd) = {
            let st = lock(&self.state);
            (
                st.breaker.state(),
                st.quota.requests_used(),
                st.quota.tokens_used(),
                st.quota.micro_usd_used(),
            )
        };
        let inflight = self.cfg.max_inflight - self.slots.available_permits();
        lock(&self.metrics).snapshot(state, inflight, req, tok, usd)
    }

    /// Ask for an advisory for `req`, giving up (typed fallback) no later than `deadline` after
    /// entry on the injected clock. `deadline` is the work item's remaining TTL.
    pub async fn call(
        &self,
        req: &AdvisoryRequest,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Outcome {
        let start = self.clock.now();
        let abs = start.saturating_add(deadline);
        let (result, source, attempts) = self.call_inner(req, abs, cancel).await;
        if let Err(u) = &result {
            lock(&self.metrics).fallback(u.reason);
        }
        Outcome {
            result,
            source,
            attempts,
            elapsed: self.clock.now().saturating_sub(start),
        }
    }

    async fn call_inner(
        &self,
        req: &AdvisoryRequest,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> (Settled, Source, u32) {
        let key = req.key().id();
        loop {
            if cancel.is_cancelled() {
                return local(FallbackReason::Cancelled);
            }
            let now = self.clock.now();
            if now >= deadline {
                return local(FallbackReason::DeadlineExceeded);
            }
            match self.role(key, now) {
                Role::Cached(a) => {
                    lock(&self.metrics).cache_hit();
                    return (Ok(a), Source::Cache, 0);
                }
                Role::Negative(e) => {
                    return (
                        Err(Unavailable {
                            reason: FallbackReason::NegativeCached,
                            last_error: Some(e),
                        }),
                        Source::Local,
                        0,
                    );
                }
                Role::Lead(guard) => {
                    let (res, attempts) = self.lead(req, key, deadline, cancel).await;
                    guard.tx.send_replace(Some(res.clone()));
                    drop(guard);
                    let source = if attempts > 0 || res.is_ok() {
                        Source::Network
                    } else {
                        Source::Local
                    };
                    return (res, source, attempts);
                }
                Role::Follow(rx) => {
                    lock(&self.metrics).coalesced();
                    match self.follow(rx, deadline, cancel).await {
                        Followed::Cancelled => return local(FallbackReason::Cancelled),
                        Followed::DeadlineHit => return local(FallbackReason::DeadlineExceeded),
                        Followed::LeaderGone => {}
                        Followed::Settled(Err(u))
                            if matches!(
                                u.reason,
                                FallbackReason::Cancelled | FallbackReason::DeadlineExceeded
                            ) => {}
                        Followed::Settled(r) => return (r, Source::Coalesced, 0),
                    }
                }
            }
        }
    }

    fn role(&self, key: B256, now: Duration) -> Role<'_> {
        let mut st = lock(&self.state);
        if let Some(a) = st.answers.get(now, &key) {
            return Role::Cached(a);
        }
        if let Some(e) = st.negatives.get(now, &key) {
            return Role::Negative(e);
        }
        if let Some(f) = st.flights.get(&key) {
            return Role::Follow(f.subscribe());
        }
        let (tx, _rx) = watch::channel(None);
        let tx: Flight = Arc::new(tx);
        st.flights.insert(key, tx.clone());
        Role::Lead(LeaderGuard {
            state: &self.state,
            key,
            tx,
        })
    }

    async fn follow(
        &self,
        mut rx: watch::Receiver<Option<Settled>>,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Followed {
        loop {
            if let Some(v) = rx.borrow_and_update().clone() {
                return Followed::Settled(v);
            }
            let remaining = deadline.saturating_sub(self.clock.now());
            if remaining.is_zero() {
                return Followed::DeadlineHit;
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Followed::Cancelled,
                r = rx.changed() => {
                    if r.is_err() {
                        return match rx.borrow().clone() {
                            Some(v) => Followed::Settled(v),
                            None => Followed::LeaderGone,
                        };
                    }
                }
                () = self.clock.sleep(remaining, SleepPurpose::DeadlineWait) => {
                    return Followed::DeadlineHit;
                }
            }
        }
    }

    async fn slot(
        &self,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, FallbackReason> {
        match self.cfg.inflight_policy {
            InflightPolicy::FailFast => self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| FallbackReason::InflightSaturated),
            InflightPolicy::Wait => {
                let remaining = deadline.saturating_sub(self.clock.now());
                if remaining.is_zero() {
                    return Err(FallbackReason::DeadlineExceeded);
                }
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(FallbackReason::Cancelled),
                    p = self.slots.clone().acquire_owned() => {
                        p.map_err(|_| FallbackReason::InflightSaturated)
                    }
                    () = self.clock.sleep(remaining, SleepPurpose::DeadlineWait) => {
                        Err(FallbackReason::InflightSaturated)
                    }
                }
            }
        }
    }

    /// The attempt loop of the single-flight leader. Returns the settled result and the number of
    /// HTTP requests made.
    async fn lead(
        &self,
        req: &AdvisoryRequest,
        key: B256,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> (Settled, u32) {
        let mut attempts: u32 = 0;
        let mut last_error: Option<TransportError> = None;
        macro_rules! fail {
            ($reason:expr) => {
                return (
                    Err(Unavailable {
                        reason: $reason,
                        last_error: last_error.clone(),
                    }),
                    attempts,
                )
            };
        }
        loop {
            if cancel.is_cancelled() {
                fail!(FallbackReason::Cancelled);
            }
            if self.clock.now() >= deadline {
                fail!(FallbackReason::DeadlineExceeded);
            }
            let permit = match self.slot(deadline, cancel).await {
                Ok(p) => p,
                Err(r) => fail!(r),
            };
            let now = self.clock.now();
            if now >= deadline {
                fail!(FallbackReason::DeadlineExceeded);
            }
            let probe = {
                let mut st = lock(&self.state);
                let probe = match st.breaker.admit(now) {
                    Admission::Rejected(r) => fail!(r),
                    Admission::Allowed => false,
                    Admission::Probe => true,
                };
                if let Err(r) = st.quota.reserve(now) {
                    st.breaker.release(probe);
                    fail!(r);
                }
                probe
            };

            attempts += 1;
            let remaining = deadline - now;
            let clamped = remaining < self.cfg.attempt_timeout;
            let timeout = self.cfg.attempt_timeout.min(remaining);
            let sent = self.transport.send(req, cancel);
            let res = tokio::select! {
                biased;
                () = cancel.cancelled() => Err(TransportError::Cancelled),
                r = sent => r,
                () = self.clock.sleep(timeout, SleepPurpose::AttemptTimeout) => {
                    Err(TransportError::Timeout(crate::error::TimeoutKind::Deadline))
                }
            };
            drop(permit);
            let sent_at = now;
            let now = self.clock.now();
            let took = now.saturating_sub(sent_at);
            let label = result_label(&res.as_ref().map(|_| ()));
            lock(&self.metrics).attempt(label, took);

            match res {
                Ok(answer) => {
                    let answer = Arc::new(answer);
                    let cost = {
                        let mut st = lock(&self.state);
                        st.breaker.on_reachable(now, probe);
                        let cost = st.quota.record_usage(now, &answer.usage);
                        st.answers.insert(now, key, answer.clone());
                        cost
                    };
                    lock(&self.metrics).usage(
                        answer.usage.input_tokens,
                        answer.usage.output_tokens,
                        cost,
                    );
                    return (Ok(answer), attempts);
                }
                Err(e) => {
                    last_error = Some(e.clone());
                    match e.class() {
                        ErrorClass::Cancelled => {
                            lock(&self.state).breaker.release(probe);
                            fail!(FallbackReason::Cancelled);
                        }
                        ErrorClass::NonRetryable => {
                            let schema = matches!(
                                e,
                                TransportError::SchemaDrift(_)
                                    | TransportError::WrongModel { .. }
                                    | TransportError::NotJson
                                    | TransportError::MalformedJson
                                    | TransportError::Oversized { .. }
                            );
                            {
                                let mut st = lock(&self.state);
                                match e {
                                    TransportError::Auth(_) => st.breaker.on_auth_failure(),
                                    TransportError::Config(_) => st.breaker.release(probe),
                                    _ => st.breaker.on_reachable(now, probe),
                                }
                                if schema {
                                    st.negatives.insert(now, key, e.clone());
                                }
                            }
                            fail!(FallbackReason::NonRetryable);
                        }
                        ErrorClass::Retryable => {
                            let our_timeout = matches!(e, TransportError::Timeout(_)) && clamped;
                            {
                                let mut st = lock(&self.state);
                                if our_timeout {
                                    // The caller's deadline cut the attempt short; that says
                                    // nothing about the service.
                                    st.breaker.release(probe);
                                } else {
                                    st.breaker.on_failure(now, probe);
                                }
                            }
                            if our_timeout {
                                fail!(FallbackReason::DeadlineExceeded);
                            }
                            if attempts >= self.cfg.retry.max_attempts {
                                fail!(FallbackReason::RetriesExhausted);
                            }
                            let r = lock(&self.rng).next_u64();
                            let delay = backoff_delay(&self.cfg.retry, attempts, r)
                                .max(e.retry_after().unwrap_or_default());
                            let left = deadline.saturating_sub(now);
                            if delay >= left {
                                fail!(FallbackReason::DeadlineExceeded);
                            }
                            tokio::select! {
                                biased;
                                () = cancel.cancelled() => fail!(FallbackReason::Cancelled),
                                () = self.clock.sleep(delay, SleepPurpose::Backoff) => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn local(reason: FallbackReason) -> (Settled, Source, u32) {
    (
        Err(Unavailable {
            reason,
            last_error: None,
        }),
        Source::Local,
        0,
    )
}
