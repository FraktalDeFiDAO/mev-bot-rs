---
graphify-generated: true
graphify-community: "153"
graphify-node-count: 16
graphify-internal-edges: 24
tags:
  - graphify/community
---

# mod.rs (C153)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/backoff.rs` — 7 nodes
- `crates/jev-advisor/src/resilience/mod.rs` — 5 nodes
- `graph` — 4 nodes

## Nodes (16)

- **mod.rs** — `crates/jev-advisor/src/resilience/mod.rs:L1`
- **backoff.rs** — `crates/jev-advisor/src/resilience/backoff.rs:L1`
- **backoff_delay()** — `crates/jev-advisor/src/resilience/backoff.rs:L29`
- **RetryConfig** — `crates/jev-advisor/src/resilience/backoff.rs:L6`
- **.default()** — `crates/jev-advisor/src/resilience/backoff.rs:L17`
- **Unavailable** — `crates/jev-advisor/src/resilience/mod.rs:L68`
- **Duration** — `graph`
- **FallbackReason** — `crates/jev-advisor/src/resilience/mod.rs:L27`
- **jitter_stays_within_band_and_hits_the_edges()** — `crates/jev-advisor/src/resilience/backoff.rs:L57`
- **no_jitter_schedule_doubles_to_cap()** — `crates/jev-advisor/src/resilience/backoff.rs:L45`
- **huge_attempt_counts_do_not_overflow()** — `crates/jev-advisor/src/resilience/backoff.rs:L65`
- **.label()** — `crates/jev-advisor/src/resilience/mod.rs:L49`
- **ConfigError** — `crates/jev-advisor/src/resilience/mod.rs:L77`
- **Default** — `graph`
- **Option** — `graph`
- **Self** — `graph`

## Related communities

- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 2 cross-community edges
- [[docs/knowledge/communities/C111 - scanner.rs|scanner.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C116 - cache.rs|cache.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C031 - resilience.rs|resilience.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C038 - ManualClock|ManualClock]] — 1 cross-community edges
- [[docs/knowledge/communities/C066 - Breaker|Breaker]] — 1 cross-community edges
- [[docs/knowledge/communities/C008 - Metrics|Metrics]] — 1 cross-community edges
- [[docs/knowledge/communities/C085 - quota.rs|quota.rs]] — 1 cross-community edges
