---
graphify-generated: true
graphify-community: "29"
graphify-node-count: 49
graphify-internal-edges: 96
tags:
  - graphify/community
---

# ShadowSchedule

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 22 nodes
- `crates/mev-liquidate/src/shadow.rs` — 14 nodes
- `crates/mev-liq-sched/src/shadow.rs` — 12 nodes
- `crates/mev-liquidate/src/shadow_span_tests.rs` — 1 node

## Nodes (49)

- **ShadowSchedule** — `crates/mev-liquidate/src/shadow.rs:L47`
- **run_shadow_pass_advisory()** — `crates/mev-liq-sched/src/shadow.rs:L212`
- **run_shadow_pass()** — `crates/mev-liq-sched/src/shadow.rs:L186`
- **ShadowReport** — `crates/mev-liq-sched/src/shadow.rs:L76`
- **ShadowParams** — `crates/mev-liq-sched/src/shadow.rs:L18`
- **load()** — `crates/mev-liq-sched/src/shadow.rs:L143`
- **shadow.rs** — `crates/mev-liq-sched/src/shadow.rs:L1`
- **ShadowError** — `crates/mev-liq-sched/src/shadow.rs:L32`
- **.spawn_pass()** — `crates/mev-liquidate/src/shadow.rs:L95`
- **account_for_load()** — `crates/mev-liq-sched/src/shadow.rs:L153`
- **AdvisoryShadow** — `crates/mev-liquidate/src/shadow.rs:L40`
- **.from_output()** — `crates/mev-liq-sched/src/shadow.rs:L86`
- **record_pass_span()** — `crates/mev-liquidate/src/shadow.rs:L211`
- **shadow.rs** — `crates/mev-liquidate/src/shadow.rs:L1`
- **.new()** — `crates/mev-liquidate/src/shadow.rs:L60`
- **pass_span_carries_chain_pass_block_and_revision_and_no_address()** — `crates/mev-liquidate/src/shadow_span_tests.rs:L57`
- **Self** — `graph`
- **.record()** — `crates/mev-liquidate/src/shadow.rs:L161`
- **pass_span()** — `crates/mev-liquidate/src/shadow.rs:L198`
- **Result** — `graph`
- **.every()** — `crates/mev-liquidate/src/shadow.rs:L77`
- **.fmt()** — `crates/mev-liq-sched/src/shadow.rs:L38`
- **.record_advisory()** — `crates/mev-liquidate/src/shadow.rs:L135`
- **.with_advisory()** — `crates/mev-liquidate/src/shadow.rs:L88`
- **.with_sink()** — `crates/mev-liquidate/src/shadow.rs:L82`
- **ShadowCounters** — `crates/mev-liq-sched/src/shadow.rs:L49`
- **SnapshotRepository** — `graph`
- **SnapshotRepository** — `graph`
- **tick_opt()** — `crates/mev-liquidate/src/shadow.rs:L231`
- **.enabled()** — `crates/mev-liquidate/src/shadow.rs:L72`
- **Address** — `graph`
- **Arc** — `graph`
- **Duration** — `graph`
- **Option** — `graph`
- **Option** — `graph`
- **ShadowSink** — `graph`
- **Span** — `graph`
- **.needs_alert()** — `crates/mev-liq-sched/src/shadow.rs:L136`
- **AtomicBool** — `graph`
- **DecisionFn** — `graph`
- **DecisionSource** — `graph`
- **Display** — `graph`
- **Error** — `graph`
- **Formatter** — `graph`
- **HealthFn** — `graph`
- **Interval** — `graph`
- **Self** — `graph`
- **String** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 9 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 8 cross-community edges
- [[docs/knowledge/communities/C013 - Runner|Runner]] — 5 cross-community edges
- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 5 cross-community edges
- [[docs/knowledge/communities/C144 - Capture|Capture]] — 2 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C199 - reasons|reasons]] — 1 cross-community edges
- [[docs/knowledge/communities/C249 - pass_cost.rs|pass_cost.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C044 - liq_shadow.rs|liq_shadow.rs]] — 1 cross-community edges
