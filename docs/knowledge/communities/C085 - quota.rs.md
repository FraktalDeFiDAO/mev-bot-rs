---
graphify-generated: true
graphify-community: "85"
graphify-node-count: 26
graphify-internal-edges: 49
tags:
  - graphify/community
---

# quota.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/quota.rs` — 20 nodes
- `graph` — 6 nodes

## Nodes (26)

- **quota.rs** — `crates/jev-advisor/src/resilience/quota.rs:L1`
- **Quota** — `crates/jev-advisor/src/resilience/quota.rs:L58`
- **QuotaConfig** — `crates/jev-advisor/src/resilience/quota.rs:L32`
- **.new()** — `crates/jev-advisor/src/resilience/quota.rs:L67`
- **.default()** — `crates/jev-advisor/src/resilience/quota.rs:L46`
- **.record_usage()** — `crates/jev-advisor/src/resilience/quota.rs:L113`
- **Duration** — `graph`
- **token_budget_uses_recorded_usage()** — `crates/jev-advisor/src/resilience/quota.rs:L163`
- **.reserve()** — `crates/jev-advisor/src/resilience/quota.rs:L91`
- **.roll()** — `crates/jev-advisor/src/resilience/quota.rs:L77`
- **.cost_micro_usd()** — `crates/jev-advisor/src/resilience/quota.rs:L22`
- **cost_budget_needs_prices_and_is_integer_math()** — `crates/jev-advisor/src/resilience/quota.rs:L177`
- **request_budget_and_reset_at_the_window_boundary()** — `crates/jev-advisor/src/resilience/quota.rs:L150`
- **s()** — `crates/jev-advisor/src/resilience/quota.rs:L139`
- **TokenPrices** — `crates/jev-advisor/src/resilience/quota.rs:L14`
- **usd_budget_without_prices_is_inert()** — `crates/jev-advisor/src/resilience/quota.rs:L194`
- **FallbackReason** — `graph`
- **Self** — `graph`
- **usage()** — `crates/jev-advisor/src/resilience/quota.rs:L142`
- **.micro_usd_used()** — `crates/jev-advisor/src/resilience/quota.rs:L130`
- **.requests_used()** — `crates/jev-advisor/src/resilience/quota.rs:L124`
- **.tokens_used()** — `crates/jev-advisor/src/resilience/quota.rs:L127`
- **Default** — `graph`
- **extreme_usage_saturates()** — `crates/jev-advisor/src/resilience/quota.rs:L206`
- **Option** — `graph`
- **Result** — `graph`

## Related communities

- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C049 - response.rs|response.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
