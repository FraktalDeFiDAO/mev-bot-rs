---
graphify-generated: true
graphify-community: "31"
graphify-node-count: 47
graphify-internal-edges: 195
tags:
  - graphify/community
---

# resilience.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/tests/resilience.rs` — 43 nodes
- `graph` — 4 nodes

## Nodes (47)

- **resilience.rs** — `crates/jev-advisor/tests/resilience.rs:L1`
- **build()** — `crates/jev-advisor/tests/resilience.rs:L49`
- **call()** — `crates/jev-advisor/tests/resilience.rs:L85`
- **rcfg()** — `crates/jev-advisor/tests/resilience.rs:L43`
- **secs()** — `crates/jev-advisor/tests/resilience.rs:L21`
- **ok()** — `crates/jev-advisor/tests/resilience.rs:L25`
- **ms()** — `crates/jev-advisor/tests/resilience.rs:L18`
- **inflight_cap_fail_fast_falls_back_without_network()** — `crates/jev-advisor/tests/resilience.rs:L460`
- **inflight_wait_never_outlasts_the_deadline()** — `crates/jev-advisor/tests/resilience.rs:L505`
- **distinct_requests_do_not_coalesce()** — `crates/jev-advisor/tests/resilience.rs:L560`
- **exact_backoff_schedule_under_a_seeded_rng()** — `crates/jev-advisor/tests/resilience.rs:L190`
- **inflight_cap_wait_queues_within_the_deadline()** — `crates/jev-advisor/tests/resilience.rs:L485`
- **one_attempt()** — `crates/jev-advisor/tests/resilience.rs:L238`
- **ten_concurrent_identical_requests_make_one_http_call()** — `crates/jev-advisor/tests/resilience.rs:L524`
- **wait_hits()** — `crates/jev-advisor/tests/resilience.rs:L75`
- **cache_hit_within_ttl_miss_after()** — `crates/jev-advisor/tests/resilience.rs:L576`
- **cache_is_bounded_lru()** — `crates/jev-advisor/tests/resilience.rs:L596`
- **cancellation_mid_backoff_stops_promptly()** — `crates/jev-advisor/tests/resilience.rs:L219`
- **cfg_for()** — `crates/jev-advisor/tests/resilience.rs:L59`
- **exact_backoff_schedule_without_jitter()** — `crates/jev-advisor/tests/resilience.rs:L168`
- **half_open_admits_exactly_one_probe()** — `crates/jev-advisor/tests/resilience.rs:L312`
- **outage_connection_refused_returns_fallbacks_within_the_deadline()** — `crates/jev-advisor/tests/resilience.rs:L665`
- **request_budget_resets_at_the_window_boundary()** — `crates/jev-advisor/tests/resilience.rs:L368`
- **retries_spend_the_request_budget()** — `crates/jev-advisor/tests/resilience.rs:L388`
- **auth_failure_is_misconfigured_until_config_changes()** — `crates/jev-advisor/tests/resilience.rs:L344`
- **breaker_closed_open_halfopen_closed_and_open_is_offline()** — `crates/jev-advisor/tests/resilience.rs:L247`
- **deadline_clamped_timeout_does_not_blame_the_service()** — `crates/jev-advisor/tests/resilience.rs:L695`
- **failed_probe_reopens_the_breaker()** — `crates/jev-advisor/tests/resilience.rs:L280`
- **never_retries_non_retryable_statuses()** — `crates/jev-advisor/tests/resilience.rs:L123`
- **outage_hanging_server_is_cut_by_attempt_timeout_and_deadline()** — `crates/jev-advisor/tests/resilience.rs:L681`
- **overload_529_then_success()** — `crates/jev-advisor/tests/resilience.rs:L153`
- **retries_429_honoring_retry_after()** — `crates/jev-advisor/tests/resilience.rs:L96`
- **retry_after_beyond_the_deadline_gives_up_without_waiting()** — `crates/jev-advisor/tests/resilience.rs:L110`
- **schema_drift_and_wrong_model_are_not_retried()** — `crates/jev-advisor/tests/resilience.rs:L141`
- **schema_failures_get_a_short_negative_cache()** — `crates/jev-advisor/tests/resilience.rs:L623`
- **TestServer** — `graph`
- **token_budget_from_reported_usage()** — `crates/jev-advisor/tests/resilience.rs:L403`
- **zero_deadline_makes_no_network_call()** — `crates/jev-advisor/tests/resilience.rs:L706`
- **cost_is_zero_and_unenforced_without_prices()** — `crates/jev-advisor/tests/resilience.rs:L450`
- **dead_port()** — `crates/jev-advisor/tests/resilience.rs:L651`
- **Duration** — `graph`
- **usd_budget_requires_prices_and_then_binds()** — `crates/jev-advisor/tests/resilience.rs:L426`
- **failures_are_not_cached()** — `crates/jev-advisor/tests/resilience.rs:L609`
- **hits()** — `crates/jev-advisor/tests/resilience.rs:L71`
- **C** — `graph`
- **invalid_config_is_rejected()** — `crates/jev-advisor/tests/resilience.rs:L716`
- **Rc** — `graph`

## Related communities

- [[docs/knowledge/communities/C268 - script_for|script_for]] — 9 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C054 - transport.rs|transport.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C065 - RedactedFeatures|RedactedFeatures]] — 3 cross-community edges
- [[docs/knowledge/communities/C306 - reason|reason]] — 2 cross-community edges
- [[docs/knowledge/communities/C001 - lib.rs (C001)|lib.rs (C001)]] — 1 cross-community edges
- [[docs/knowledge/communities/C118 - jev-advisor.md|jev-advisor.md]] — 1 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
- [[docs/knowledge/communities/C159 - Lit|Lit]] — 1 cross-community edges
- [[docs/knowledge/communities/C038 - ManualClock|ManualClock]] — 1 cross-community edges
- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 1 cross-community edges
