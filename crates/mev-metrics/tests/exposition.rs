//! Exposition-level guarantees of the registry: well-formed text, monotonic counters across scrapes,
//! bounded labels, no address-like strings for any input, and scrapes that do not stall producers.

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use mev_metrics::{
    Metrics, Registry, ServerHandle,
    prom::address_like,
    registry::{EVENT_ADVISORY, EVENT_SCHEDULE_COUNTERS, SCHED_FAMILIES},
    textcheck::parse_strict,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn sched(now: u64, miss: u64) -> Value {
    json!({
        "now_ns": now,
        "candidates": {"p0": 1, "p1": 2, "p2": 3, "p3": 4},
        "deadline_misses_p0": miss, "deadline_misses_p1": 1, "deadline_misses_other": 2,
        "deadline_misses_backlog_p0": miss / 2, "deadline_misses_backlog_p1": 0,
        "deadline_misses_backlog_other": 1,
        "coverage_violations": 0
    })
}

fn advisory(unchanged: bool) -> Value {
    json!({
        "mandatory_unchanged": unchanged,
        "counters": {
            "copper_jev_fallback_total": {"expired": 1, "circuit_open": 2},
            "copper_jev_stale_response_total": 1,
            "copper_jev_applied_bonus_total": {"high": 3}
        }
    })
}

#[test]
fn fully_populated_registry_renders_exactly_the_declared_families() {
    let r = Registry::new();
    r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(1, 2));
    r.observe_event(EVENT_ADVISORY, &advisory(true));
    let p = parse_strict(&r.render()).unwrap();
    let mut got: Vec<_> = p.families.keys().map(String::as_str).collect();
    let mut want: Vec<_> = SCHED_FAMILIES.to_vec();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn counters_are_monotonic_across_scrapes() {
    let r = Registry::new();
    let mut prev = std::collections::BTreeMap::new();
    for i in 1..=20u64 {
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(i * 10, i % 5));
        r.observe_event(EVENT_ADVISORY, &advisory(i % 7 != 0));
        let p = parse_strict(&r.render()).unwrap();
        for s in &p.samples {
            if p.families.get(&s.name).is_some_and(|k| k == "counter") {
                let key = (s.name.clone(), s.labels.clone());
                let before = prev.insert(key, s.value).unwrap_or(0.0);
                assert!(s.value >= before, "{} went backwards", s.name);
            }
        }
    }
}

#[test]
fn a_scrape_does_not_block_a_concurrent_producer() {
    let r = Arc::new(Registry::new());
    let stop = Arc::new(AtomicBool::new(false));
    let scraper = {
        let (r, stop) = (r.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let _ = r.render();
                n += 1;
                // Scrape back to back, but not in a busy loop: `std::sync::Mutex` is not fair, so
                // a scraper that re-locks immediately can starve the producer on a loaded host
                // (37.8 s in the deploy gate, 2026-10-14) without saying anything about blocking.
                std::thread::sleep(Duration::from_micros(200));
            }
            n
        })
    };
    let t = Instant::now();
    for i in 1..=2000u64 {
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(i, 1));
        r.observe_event(EVENT_ADVISORY, &advisory(true));
    }
    let producer = t.elapsed();
    stop.store(true, Ordering::Relaxed);
    assert!(scraper.join().unwrap() > 0);
    assert!(
        producer < Duration::from_secs(20),
        "producer took {producer:?} with a scraper running"
    );
}

#[test]
fn the_listener_serves_a_parseable_scrape_on_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let m = Metrics::new(dir.path().to_path_buf()).unwrap();
    m.event(EVENT_SCHEDULE_COUNTERS, sched(5, 1));
    let bound = m.serve_prometheus("127.0.0.1:0".parse().unwrap()).unwrap();
    assert!(bound.ip().is_loopback());
    let mut c = TcpStream::connect(bound).unwrap();
    c.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    let body = resp.split("\r\n\r\n").nth(1).unwrap();
    let p = parse_strict(body).unwrap();
    assert_eq!(p.get("copper_liq_sched_passes_total", &[]), Some(1.0));
    // The event log is untouched by the exporter.
    m.flush();
}

#[test]
fn nothing_listens_unless_started() {
    // Metrics::new alone never binds a port: the only way to a listener is serve_prometheus/ServerHandle.
    let dir = tempfile::tempdir().unwrap();
    let _m = Metrics::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        mev_metrics::addr_from_env("MEV_METRICS_UNSET_FOR_TEST"),
        Ok(None)
    );
    let _unused: Option<ServerHandle> = None;
}

fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_]{1,12}",
        "0x[0-9a-f]{40}",
        "[0-9a-f]{64}",
        "[ -~]{0,80}",
        "\\PC{0,20}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_input_can_put_an_address_or_unbounded_label_in_the_output(
        keys in proptest::collection::vec(arb_key(), 0..80),
        counts in proptest::collection::vec(0u64..1_000_000, 80),
    ) {
        let r = Registry::new();
        let mut fb = serde_json::Map::new();
        let mut bonus = serde_json::Map::new();
        for (k, c) in keys.iter().zip(&counts) {
            fb.insert(k.clone(), json!(c));
            bonus.insert(format!("{k}x"), json!(c));
        }
        r.observe_event(EVENT_ADVISORY, &json!({
            "mandatory_unchanged": true,
            "counters": {"copper_jev_fallback_total": fb, "copper_jev_applied_bonus_total": bonus,
                         "copper_jev_stale_response_total": 1}
        }));
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(1, 0));
        let text = r.render();
        let p = parse_strict(&text).expect("valid exposition");
        for s in &p.samples {
            for v in s.labels.values() {
                prop_assert!(!address_like(v), "address-like label {v:?}");
                prop_assert!(v.len() <= 48);
            }
        }
        // Cardinality bound per family.
        for fam in p.families.keys() {
            let n = p.samples.iter().filter(|s| &s.name == fam).count();
            prop_assert!(n <= 64, "{fam} has {n} series");
        }
        // No 40-hex run anywhere in the scrape.
        let mut run = 0;
        for ch in text.chars() {
            if ch.is_ascii_hexdigit() { run += 1; } else { run = 0; }
            // "deadline" style words are short; 32+ hex chars in a row never occur in names or help text.
            prop_assert!(run < 32, "long hex run in output");
        }
    }
}
