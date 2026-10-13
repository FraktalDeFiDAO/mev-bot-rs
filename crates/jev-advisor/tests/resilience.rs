//! Behavior of `ResilientClient` over the real `TypeSafeClient` talking to a real local TCP server.
//! Time-exact tests use `ManualClock` (no real timer races real IO); wall-time tests (in-flight
//! cap, outage) use `TokioClock` with short real durations.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::manual::ManualClock;
use common::*;
use jev_advisor::*;
use proptest::prelude::*;

type Rc<C> = ResilientClient<TypeSafeClient, C, SeededRng>;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn ok() -> Script {
    json_ok(&fixture("valid.json"))
}
fn status(code: &str, retry_after: Option<&str>) -> Script {
    let mut h = vec![("Content-Type", "application/json")];
    if let Some(v) = retry_after {
        h.push(("Retry-After", v));
    }
    Script::Raw(http(code, &h, b"{}"))
}
fn s429(ra: &str) -> Script {
    status("429 Too Many Requests", Some(ra))
}
fn s500() -> Script {
    status("500 Internal Server Error", None)
}

/// Deterministic schedule, no cache confusion: jitter off unless a test turns it on.
fn rcfg() -> ResilienceConfig {
    let mut c = ResilienceConfig::default();
    c.retry.jitter_permille = 0;
    c
}

fn build<C: Clock>(server: &TestServer, clock: C, cfg: ResilienceConfig) -> Rc<C> {
    ResilientClient::new(
        TypeSafeClient::new(cfg_for(server)).unwrap(),
        clock,
        SeededRng::new(7),
        cfg,
    )
    .unwrap()
}

fn cfg_for(server: &TestServer) -> ClientConfig {
    let mut c = cfg(server.addr);
    // The transport's own timers are far looser than the layer's, so only the layer decides.
    c.total_timeout = secs(30);
    c.read_timeout = secs(30);
    c
}

fn req_n(i: usize) -> AdvisoryRequest {
    build_request(&features_n(i), MODEL).unwrap()
}

fn hits(s: &TestServer) -> usize {
    s.requests.lock().unwrap().len()
}

async fn wait_hits(s: &TestServer, n: usize) {
    for _ in 0..2000 {
        if hits(s) >= n {
            return;
        }
        tokio::time::sleep(ms(2)).await;
    }
    panic!("server never saw {n} requests");
}

async fn call<C: Clock>(c: &Rc<C>, i: usize, deadline: Duration) -> Outcome {
    c.call(&req_n(i), deadline, &CancellationToken::new()).await
}

fn reason(o: &Outcome) -> FallbackReason {
    o.result.as_ref().unwrap_err().reason
}

// ---------------------------------------------------------------- retry

#[tokio::test]
async fn retries_429_honoring_retry_after() {
    let server = TestServer::spawn(Script::Seq(vec![s429("1"), ok()])).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    let o = clock.drive(call(&c, 0, secs(10))).await;
    assert!(o.result.is_ok());
    assert_eq!(o.attempts, 2);
    assert_eq!(hits(&server), 2);
    // Retry-After (1s) beats the 100ms backoff.
    assert_eq!(clock.requested(SleepPurpose::Backoff), [secs(1)]);
    assert_eq!(o.elapsed, secs(1));
}

#[tokio::test]
async fn retry_after_beyond_the_deadline_gives_up_without_waiting() {
    let server = TestServer::spawn(Script::Seq(vec![s429("10"), ok()])).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    let o = clock.drive(call(&c, 0, secs(2))).await;
    assert_eq!(reason(&o), FallbackReason::DeadlineExceeded);
    assert_eq!(o.attempts, 1);
    assert_eq!(hits(&server), 1);
    assert!(clock.requested(SleepPurpose::Backoff).is_empty());
    assert_eq!(o.elapsed, Duration::ZERO);
}

#[tokio::test]
async fn never_retries_non_retryable_statuses() {
    for code in [
        "401 Unauthorized",
        "403 Forbidden",
        "422 Unprocessable Entity",
        "400 Bad Request",
    ] {
        let server = TestServer::spawn(Script::Seq(vec![status(code, None), ok()])).await;
        let clock = ManualClock::new();
        let c = build(&server, clock.clone(), rcfg());
        let o = clock.drive(call(&c, 0, secs(10))).await;
        assert_eq!(reason(&o), FallbackReason::NonRetryable, "{code}");
        assert_eq!(hits(&server), 1, "{code}");
        assert!(clock.requested(SleepPurpose::Backoff).is_empty(), "{code}");
    }
}

#[tokio::test]
async fn schema_drift_and_wrong_model_are_not_retried() {
    for name in ["schema_drift.json", "wrong_model.json", "malformed.json"] {
        let server = TestServer::spawn(Script::Seq(vec![json_ok(&fixture(name)), ok()])).await;
        let clock = ManualClock::new();
        let c = build(&server, clock.clone(), rcfg());
        let o = clock.drive(call(&c, 0, secs(10))).await;
        assert_eq!(reason(&o), FallbackReason::NonRetryable, "{name}");
        assert_eq!(hits(&server), 1, "{name}");
    }
}

#[tokio::test]
async fn overload_529_then_success() {
    let server = TestServer::spawn(Script::Seq(vec![status("529 Overloaded", None), ok()])).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    let o = clock.drive(call(&c, 0, secs(10))).await;
    let a = o.result.unwrap();
    assert_eq!(a.model, MODEL);
    assert_eq!(o.attempts, 2);
    assert_eq!(clock.requested(SleepPurpose::Backoff), [ms(100)]);
    let m = c.snapshot();
    assert_eq!(m.requests_total["overloaded"], 1);
    assert_eq!(m.requests_total["ok"], 1);
}

#[tokio::test]
async fn exact_backoff_schedule_without_jitter() {
    let server = TestServer::spawn(s500()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.retry.max_attempts = 4;
    cf.breaker.consecutive_failures = 100;
    let c = build(&server, clock.clone(), cf);
    let o = clock.drive(call(&c, 0, secs(60))).await;
    assert_eq!(reason(&o), FallbackReason::RetriesExhausted);
    assert_eq!(o.attempts, 4);
    assert_eq!(hits(&server), 4);
    assert_eq!(
        clock.requested(SleepPurpose::Backoff),
        [ms(100), ms(200), ms(400)]
    );
    assert!(matches!(
        o.result.unwrap_err().last_error,
        Some(TransportError::Retryable { .. })
    ));
}

#[tokio::test]
async fn exact_backoff_schedule_under_a_seeded_rng() {
    let server = TestServer::spawn(s500()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.retry.jitter_permille = 500;
    cf.retry.max_attempts = 4;
    cf.breaker.consecutive_failures = 100;
    let c = build(&server, clock.clone(), cf.clone());
    let o = clock.drive(call(&c, 0, secs(60))).await;
    assert_eq!(o.attempts, 4);
    // The client was seeded with 7: replay that stream through the public formula.
    let mut rng = SeededRng::new(7);
    let expected: Vec<Duration> = (1..=3)
        .map(|n| backoff_delay(&cf.retry, n, rng.next_u64()))
        .collect();
    let got = clock.requested(SleepPurpose::Backoff);
    assert_eq!(got, expected);
    for (d, cap) in got.iter().zip([100u64, 200, 400]) {
        assert!(*d >= ms(cap / 2) && *d <= ms(cap), "{d:?} vs cap {cap}ms");
    }
    // Same seed, same schedule on a fresh client.
    let server2 = TestServer::spawn(s500()).await;
    let clock2 = ManualClock::new();
    let c2 = build(&server2, clock2.clone(), cf);
    clock2.drive(call(&c2, 0, secs(60))).await;
    assert_eq!(clock2.requested(SleepPurpose::Backoff), got);
}

#[tokio::test]
async fn cancellation_mid_backoff_stops_promptly() {
    let server = TestServer::spawn(Script::Seq(vec![s500(), ok()])).await;
    let clock = ManualClock::new();
    let c = Arc::new(build(&server, clock.clone(), rcfg()));
    let cancel = CancellationToken::new();
    let task = {
        let (c, cancel) = (c.clone(), cancel.clone());
        tokio::spawn(async move { c.call(&req_n(0), secs(60), &cancel).await })
    };
    clock.pending(SleepPurpose::Backoff).await; // the call is now asleep between attempts
    cancel.cancel();
    let o = task.await.unwrap();
    assert_eq!(reason(&o), FallbackReason::Cancelled);
    assert_eq!(hits(&server), 1);
    assert_eq!(o.elapsed, Duration::ZERO);
}

// -------------------------------------------------------------- breaker

fn one_attempt() -> ResilienceConfig {
    let mut c = rcfg();
    c.retry.max_attempts = 1;
    c.breaker.consecutive_failures = 3;
    c.breaker.cooldown = secs(30);
    c
}

#[tokio::test]
async fn breaker_closed_open_halfopen_closed_and_open_is_offline() {
    let server = TestServer::spawn(Script::Seq(vec![s500(), s500(), s500(), ok()])).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), one_attempt());
    for i in 0..3 {
        let o = call(&c, i, secs(5)).await;
        assert_eq!(reason(&o), FallbackReason::RetriesExhausted);
    }
    assert_eq!(c.snapshot().circuit_state, 2);
    // Open: any number of calls, zero network.
    for i in 0..20 {
        let o = call(&c, 100 + i, secs(5)).await;
        assert_eq!(reason(&o), FallbackReason::CircuitOpen);
        assert_eq!(o.attempts, 0);
        assert_eq!(o.source, Source::Local);
    }
    assert_eq!(hits(&server), 3);
    clock.advance(secs(29));
    assert_eq!(
        reason(&call(&c, 0, secs(5)).await),
        FallbackReason::CircuitOpen
    );
    assert_eq!(hits(&server), 3);
    clock.advance(secs(1));
    let probe = call(&c, 0, secs(5)).await; // cooldown over: this is the one probe
    assert!(probe.result.is_ok());
    assert_eq!(hits(&server), 4);
    assert_eq!(c.snapshot().circuit_state, 0);
    let m = c.snapshot();
    assert_eq!(m.fallback_total["circuit_open"], 21);
}

#[tokio::test]
async fn failed_probe_reopens_the_breaker() {
    let server = TestServer::spawn(Script::Seq(vec![
        s500(),
        s500(),
        s500(),
        s500(), // probe fails
        ok(),   // second probe succeeds
    ]))
    .await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), one_attempt());
    for i in 0..3 {
        call(&c, i, secs(5)).await;
    }
    clock.advance(secs(30));
    let o = call(&c, 10, secs(5)).await;
    assert_eq!(reason(&o), FallbackReason::RetriesExhausted);
    assert_eq!(hits(&server), 4);
    assert_eq!(c.snapshot().circuit_state, 2);
    // Fresh cooldown from the failed probe: still open 29s later, zero network.
    clock.advance(secs(29));
    assert_eq!(
        reason(&call(&c, 11, secs(5)).await),
        FallbackReason::CircuitOpen
    );
    assert_eq!(hits(&server), 4);
    clock.advance(secs(1));
    assert!(call(&c, 12, secs(5)).await.result.is_ok());
    assert_eq!(c.snapshot().circuit_state, 0);
}

#[tokio::test]
async fn half_open_admits_exactly_one_probe() {
    let server = TestServer::spawn(Script::Seq(vec![
        s500(),
        s500(),
        s500(),
        Script::Delayed(ms(300), Box::new(ok())),
    ]))
    .await;
    let clock = ManualClock::new();
    let c = Arc::new(build(&server, clock.clone(), one_attempt()));
    for i in 0..3 {
        call(&c, i, secs(5)).await;
    }
    clock.advance(secs(30));
    let probe = {
        let c = c.clone();
        tokio::spawn(async move { call(&c, 50, secs(5)).await })
    };
    wait_hits(&server, 4).await;
    assert_eq!(
        c.snapshot().circuit_state,
        1,
        "half-open while the probe is out"
    );
    let other = call(&c, 51, secs(5)).await;
    assert_eq!(reason(&other), FallbackReason::CircuitOpen);
    assert!(probe.await.unwrap().result.is_ok());
    assert_eq!(hits(&server), 4);
    assert_eq!(c.snapshot().circuit_state, 0);
}

#[tokio::test]
async fn auth_failure_is_misconfigured_until_config_changes() {
    let server = TestServer::spawn(Script::Seq(vec![status("401 Unauthorized", None), ok()])).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    assert_eq!(
        reason(&call(&c, 0, secs(5)).await),
        FallbackReason::NonRetryable
    );
    let m = c.snapshot();
    assert!(m.circuit_misconfigured);
    assert_eq!(m.circuit_state, 2);
    clock.advance(secs(86_400)); // no cooldown ever re-probes a bad credential
    let o = call(&c, 1, secs(5)).await;
    assert_eq!(reason(&o), FallbackReason::Misconfigured);
    assert_eq!(hits(&server), 1);
    c.config_changed();
    assert!(call(&c, 2, secs(5)).await.result.is_ok());
    assert_eq!(hits(&server), 2);
    assert!(!c.snapshot().circuit_misconfigured);
}

// ---------------------------------------------------------------- quota

#[tokio::test]
async fn request_budget_resets_at_the_window_boundary() {
    let server = TestServer::spawn(ok()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.quota.requests_per_window = 2;
    let c = build(&server, clock.clone(), cf);
    assert!(call(&c, 0, secs(5)).await.result.is_ok());
    clock.advance(secs(3000));
    assert!(call(&c, 1, secs(5)).await.result.is_ok());
    clock.advance(secs(599)); // t = 3599
    let o = call(&c, 2, secs(5)).await;
    assert_eq!(reason(&o), FallbackReason::RequestBudget);
    assert_eq!(hits(&server), 2);
    clock.advance(secs(1)); // t = 3600: new hour
    assert!(call(&c, 3, secs(5)).await.result.is_ok());
    assert_eq!(hits(&server), 3);
    assert_eq!(c.snapshot().window_requests_used, 1);
}

#[tokio::test]
async fn retries_spend_the_request_budget() {
    let server = TestServer::spawn(s500()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.quota.requests_per_window = 2;
    cf.retry.max_attempts = 5;
    cf.breaker.consecutive_failures = 100;
    let c = build(&server, clock.clone(), cf);
    let o = clock.drive(call(&c, 0, secs(60))).await;
    assert_eq!(reason(&o), FallbackReason::RequestBudget);
    assert_eq!(o.attempts, 2);
    assert_eq!(hits(&server), 2);
}

#[tokio::test]
async fn token_budget_from_reported_usage() {
    // valid.json reports 412 + 31 = 443 tokens per answer.
    let server = TestServer::spawn(ok()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.quota.tokens_per_window = Some(500);
    let c = build(&server, clock.clone(), cf);
    assert!(call(&c, 0, secs(5)).await.result.is_ok()); // 443
    assert!(call(&c, 1, secs(5)).await.result.is_ok()); // 886 >= 500 now
    assert_eq!(
        reason(&call(&c, 2, secs(5)).await),
        FallbackReason::TokenBudget
    );
    assert_eq!(hits(&server), 2);
    let m = c.snapshot();
    assert_eq!(m.input_tokens_total, 824);
    assert_eq!(m.output_tokens_total, 62);
    assert_eq!(m.window_tokens_used, 886);
    clock.advance(secs(3600));
    assert!(call(&c, 3, secs(5)).await.result.is_ok());
}

#[tokio::test]
async fn usd_budget_requires_prices_and_then_binds() {
    let mut bad = rcfg();
    bad.quota.usd_micro_per_window = Some(1);
    assert!(bad.validate().is_err());

    let server = TestServer::spawn(ok()).await;
    let clock = ManualClock::new();
    let mut cf = rcfg();
    cf.quota.prices = Some(TokenPrices {
        input_micro_usd_per_mtok: 3_000_000,
        output_micro_usd_per_mtok: 15_000_000,
    });
    cf.quota.usd_micro_per_window = Some(2000);
    let c = build(&server, clock.clone(), cf);
    assert!(call(&c, 0, secs(5)).await.result.is_ok()); // 1701 micro-USD
    assert!(call(&c, 1, secs(5)).await.result.is_ok()); // 3402 >= 2000
    assert_eq!(
        reason(&call(&c, 2, secs(5)).await),
        FallbackReason::CostBudget
    );
    assert_eq!(c.snapshot().estimated_cost_micro_usd_total, 3402);
}

#[tokio::test]
async fn cost_is_zero_and_unenforced_without_prices() {
    let server = TestServer::spawn(ok()).await;
    let c = build(&server, ManualClock::new(), rcfg());
    for i in 0..5 {
        assert!(call(&c, i, secs(5)).await.result.is_ok());
    }
    assert_eq!(c.snapshot().estimated_cost_micro_usd_total, 0);
}

#[tokio::test]
async fn inflight_cap_fail_fast_falls_back_without_network() {
    let server = TestServer::spawn(Script::Delayed(ms(400), Box::new(ok()))).await;
    let mut cf = rcfg();
    cf.max_inflight = 2;
    cf.inflight_policy = InflightPolicy::FailFast;
    let c = Arc::new(build(&server, TokioClock::new(), cf));
    let busy: Vec<_> = (0..2)
        .map(|i| {
            let c = c.clone();
            tokio::spawn(async move { call(&c, i, secs(5)).await })
        })
        .collect();
    wait_hits(&server, 2).await;
    assert_eq!(c.snapshot().inflight, 2);
    let o = call(&c, 2, secs(5)).await;
    assert_eq!(reason(&o), FallbackReason::InflightSaturated);
    assert_eq!(hits(&server), 2);
    for t in busy {
        assert!(t.await.unwrap().result.is_ok());
    }
    assert_eq!(c.snapshot().inflight, 0);
    assert!(call(&c, 3, secs(5)).await.result.is_ok());
}

#[tokio::test]
async fn inflight_cap_wait_queues_within_the_deadline() {
    let server = TestServer::spawn(Script::Delayed(ms(300), Box::new(ok()))).await;
    let mut cf = rcfg();
    cf.max_inflight = 2;
    let c = Arc::new(build(&server, TokioClock::new(), cf));
    let t0 = std::time::Instant::now();
    let tasks: Vec<_> = (0..3)
        .map(|i| {
            let c = c.clone();
            tokio::spawn(async move { call(&c, i, secs(5)).await })
        })
        .collect();
    for t in tasks {
        assert!(t.await.unwrap().result.is_ok());
    }
    // Two waves: the third call had to wait for a free slot.
    assert!(t0.elapsed() >= ms(580), "{:?}", t0.elapsed());
}

#[tokio::test]
async fn inflight_wait_never_outlasts_the_deadline() {
    let server = TestServer::spawn(Script::Delayed(ms(600), Box::new(ok()))).await;
    let mut cf = rcfg();
    cf.max_inflight = 1;
    let c = Arc::new(build(&server, TokioClock::new(), cf));
    let first = {
        let c = c.clone();
        tokio::spawn(async move { call(&c, 0, secs(5)).await })
    };
    wait_hits(&server, 1).await;
    let o = call(&c, 1, ms(100)).await;
    assert_eq!(reason(&o), FallbackReason::InflightSaturated);
    assert!(o.elapsed <= ms(150), "{:?}", o.elapsed);
    assert!(first.await.unwrap().result.is_ok());
}

// ------------------------------------------------- single-flight, cache

#[tokio::test]
async fn ten_concurrent_identical_requests_make_one_http_call() {
    let server = TestServer::spawn(Script::Delayed(ms(300), Box::new(ok()))).await;
    let clock = ManualClock::new();
    let c = Arc::new(build(&server, clock.clone(), rcfg()));
    let tasks: Vec<_> = (0..10)
        .map(|_| {
            let c = c.clone();
            tokio::spawn(async move { call(&c, 0, secs(5)).await })
        })
        .collect();
    let mut outcomes = Vec::new();
    for t in tasks {
        outcomes.push(t.await.unwrap());
    }
    assert_eq!(hits(&server), 1);
    assert!(outcomes.iter().all(|o| o.result.is_ok()));
    let first = outcomes[0].result.as_ref().unwrap().response_digest;
    assert!(
        outcomes
            .iter()
            .all(|o| o.result.as_ref().unwrap().response_digest == first)
    );
    let net = outcomes
        .iter()
        .filter(|o| o.source == Source::Network)
        .count();
    let co = outcomes
        .iter()
        .filter(|o| o.source == Source::Coalesced)
        .count();
    assert_eq!((net, co), (1, 9));
    assert_eq!(c.snapshot().coalesced_total, 9);
    assert_eq!(c.snapshot().requests_total["ok"], 1);
}

#[tokio::test]
async fn distinct_requests_do_not_coalesce() {
    let server = TestServer::spawn(Script::Delayed(ms(100), Box::new(ok()))).await;
    let c = Arc::new(build(&server, ManualClock::new(), rcfg()));
    let tasks: Vec<_> = (0..3)
        .map(|i| {
            let c = c.clone();
            tokio::spawn(async move { call(&c, i, secs(5)).await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().result.unwrap();
    }
    assert_eq!(hits(&server), 3);
}

#[tokio::test]
async fn cache_hit_within_ttl_miss_after() {
    let server = TestServer::spawn(ok()).await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    let a = call(&c, 0, secs(5)).await;
    assert_eq!(a.source, Source::Network);
    clock.advance(secs(59));
    let b = call(&c, 0, secs(5)).await;
    assert_eq!(b.source, Source::Cache);
    assert_eq!(b.attempts, 0);
    assert_eq!(a.result, b.result);
    assert_eq!(hits(&server), 1);
    clock.advance(secs(1)); // t = 60s: expired
    let d = call(&c, 0, secs(5)).await;
    assert_eq!(d.source, Source::Network);
    assert_eq!(hits(&server), 2);
    assert_eq!(c.snapshot().cache_hits_total, 1);
}

#[tokio::test]
async fn cache_is_bounded_lru() {
    let server = TestServer::spawn(ok()).await;
    let mut cf = rcfg();
    cf.cache.max_entries = 2;
    let c = build(&server, ManualClock::new(), cf);
    for i in 0..3 {
        call(&c, i, secs(5)).await; // 0 evicted when 2 arrives
    }
    assert_eq!(call(&c, 2, secs(5)).await.source, Source::Cache);
    assert_eq!(call(&c, 0, secs(5)).await.source, Source::Network);
}

#[tokio::test]
async fn failures_are_not_cached() {
    let server = TestServer::spawn(Script::Seq(vec![s500(), ok()])).await;
    let mut cf = rcfg();
    cf.retry.max_attempts = 1;
    let c = build(&server, ManualClock::new(), cf);
    assert_eq!(
        reason(&call(&c, 0, secs(5)).await),
        FallbackReason::RetriesExhausted
    );
    assert!(call(&c, 0, secs(5)).await.result.is_ok());
    assert_eq!(hits(&server), 2);
}

#[tokio::test]
async fn schema_failures_get_a_short_negative_cache() {
    let server = TestServer::spawn(Script::Seq(vec![
        json_ok(&fixture("schema_drift.json")),
        ok(),
    ]))
    .await;
    let clock = ManualClock::new();
    let c = build(&server, clock.clone(), rcfg());
    assert_eq!(
        reason(&call(&c, 0, secs(5)).await),
        FallbackReason::NonRetryable
    );
    clock.advance(secs(4));
    let o = call(&c, 0, secs(5)).await;
    assert_eq!(reason(&o), FallbackReason::NegativeCached);
    assert!(matches!(
        o.result.unwrap_err().last_error,
        Some(TransportError::SchemaDrift(_))
    ));
    assert_eq!(hits(&server), 1);
    // A different request is unaffected.
    clock.advance(secs(1)); // 5s: negative entry expired
    assert!(call(&c, 0, secs(5)).await.result.is_ok());
    assert_eq!(hits(&server), 2);
}

// ---------------------------------------------------------------- outage

async fn dead_port() -> TestServer {
    // Bind to learn a free port, then close it: connections are refused.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    TestServer {
        addr,
        requests: Default::default(),
        connections: Default::default(),
        flood_sent: Default::default(),
    }
}

#[tokio::test]
async fn outage_connection_refused_returns_fallbacks_within_the_deadline() {
    let server = dead_port().await;
    let c = Arc::new(build(&server, TokioClock::new(), rcfg()));
    let deadline = ms(400);
    for i in 0..12 {
        let o = call(&c, i, deadline).await;
        assert!(o.result.is_err());
        assert!(o.elapsed <= deadline + ms(50), "{:?}", o.elapsed);
    }
    // After the breaker opened, the rest never even tried.
    let m = c.snapshot();
    assert_eq!(m.circuit_state, 2);
    assert!(m.fallback_total["circuit_open"] > 0);
}

#[tokio::test]
async fn outage_hanging_server_is_cut_by_attempt_timeout_and_deadline() {
    let server = TestServer::spawn(Script::Hang).await;
    let mut cf = rcfg();
    cf.attempt_timeout = ms(100);
    let c = build(&server, TokioClock::new(), cf);
    let deadline = ms(350);
    let o = call(&c, 0, deadline).await;
    assert!(o.result.is_err());
    assert!(o.elapsed <= deadline + ms(50), "{:?}", o.elapsed);
    assert!(o.attempts >= 2, "attempts {}", o.attempts);
    assert!(c.snapshot().requests_total["timeout"] >= 2);
}

#[tokio::test]
async fn deadline_clamped_timeout_does_not_blame_the_service() {
    let server = TestServer::spawn(Script::Hang).await;
    let mut cf = rcfg();
    cf.breaker.consecutive_failures = 1;
    let c = build(&server, TokioClock::new(), cf);
    let o = call(&c, 0, ms(60)).await; // attempt_timeout 1500ms >> deadline
    assert_eq!(reason(&o), FallbackReason::DeadlineExceeded);
    assert_eq!(c.snapshot().circuit_state, 0);
}

#[tokio::test]
async fn zero_deadline_makes_no_network_call() {
    let server = TestServer::spawn(ok()).await;
    let c = build(&server, ManualClock::new(), rcfg());
    let o = call(&c, 0, Duration::ZERO).await;
    assert_eq!(reason(&o), FallbackReason::DeadlineExceeded);
    assert_eq!(hits(&server), 0);
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_config_is_rejected() {
    let mut c = rcfg();
    c.max_inflight = 0;
    assert!(c.validate().is_err());
    let mut c = rcfg();
    c.retry.max_attempts = 0;
    assert!(c.validate().is_err());
    assert!(rcfg().validate().is_ok());
    let d = ResilienceConfig::default();
    assert_eq!(d.attempt_timeout, ms(1500));
    assert_eq!(d.max_inflight, 4);
    assert_eq!(d.quota.requests_per_window, 200);
    assert_eq!(d.cache.ttl, secs(60));
}

// -------------------------------------------------------------- property

fn script_for(kind: u8, ra: u8) -> Script {
    let ra = ra.to_string();
    match kind % 9 {
        0 => ok(),
        1 => s429(&ra),
        2 => status("529 Overloaded", Some(&ra)),
        3 => s500(),
        4 => status("503 Service Unavailable", Some(&ra)),
        5 => status("401 Unauthorized", None),
        6 => status("422 Unprocessable Entity", None),
        7 => json_ok(&fixture("schema_drift.json")),
        _ => status("400 Bad Request", None),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_server_behavior_never_exceeds_the_bounds(
        responses in proptest::collection::vec((0u8..9, 0u8..4), 1..10),
        max_attempts in 1u32..5,
        budget in 1u32..8,
        deadline_ms in 50u64..6000,
        calls in 1usize..4,
        consecutive in 1u32..6,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let server = TestServer::spawn(Script::Seq(
                responses.iter().map(|(k, r)| script_for(*k, *r)).collect(),
            )).await;
            let clock = ManualClock::new();
            let mut cf = rcfg();
            cf.retry.max_attempts = max_attempts;
            cf.retry.jitter_permille = 300;
            cf.quota.requests_per_window = budget;
            cf.breaker.consecutive_failures = consecutive;
            cf.cache.ttl = Duration::ZERO;
            cf.cache.negative_ttl = Duration::ZERO;
            let c = build(&server, clock.clone(), cf);
            let deadline = ms(deadline_ms);
            let mut total_attempts = 0u32;
            for i in 0..calls {
                let o = clock.drive(call(&c, i, deadline)).await;
                prop_assert!(o.attempts <= max_attempts, "attempts {}", o.attempts);
                prop_assert!(o.elapsed <= deadline, "elapsed {:?} > {:?}", o.elapsed, deadline);
                total_attempts += o.attempts;
                if let Err(u) = &o.result {
                    prop_assert!(!u.reason.label().is_empty());
                }
            }
            prop_assert_eq!(hits(&server) as u32, total_attempts);
            prop_assert!(total_attempts <= budget, "{} > budget {}", total_attempts, budget);
            prop_assert!(total_attempts <= calls as u32 * max_attempts);
            Ok(())
        })?;
    }
}
