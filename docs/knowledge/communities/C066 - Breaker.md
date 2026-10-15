---
graphify-generated: true
graphify-community: "66"
graphify-node-count: 31
graphify-internal-edges: 85
tags:
  - graphify/community
---

# Breaker

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/breaker.rs` — 25 nodes
- `graph` — 6 nodes

## Nodes (31)

- **Breaker** — `crates/jev-advisor/src/resilience/breaker.rs:L72`
- **breaker.rs** — `crates/jev-advisor/src/resilience/breaker.rs:L1`
- **.on_failure()** — `crates/jev-advisor/src/resilience/breaker.rs:L147`
- **Duration** — `graph`
- **.new()** — `crates/jev-advisor/src/resilience/breaker.rs:L82`
- **BreakerConfig** — `crates/jev-advisor/src/resilience/breaker.rs:L25`
- **s()** — `crates/jev-advisor/src/resilience/breaker.rs:L216`
- **auth_failure_is_sticky_until_config_changes()** — `crates/jev-advisor/src/resilience/breaker.rs:L292`
- **cfg()** — `crates/jev-advisor/src/resilience/breaker.rs:L219`
- **.on_reachable()** — `crates/jev-advisor/src/resilience/breaker.rs:L130`
- **closed_open_halfopen_closed()** — `crates/jev-advisor/src/resilience/breaker.rs:L228`
- **released_probe_slot_can_be_retaken()** — `crates/jev-advisor/src/resilience/breaker.rs:L281`
- **success_resets_the_consecutive_count()** — `crates/jev-advisor/src/resilience/breaker.rs:L270`
- **failed_probe_reopens_with_fresh_cooldown()** — `crates/jev-advisor/src/resilience/breaker.rs:L254`
- **failure_ratio_in_window_opens()** — `crates/jev-advisor/src/resilience/breaker.rs:L308`
- **.record()** — `crates/jev-advisor/src/resilience/breaker.rs:L183`
- **old_samples_leave_the_window()** — `crates/jev-advisor/src/resilience/breaker.rs:L332`
- **.admit()** — `crates/jev-advisor/src/resilience/breaker.rs:L97`
- **.ratio_tripped()** — `crates/jev-advisor/src/resilience/breaker.rs:L195`
- **Admission** — `crates/jev-advisor/src/resilience/breaker.rs:L62`
- **RatioTrip** — `crates/jev-advisor/src/resilience/breaker.rs:L16`
- **.config_changed()** — `crates/jev-advisor/src/resilience/breaker.rs:L168`
- **.default()** — `crates/jev-advisor/src/resilience/breaker.rs:L32`
- **.on_auth_failure()** — `crates/jev-advisor/src/resilience/breaker.rs:L162`
- **.open()** — `crates/jev-advisor/src/resilience/breaker.rs:L175`
- **.release()** — `crates/jev-advisor/src/resilience/breaker.rs:L123`
- **FallbackReason** — `graph`
- **Self** — `graph`
- **VecDeque** — `graph`
- **Default** — `graph`
- **Option** — `graph`

## Related communities

- [[docs/knowledge/communities/C139 - MetricsInner|MetricsInner]] — 3 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
