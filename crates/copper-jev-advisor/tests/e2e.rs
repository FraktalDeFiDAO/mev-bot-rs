//! End-to-end tests of the worker library entry point against real temp SQLite files and a real
//! local TCP server. Nothing is mocked: export -> claim -> HTTP -> apply_decision(shadow) -> ack.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use copper_jev_advisor::{CountersSnapshot, Worker, WorkerError, check};
use jev_advisor::CancellationToken;
use tracing::Instrument;

type Handle = tokio::task::JoinHandle<Result<CountersSnapshot, WorkerError>>;

fn start(w: &Arc<Worker>) -> (CancellationToken, Handle) {
    let token = CancellationToken::new();
    let (w, t) = (w.clone(), token.clone());
    // Run under an unrelated "hot_path" span: request spans must still be separate traces.
    let h = tokio::spawn(async move { w.run(t).await }.instrument(tracing::info_span!("hot_path")));
    (token, h)
}

fn done(w: &Worker) -> u64 {
    w.queue_stats().unwrap().done
}

fn decision_rows(env: &Env) -> Vec<(String, String)> {
    let c = rusqlite::Connection::open(env.advisor_path()).unwrap();
    let mut s = c
        .prepare("SELECT decision_mode, watch_tier FROM advisory_decision")
        .unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn items_flow_export_to_shadow_decision_to_ack() {
    init_logs();
    let env = Env::new(5);
    let server = TestServer::spawn(ok()).await;
    let w = Worker::new(env.config(), client_cfg(server.addr)).unwrap();
    let (stop, h) = start(&w);

    wait_until("5 rows acked", Duration::from_secs(15), || done(&w) == 5).await;
    // Let a metrics line be emitted.
    tokio::time::sleep(Duration::from_millis(250)).await;
    stop.cancel();
    let c = h.await.unwrap().unwrap();

    assert_eq!(c.applied, 5);
    assert_eq!(
        (c.dead_invalid_features, c.failed_retry, c.lease_lost),
        (0, 0, 0)
    );
    let q = w.queue_stats().unwrap();
    assert_eq!((q.done, q.dead, q.pending, q.in_flight), (5, 0, 0, 0));
    for n in 1..=5 {
        assert_eq!(w.store().decision_count(&account(n)).unwrap(), 1);
    }
    let rows = decision_rows(&env);
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|(m, t)| m == "shadow" && t == "high"));

    // The wire: one POST per item with the bearer key and a body without account material.
    let reqs = server.recorded();
    assert_eq!(reqs.len(), 5);
    for r in &reqs {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        let body = String::from_utf8(r.body.clone()).unwrap();
        assert!(longest_hex_run(&body) < 40, "address-like hex on the wire");
        assert!(body.contains(MODEL));
    }

    // Metrics line: structured JSON with queue stats and Jev counters.
    let l = logs();
    let metrics = l
        .lines()
        .filter(|l| l.contains("\"message\":\"metrics\""))
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["fields"]["queue_done"] == 5 && v["fields"]["applied"] == 5)
        .expect("a metrics log line showing five applied decisions");
    let f = &metrics["fields"];
    assert_eq!(f["circuit_state"], 0);
    assert!(f["requests_total"].as_str().unwrap().contains("\"ok\":5"));

    // Each request ran in its own root span (separate trace) with request id + feature digest.
    let sp: Vec<_> = spans()
        .into_iter()
        .filter(|(n, _)| n == "jev_request")
        .collect();
    assert!(sp.len() >= 5);
    assert!(
        sp.iter().all(|(_, root)| *root),
        "jev_request must be a trace root"
    );
    let req_line = l
        .lines()
        .find(|l| l.contains("shadow decision stored"))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(req_line).unwrap();
    assert_eq!(v["span"]["name"], "jev_request");
    assert_eq!(v["span"]["request_id"].as_str().unwrap().len(), 16);
    assert_eq!(v["span"]["feature_digest"].as_str().unwrap().len(), 16);
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rate_limit_trips_the_breaker_and_the_worker_recovers() {
    init_logs();
    let env = Env::new(3);
    // Four 429s, then healthy.
    let server = TestServer::spawn(Script::Seq(vec![
        too_many(),
        too_many(),
        too_many(),
        too_many(),
        ok(),
    ]))
    .await;
    let mut cfg = env.config();
    cfg.max_attempts = 20;
    cfg.resilience.max_inflight = 3;
    cfg.resilience.breaker.consecutive_failures = 2;
    cfg.resilience.breaker.cooldown = Duration::from_millis(150);
    let w = Worker::new(cfg, client_cfg(server.addr)).unwrap();
    let (stop, h) = start(&w);

    wait_until("all rows eventually acked", Duration::from_secs(20), || {
        done(&w) == 3
    })
    .await;
    assert!(!h.is_finished(), "the worker must survive an outage");
    stop.cancel();
    let c = h.await.unwrap().unwrap();
    let m = w.jev_metrics();
    assert!(m.requests_total.get("rate_limited").copied().unwrap_or(0) >= 2);
    assert!(
        m.fallback_total.get("circuit_open").copied().unwrap_or(0) >= 1,
        "breaker never fell back: {m:?}"
    );
    assert_eq!(c.applied, 3);
    assert_eq!(w.queue_stats().unwrap().dead, 0);
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreachable_api_degrades_without_crashing_and_keeps_rows() {
    init_logs();
    let env = Env::new(3);
    let mut cfg = env.config();
    cfg.max_attempts = 100; // do not dead-letter during the observation window
    cfg.resilience.breaker.consecutive_failures = 2;
    cfg.resilience.breaker.cooldown = Duration::from_millis(100);
    let w = Worker::new(cfg, client_cfg(closed_port_addr())).unwrap();
    let (stop, h) = start(&w);

    wait_until("breaker fallback observed", Duration::from_secs(15), || {
        let m = w.jev_metrics();
        m.requests_total
            .get("connection_error")
            .copied()
            .unwrap_or(0)
            >= 2
            && m.fallback_total.values().sum::<u64>() >= 2
    })
    .await;
    assert!(!h.is_finished());
    let q = w.queue_stats().unwrap();
    assert_eq!((q.done, q.dead), (0, 0));
    assert_eq!(
        q.pending, 3,
        "advisory work stays queued; baseline is unaffected"
    );
    stop.cancel();
    let c = h.await.unwrap().unwrap();
    assert_eq!(c.applied, 0);
    assert_eq!(
        w.queue_stats().unwrap().in_flight,
        0,
        "leases released on exit"
    );
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_mid_flight_releases_leases_and_restart_finishes_exactly_once() {
    init_logs();
    let env = Env::new(6);
    // First run: every answer takes 800 ms and the grace period is tiny.
    let slow = TestServer::spawn(Script::Delayed(Duration::from_millis(800), Box::new(ok()))).await;
    let mut cfg = env.config();
    cfg.shutdown_grace = Duration::from_millis(50);
    let w1 = Worker::new(cfg, client_cfg(slow.addr)).unwrap();
    let (stop, h) = start(&w1);
    wait_until("two requests in flight", Duration::from_secs(10), || {
        slow.hits() >= 2
    })
    .await;
    let t0 = std::time::Instant::now();
    stop.cancel();
    let c1 = h.await.unwrap().unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "shutdown must be bounded"
    );
    let q = w1.queue_stats().unwrap();
    assert_eq!(q.in_flight, 0, "no lease may survive a graceful shutdown");
    assert_eq!(q.dead, 0);
    assert!(c1.released >= 2, "in-flight rows were released: {c1:?}");
    let done_before = q.done;
    assert!(done_before < 6);
    drop(w1);

    // Restart against a healthy server: the remaining rows are processed, each exactly once.
    let fast = TestServer::spawn(ok()).await;
    let w2 = Worker::new(env.config(), client_cfg(fast.addr)).unwrap();
    let (stop, h) = start(&w2);
    wait_until(
        "all rows acked after restart",
        Duration::from_secs(15),
        || done(&w2) == 6,
    )
    .await;
    stop.cancel();
    let c2 = h.await.unwrap().unwrap();
    assert_eq!(c2.lease_lost, 0);
    assert_eq!(c2.applied + done_before, 6);
    assert_eq!(
        fast.hits() as u64,
        c2.applied,
        "no row was sent twice after restart"
    );
    for n in 1..=6 {
        assert_eq!(
            w2.store().decision_count(&account(n)).unwrap(),
            1,
            "account {n} has exactly one decision"
        );
    }
    let q = w2.queue_stats().unwrap();
    assert_eq!((q.done, q.dead, q.pending, q.in_flight), (6, 0, 0, 0));
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_without_shutdown_is_replayed_after_lease_expiry() {
    init_logs();
    let env = Env::new(2);
    // A "crashed" worker: claims and never settles (lease of 1 ms already elapsed by restart).
    let queue = mev_liq_state::QueueConfig {
        max_pending: 1000,
        max_in_flight: 10,
        max_attempts: 5,
        base_backoff_ns: 1,
        max_backoff_ns: 1,
    };
    let store = mev_liq_state::AdvisorStore::open(&env.advisor_path(), &env.snapshot_path(), queue)
        .unwrap();
    store
        .pull(copper_jev_advisor::worker::now_ns(), 100)
        .unwrap();
    let claimed = store
        .claim(
            "crashed-worker",
            copper_jev_advisor::worker::now_ns(),
            1_000_000,
            2,
        )
        .unwrap();
    assert_eq!(claimed.len(), 2);
    drop(store);
    tokio::time::sleep(Duration::from_millis(5)).await;

    let server = TestServer::spawn(ok()).await;
    let w = Worker::new(env.config(), client_cfg(server.addr)).unwrap();
    let (stop, h) = start(&w);
    wait_until("replayed rows acked", Duration::from_secs(10), || {
        done(&w) == 2
    })
    .await;
    stop.cancel();
    h.await.unwrap().unwrap();
    assert_eq!(server.hits(), 2);
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampered_export_is_dead_lettered_and_never_sent() {
    init_logs();
    let env = Env::new(1);
    // A well-formed JSON object with a value outside the closed vocabulary, exported with its
    // own digest so only the worker's re-validation can catch it.
    let good = features_n(9);
    let bad = good.json().replace("aave_v3", "mystery_protocol");
    env.export_raw(2, 11, 0, &bad, alloy::primitives::keccak256(bad.as_bytes()));
    let server = TestServer::spawn(ok()).await;
    let w = Worker::new(env.config(), client_cfg(server.addr)).unwrap();
    let (stop, h) = start(&w);
    wait_until("one acked one dead", Duration::from_secs(10), || {
        let q = w.queue_stats().unwrap();
        q.done == 1 && q.dead == 1
    })
    .await;
    stop.cancel();
    let c = h.await.unwrap().unwrap();
    assert_eq!(c.dead_invalid_features, 1);
    assert_eq!(
        server.hits(),
        1,
        "the invalid row never reached the network"
    );
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_export_is_skipped_without_a_request() {
    init_logs();
    let env = Env::new(1);
    // Account 1 moves to revision 2 with new features before the worker sees the old row.
    env.export(1, 20, 1, &features_n(33));
    let server = TestServer::spawn(ok()).await;
    let w = Worker::new(env.config(), client_cfg(server.addr)).unwrap();
    let (stop, h) = start(&w);
    wait_until("both rows consumed", Duration::from_secs(10), || {
        done(&w) == 2
    })
    .await;
    stop.cancel();
    let c = h.await.unwrap().unwrap();
    assert_eq!((c.superseded, c.applied), (1, 1));
    assert_eq!(server.hits(), 1);
    assert_eq!(w.store().decision_count(&account(1)).unwrap(), 1);
    assert_logs_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_check_needs_both_dbs_and_ignores_the_api() {
    let env = Env::new(1);
    let cfg = env.config();
    let r = check(&cfg).unwrap();
    assert_eq!(r.snapshot_schema, mev_liq_state::SCHEMA_VERSION);
    assert_eq!(r.advisor_schema, mev_liq_state::ADVISOR_SCHEMA_VERSION);

    // The configured endpoint is unreachable from this test; readiness must not care.
    let mut missing = cfg.clone();
    missing.snapshot_db = env.dir.path().join("nope.db");
    let e = check(&missing).unwrap_err().to_string();
    assert!(e.contains("snapshot database"), "{e}");

    let mut unwritable = cfg.clone();
    unwritable.advisor_db = env.dir.path().join("no/such/dir/advisor.db");
    assert!(
        check(&unwritable)
            .unwrap_err()
            .to_string()
            .contains("advisor database")
    );
}
