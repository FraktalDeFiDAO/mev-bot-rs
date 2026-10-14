//! JEV-LIQ-013: the advisor's Prometheus exposition, the optional loopback listener on the real binary,
//! and the checked-in alert rules and dashboard (every series they use must be produced).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use copper_jev_advisor::CountersSnapshot;
use copper_jev_advisor::observability::{
    ADVISOR_FAMILIES, AdvisorSnapshot, SnapshotCache, install, render_advisor,
};
use jev_advisor::{DurationStat, FallbackReason, MetricsSnapshot};
use mev_liq_state::QueueStats;
use mev_metrics::{
    Exposition, Registry,
    prom::address_like,
    registry::{EVENT_ADVISORY, EVENT_SCHEDULE_COUNTERS, SCHED_FAMILIES},
    textcheck::{Parsed, parse_strict},
};
use proptest::prelude::*;
use serde_json::json;

const RESULTS: [&str; 11] = [
    "ok",
    "rate_limited",
    "overloaded",
    "server_error",
    "connection_error",
    "timeout",
    "auth_error",
    "bad_request",
    "cancelled",
    "schema_error",
    "other_error",
];

const ALL_FALLBACKS: [FallbackReason; 11] = [
    FallbackReason::CircuitOpen,
    FallbackReason::Misconfigured,
    FallbackReason::RequestBudget,
    FallbackReason::TokenBudget,
    FallbackReason::CostBudget,
    FallbackReason::InflightSaturated,
    FallbackReason::DeadlineExceeded,
    FallbackReason::Cancelled,
    FallbackReason::RetriesExhausted,
    FallbackReason::NonRetryable,
    FallbackReason::NegativeCached,
];

fn full_snapshot() -> AdvisorSnapshot {
    let mut jev = MetricsSnapshot::default();
    for r in RESULTS {
        jev.requests_total.insert(r, 3);
        jev.request_duration.insert(
            r,
            DurationStat {
                count: 6,
                sum_seconds: 1.5,
                buckets: [1, 1, 1, 1, 1, 1, 0, 0, 0],
            },
        );
    }
    for f in ALL_FALLBACKS {
        jev.fallback_total.insert(f.label(), 2);
    }
    jev.circuit_state = 2;
    jev.circuit_misconfigured = true;
    jev.input_tokens_total = 100;
    jev.output_tokens_total = 50;
    jev.estimated_cost_micro_usd_total = 2_500_000;
    jev.cache_hits_total = 4;
    jev.coalesced_total = 1;
    jev.inflight = 2;
    jev.window_requests_used = 9;
    jev.window_tokens_used = 900;
    jev.window_micro_usd_used = 1_000_000;
    AdvisorSnapshot {
        jev,
        queue: Some(QueueStats {
            pending: 3,
            in_flight: 1,
            done: 10,
            dead: 2,
            missed_total: 5,
        }),
        counters: CountersSnapshot {
            claimed: 5,
            applied: 4,
            superseded: 1,
            discarded_stale_response: 2,
            dead_invalid_features: 1,
            dead_invalid_response: 1,
            dead_non_retryable: 1,
            failed_retry: 2,
            released: 1,
            lease_lost: 1,
            pulled: 6,
            export_rows_missed: 1,
        },
    }
}

fn render_snapshot(s: &AdvisorSnapshot) -> String {
    let mut ex = Exposition::new();
    render_advisor(&mut ex, s);
    assert_eq!(ex.dropped(), 0, "the renderer dropped samples");
    ex.finish()
}

/// Hot-path registry fed with the scheduler and advisory events, as the engine would.
fn hot_registry() -> Registry {
    let r = Registry::new();
    r.observe_event(
        EVENT_SCHEDULE_COUNTERS,
        &json!({
            "now_ns": 1,
            "candidates": {"p0": 1, "p1": 2, "p2": 3, "p3": 4},
            "deadline_misses_p0": 1, "deadline_misses_p1": 0, "deadline_misses_other": 0,
            "deadline_misses_backlog_p0": 1, "deadline_misses_backlog_p1": 0,
            "deadline_misses_backlog_other": 0, "coverage_violations": 1
        }),
    );
    r.observe_event(
        EVENT_ADVISORY,
        &json!({"mandatory_unchanged": false, "counters": {
            "copper_jev_fallback_total": {"expired": 1}, "copper_jev_stale_response_total": 1,
            "copper_jev_applied_bonus_total": {"high": 1}}}),
    );
    r
}

/// Every produced series name (including histogram suffixes) and, per metric, label name -> values.
struct Produced {
    names: BTreeSet<String>,
    labels: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

fn produced() -> Produced {
    let hot = parse_strict(&hot_registry().render()).unwrap();
    let adv = parse_strict(&render_snapshot(&full_snapshot())).unwrap();
    let mut names = BTreeSet::new();
    let mut labels: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for p in [&hot, &adv] {
        names.extend(p.families.keys().cloned());
        for s in &p.samples {
            names.insert(s.name.clone());
            for (k, v) in &s.labels {
                labels
                    .entry(s.name.clone())
                    .or_default()
                    .entry(k.clone())
                    .or_default()
                    .insert(v.clone());
            }
        }
    }
    Produced { names, labels }
}

// ---- exposition -------------------------------------------------------------------------------

#[test]
fn advisor_exposition_is_valid_and_complete() {
    let p = parse_strict(&render_snapshot(&full_snapshot())).unwrap();
    let got: BTreeSet<_> = p.families.keys().map(String::as_str).collect();
    let want: BTreeSet<_> = ADVISOR_FAMILIES.iter().copied().collect();
    assert_eq!(got, want);
    // Unit conversion: micro-USD to USD.
    assert_eq!(p.get("copper_jev_estimated_cost_usd_total", &[]), Some(2.5));
    assert_eq!(p.get("copper_jev_circuit_state", &[]), Some(2.0));
    assert_eq!(
        p.get("copper_jev_requests_total", &[("result", "rate_limited")]),
        Some(3.0)
    );
    assert_eq!(
        p.get(
            "copper_jev_request_duration_seconds_count",
            &[("status", "ok")]
        ),
        Some(6.0)
    );
    assert_eq!(
        p.get("copper_jev_advisor_queue_rows", &[("state", "dead")]),
        Some(2.0)
    );
}

#[test]
fn hot_path_and_advisor_families_do_not_overlap_except_the_shared_names() {
    // copper_jev_fallback_total and copper_jev_stale_response_total exist in both processes (policy
    // verdicts on the bot, client calls on the advisor); everything else is process-specific.
    let shared: BTreeSet<_> = SCHED_FAMILIES
        .iter()
        .filter(|f| ADVISOR_FAMILIES.contains(f))
        .copied()
        .collect();
    assert_eq!(
        shared,
        BTreeSet::from([
            "copper_jev_fallback_total",
            "copper_jev_stale_response_total"
        ])
    );
}

#[test]
fn a_failed_queue_query_is_reported_not_faked() {
    let mut s = full_snapshot();
    s.queue = None;
    let p = parse_strict(&render_snapshot(&s)).unwrap();
    assert_eq!(p.get("copper_jev_advisor_queue_stats_ok", &[]), Some(0.0));
    assert!(!p.families.contains_key("copper_jev_advisor_queue_rows"));
}

#[test]
fn nothing_is_rendered_before_the_first_snapshot() {
    let reg = Registry::new();
    let cache = SnapshotCache::default();
    install(&reg, &cache);
    assert_eq!(reg.render(), "");
    cache.store(full_snapshot());
    assert!(parse_strict(&reg.render()).is_ok());
}

#[test]
fn a_scrape_never_blocks_the_snapshot_writer() {
    let reg = Arc::new(Registry::new());
    let cache = SnapshotCache::default();
    install(&reg, &cache);
    cache.store(full_snapshot());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scraper = {
        let (reg, stop) = (reg.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut n = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = reg.render();
                n += 1;
            }
            n
        })
    };
    let t = Instant::now();
    for i in 0..5000u64 {
        let mut s = full_snapshot();
        s.jev.input_tokens_total = i;
        cache.store(s);
    }
    let took = t.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(scraper.join().unwrap() > 0);
    assert!(took < Duration::from_secs(30), "writer took {took:?}");
}

fn counter_values(p: &Parsed) -> BTreeMap<(String, BTreeMap<String, String>), f64> {
    p.samples
        .iter()
        .filter(|s| {
            let base = s
                .name
                .strip_suffix("_bucket")
                .or_else(|| s.name.strip_suffix("_count"))
                .unwrap_or(&s.name);
            p.families.get(&s.name).is_some_and(|k| k == "counter")
                || p.families.get(base).is_some_and(|k| k == "histogram")
        })
        .map(|s| ((s.name.clone(), s.labels.clone()), s.value))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn random_snapshots_render_valid_bounded_address_free_text(
        counts in proptest::collection::vec(0u64..5_000_000, 11),
        buckets in proptest::collection::vec(0u64..1000, 9),
        circuit in 0u8..3,
        queue in proptest::option::of((0u64..10_000, 0u64..10_000, 0u64..10_000, 0u64..10_000, 0u64..10_000)),
        micro in 0u64..50_000_000_000,
        later in proptest::collection::vec(0u64..1000, 11),
    ) {
        let mut a = full_snapshot();
        a.jev.circuit_state = circuit;
        a.jev.estimated_cost_micro_usd_total = micro;
        a.queue = queue.map(|(p, i, d, x, m)| QueueStats { pending: p, in_flight: i, done: d, dead: x, missed_total: m });
        for (i, &r) in RESULTS.iter().enumerate() {
            a.jev.requests_total.insert(r, counts[i]);
            let mut b = [0u64; 9];
            b.copy_from_slice(&buckets);
            a.jev.request_duration.insert(r, DurationStat { count: b.iter().sum(), sum_seconds: b.iter().sum::<u64>() as f64 * 0.1, buckets: b });
        }
        let text = render_snapshot(&a);
        let p = parse_strict(&text).expect("valid exposition");
        for s in &p.samples {
            for v in s.labels.values() {
                prop_assert!(!address_like(v), "address-like label {v:?}");
            }
        }
        for fam in p.families.keys() {
            prop_assert!(p.samples.iter().filter(|s| &s.name == fam).count() <= 64);
        }
        let mut run = 0;
        for ch in text.chars() {
            if ch.is_ascii_hexdigit() { run += 1 } else { run = 0 }
            prop_assert!(run < 32);
        }
        // Counters never go down when the underlying monotonic counters only grow.
        let before = counter_values(&p);
        let mut b = a.clone();
        for (i, &r) in RESULTS.iter().enumerate() {
            *b.jev.requests_total.entry(r).or_default() += later[i];
            let d = b.jev.request_duration.get_mut(r).unwrap();
            d.buckets[i % 9] += later[i];
            d.count += later[i];
            d.sum_seconds += later[i] as f64;
        }
        b.jev.estimated_cost_micro_usd_total += later[0];
        b.counters.applied += later[1];
        let after = counter_values(&parse_strict(&render_snapshot(&b)).unwrap());
        for (k, v) in before {
            let nv = after.get(&k).copied().unwrap_or(f64::NAN);
            prop_assert!(nv >= v, "{k:?} went from {v} to {nv}");
        }
    }
}

// ---- alert rules and dashboard reference only produced series --------------------------------

fn monitoring(file: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/monitoring")
        .join(file);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[derive(Debug)]
struct Matcher {
    key: String,
    op: String,
    value: String,
}

#[derive(Debug)]
struct Selector {
    metric: String,
    matchers: Vec<Matcher>,
}

/// Selectors of the form `name` or `name{k="v",k2=~"a|b"}` in a PromQL expression. Anything not
/// starting with `copper_` (and `up`) is a function, keyword or label list and is skipped.
fn selectors(expr: &str) -> Vec<Selector> {
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c == '"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
        } else if c == '[' {
            while i < b.len() && b[i] != b']' {
                i += 1;
            }
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' || c == ':' {
            let s = i;
            while i < b.len()
                && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b':')
            {
                i += 1;
            }
            let name = &expr[s..i];
            let mut matchers = Vec::new();
            let is_metric = name.starts_with("copper_") || name == "up";
            if i < b.len() && b[i] == b'{' {
                let e = expr[i..].find('}').expect("unclosed selector") + i;
                if is_metric {
                    for m in expr[i + 1..e].split(',').filter(|m| !m.trim().is_empty()) {
                        let op_at = m.find(['=', '!']).expect("matcher op");
                        let rest = &m[op_at..];
                        let op_len = if rest.starts_with("=~")
                            || rest.starts_with("!~")
                            || rest.starts_with("!=")
                        {
                            2
                        } else {
                            1
                        };
                        matchers.push(Matcher {
                            key: m[..op_at].trim().to_string(),
                            op: rest[..op_len].to_string(),
                            value: rest[op_len..].trim().trim_matches('"').to_string(),
                        });
                    }
                }
                i = e + 1;
            }
            if is_metric {
                out.push(Selector {
                    metric: name.to_string(),
                    matchers,
                });
            }
        } else {
            i += 1;
        }
    }
    out
}

fn assert_produced(what: &str, expr: &str, prod: &Produced) {
    let sels = selectors(expr);
    assert!(!sels.is_empty(), "{what}: no copper_ series in {expr}");
    for s in sels {
        if s.metric == "up" {
            continue; // synthesised by Prometheus for every scrape target
        }
        assert!(
            prod.names.contains(&s.metric),
            "{what}: {} is not produced ({expr})",
            s.metric
        );
        for m in &s.matchers {
            let have = prod
                .labels
                .get(&s.metric)
                .and_then(|l| l.get(&m.key))
                .unwrap_or_else(|| panic!("{what}: {} has no label {}", s.metric, m.key));
            let alts: Vec<&str> = if m.op.ends_with('~') {
                m.value.split('|').collect()
            } else {
                vec![m.value.as_str()]
            };
            assert!(
                alts.iter().all(|a| {
                    // Regex alternatives are literal here (`lease_lost|dead_.*` style prefixes are matched by prefix).
                    match a.strip_suffix(".*") {
                        Some(pre) => have.iter().any(|h| h.starts_with(pre)),
                        None => have.contains(*a),
                    }
                }),
                "{what}: {}{{{}{}{:?}}} matches a label value that is never produced (have {have:?})",
                s.metric,
                m.key,
                m.op,
                m.value
            );
        }
    }
}

struct Alert {
    name: String,
    expr: String,
    labels: BTreeMap<String, String>,
    runbook: String,
}

fn parse_rules(text: &str) -> Vec<Alert> {
    let mut out: Vec<Alert> = Vec::new();
    let mut in_labels = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        let unq = |v: &str| v.trim().trim_matches('"').to_string();
        if let Some(n) = t.strip_prefix("- alert:") {
            out.push(Alert {
                name: unq(n),
                expr: String::new(),
                labels: BTreeMap::new(),
                runbook: String::new(),
            });
            in_labels = false;
        } else if let Some(a) = out.last_mut() {
            if let Some(e) = t.strip_prefix("expr:") {
                assert!(
                    !e.trim().starts_with('|') && !e.trim().starts_with('>'),
                    "expr must be single line"
                );
                a.expr = e.trim().to_string();
                in_labels = false;
            } else if t == "labels:" {
                in_labels = true;
            } else if t == "annotations:" {
                in_labels = false;
            } else if let Some(r) = t.strip_prefix("runbook:") {
                a.runbook = unq(r);
            } else if in_labels && let Some((k, v)) = t.split_once(':') {
                a.labels.insert(k.trim().to_string(), unq(v));
            }
        }
    }
    out
}

#[test]
fn every_alert_references_only_produced_series_and_labels() {
    let prod = produced();
    let alerts = parse_rules(&monitoring("copper-liq.rules.yml"));
    assert!(alerts.len() >= 10, "{}", alerts.len());
    for a in &alerts {
        assert!(!a.expr.is_empty(), "{} has no expr", a.name);
        assert_produced(&a.name, &a.expr, &prod);
    }
}

#[test]
fn advisory_alerts_never_page_and_critical_ones_do() {
    let alerts = parse_rules(&monitoring("copper-liq.rules.yml"));
    let spec_critical = [
        "CopperLiqMandatoryDeadlineMiss",
        "CopperLiqAdvisoryChangedMandatory",
    ];
    for c in spec_critical {
        assert!(alerts.iter().any(|a| a.name == c), "missing {c}");
    }
    for a in &alerts {
        let (sev, page) = (a.labels["severity"].as_str(), a.labels["page"].as_str());
        if a.name.starts_with("CopperJev") {
            assert_eq!(
                (sev, page),
                ("warning", "false"),
                "{} must be non-paging",
                a.name
            );
            assert_eq!(a.labels["class"], "advisory-only");
        } else {
            assert_eq!((sev, page), ("critical", "true"), "{}", a.name);
        }
    }
}

#[test]
fn every_alert_links_to_a_runbook_entry() {
    let rb = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/runbooks/copper-liq-alerts.md"),
    )
    .unwrap();
    let headings: BTreeSet<String> = rb
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();
    for a in parse_rules(&monitoring("copper-liq.rules.yml")) {
        let (path, anchor) = a.runbook.split_once('#').expect("runbook anchor");
        assert_eq!(path, "docs/runbooks/copper-liq-alerts.md");
        assert!(
            headings.contains(anchor),
            "{}: no runbook heading for {anchor}",
            a.name
        );
        assert_eq!(anchor, a.name.to_ascii_lowercase());
    }
}

#[test]
fn dashboard_is_valid_json_with_the_four_sections_and_only_produced_series() {
    let prod = produced();
    let d: serde_json::Value =
        serde_json::from_str(&monitoring("copper-liq-dashboard.json")).unwrap();
    let panels = d["panels"].as_array().unwrap();
    let rows: Vec<&str> = panels
        .iter()
        .filter(|p| p["type"] == "row")
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert_eq!(rows.len(), 4, "{rows:?}");
    for want in [
        "Hot-path SLOs",
        "Mandatory coverage",
        "Jev quality and cost",
        "Replay outcomes",
    ] {
        assert!(
            rows.iter().any(|r| r.starts_with(want)),
            "missing section {want}"
        );
    }
    let mut queries = 0;
    for p in panels
        .iter()
        .filter(|p| p["type"] != "row" && p["type"] != "text")
    {
        let targets = p["targets"]
            .as_array()
            .unwrap_or_else(|| panic!("panel {} has no targets", p["title"]));
        assert!(!targets.is_empty());
        for t in targets {
            assert_produced(
                p["title"].as_str().unwrap(),
                t["expr"].as_str().unwrap(),
                &prod,
            );
            queries += 1;
        }
    }
    assert!(queries >= 15, "{queries}");
    let ids: BTreeSet<u64> = panels.iter().map(|p| p["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), panels.len(), "panel ids must be unique");
}

#[test]
fn the_selector_scanner_catches_a_missing_series() {
    let prod = produced();
    let r = std::panic::catch_unwind(|| {
        assert_produced("t", "rate(copper_jev_no_such_total[5m])", &produced())
    });
    assert!(r.is_err());
    let r = std::panic::catch_unwind(|| {
        assert_produced(
            "t",
            "copper_jev_fallback_total{reason=\"nope\"}",
            &produced(),
        )
    });
    assert!(r.is_err());
    assert_produced(
        "t",
        "sum by (tier) (increase(copper_liq_recheck_deadline_miss_total{tier=~\"p0|p1\"}[5m])) > 0",
        &prod,
    );
}

// ---- the real binary ---------------------------------------------------------------------------

fn bin(env: &Env) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_copper-jev-advisor"));
    c.env_clear()
        .env("TYPESAFE_API_KEY_FILE", env.key_file())
        .env("JEV_POLICY_VERSION", POLICY)
        .env("ADVISOR_DB", env.advisor_path())
        .env("SNAPSHOT_DB", env.snapshot_path())
        .env("ADVISOR_POLL_MS", "20")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn scrape(port: u16) -> Option<String> {
    let mut c = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(300),
    )
    .ok()?;
    c.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    c.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
        .ok()?;
    let mut s = String::new();
    c.read_to_string(&mut s).ok()?;
    s.split("\r\n\r\n").nth(1).map(str::to_string)
}

#[test]
fn metrics_are_off_by_default_and_non_loopback_needs_the_override() {
    let env = Env::new(0);
    // Not loopback: refused before anything starts.
    let out = bin(&env)
        .args(["--metrics-addr", "0.0.0.0:9109", "check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not loopback"), "{err}");
    // Hostnames and garbage are refused too.
    let out = bin(&env)
        .args(["--metrics-addr", "localhost:9109", "check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // Default: `check` works with no exporter flag at all.
    let out = bin(&env).arg("check").output().unwrap();
    assert!(out.status.success());
}

#[test]
fn the_binary_serves_valid_metrics_on_loopback_and_stops_cleanly() {
    let env = Env::new(0);
    let port = free_port();
    let mut child = bin(&env)
        .args(["--metrics-addr", &format!("127.0.0.1:{port}")])
        .spawn()
        .unwrap();
    let start = Instant::now();
    let body = loop {
        if let Some(b) = scrape(port).filter(|b| b.contains("copper_jev_advisor_queue_rows")) {
            break b;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "no metrics after 20s"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let p = parse_strict(&body).unwrap();
    assert_eq!(p.get("copper_jev_circuit_state", &[]), Some(0.0));
    assert_eq!(p.get("copper_jev_advisor_queue_stats_ok", &[]), Some(1.0));
    // Only advisor families (and nothing from the hot path) are exposed.
    for f in p.families.keys() {
        assert!(
            ADVISOR_FAMILIES.contains(&f.as_str()),
            "unexpected family {f}"
        );
    }
    // Optional evidence dump for `promtool check metrics`.
    if let Ok(path) = std::env::var("COPPER_METRICS_DUMP") {
        std::fs::write(path, &body).unwrap();
    }
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "no exit after SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    assert!(scrape(port).is_none(), "listener must be gone after exit");
}
