---
graphify-generated: true
graphify-community: "116"
graphify-node-count: 20
graphify-internal-edges: 43
tags:
  - graphify/community
---

# cache.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/cache.rs` — 14 nodes
- `graph` — 6 nodes

## Nodes (20)

- **cache.rs** — `crates/jev-advisor/src/resilience/cache.rs:L1`
- **TtlLru** — `crates/jev-advisor/src/resilience/cache.rs:L16`
- **.insert()** — `crates/jev-advisor/src/resilience/cache.rs:L50`
- **.new()** — `crates/jev-advisor/src/resilience/cache.rs:L25`
- **Duration** — `graph`
- **s()** — `crates/jev-advisor/src/resilience/cache.rs:L90`
- **.get()** — `crates/jev-advisor/src/resilience/cache.rs:L34`
- **TtlLru<K, V>** — `crates/jev-advisor/src/resilience/cache.rs:L23`
- **V** — `graph`
- **Entry** — `crates/jev-advisor/src/resilience/cache.rs:L9`
- **evicts_least_recently_used_when_full()** — `crates/jev-advisor/src/resilience/cache.rs:L104`
- **expired_entries_are_dropped_before_live_ones()** — `crates/jev-advisor/src/resilience/cache.rs:L117`
- **hit_within_ttl_miss_at_ttl()** — `crates/jev-advisor/src/resilience/cache.rs:L95`
- **zero_ttl_or_cap_disables()** — `crates/jev-advisor/src/resilience/cache.rs:L127`
- **K** — `graph`
- **HashMap** — `graph`
- **.is_empty()** — `crates/jev-advisor/src/resilience/cache.rs:L81`
- **.len()** — `crates/jev-advisor/src/resilience/cache.rs:L77`
- **Option** — `graph`
- **Self** — `graph`

## Related communities

- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
- [[docs/knowledge/communities/C049 - response.rs|response.rs]] — 1 cross-community edges
