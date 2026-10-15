---
graphify-generated: true
graphify-community: "139"
graphify-node-count: 18
graphify-internal-edges: 25
tags:
  - graphify/community
---

# MetricsInner

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/metrics.rs` — 11 nodes
- `graph` — 4 nodes
- `crates/jev-advisor/src/resilience/breaker.rs` — 3 nodes

## Nodes (18)

- **MetricsInner** — `crates/jev-advisor/src/resilience/metrics.rs:L76`
- **metrics.rs** — `crates/jev-advisor/src/resilience/metrics.rs:L1`
- **MetricsSnapshot** — `crates/jev-advisor/src/resilience/metrics.rs:L50`
- **CircuitState** — `crates/jev-advisor/src/resilience/breaker.rs:L42`
- **result_label()** — `crates/jev-advisor/src/resilience/metrics.rs:L16`
- **.snapshot()** — `crates/jev-advisor/src/resilience/metrics.rs:L115`
- **BTreeMap** — `graph`
- **DurationStat** — `crates/jev-advisor/src/resilience/metrics.rs:L41`
- **.attempt()** — `crates/jev-advisor/src/resilience/metrics.rs:L88`
- **.fallback()** — `crates/jev-advisor/src/resilience/metrics.rs:L100`
- **.state()** — `crates/jev-advisor/src/resilience/breaker.rs:L93`
- **Duration** — `graph`
- **FallbackReason** — `graph`
- **.cache_hit()** — `crates/jev-advisor/src/resilience/metrics.rs:L108`
- **.coalesced()** — `crates/jev-advisor/src/resilience/metrics.rs:L111`
- **.gauge()** — `crates/jev-advisor/src/resilience/breaker.rs:L52`
- **.usage()** — `crates/jev-advisor/src/resilience/metrics.rs:L103`
- **Result** — `graph`

## Related communities

- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C066 - Breaker|Breaker]] — 3 cross-community edges
- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 2 cross-community edges
- [[docs/knowledge/communities/C107 - AdvisorSnapshot|AdvisorSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C015 - Worker|Worker]] — 1 cross-community edges
