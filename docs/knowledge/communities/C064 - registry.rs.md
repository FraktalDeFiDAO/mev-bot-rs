---
graphify-generated: true
graphify-community: "64"
graphify-node-count: 32
graphify-internal-edges: 75
tags:
  - graphify/community
---

# registry.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-metrics/src/registry.rs` — 24 nodes
- `graph` — 8 nodes

## Nodes (32)

- **registry.rs** — `crates/mev-metrics/src/registry.rs:L1`
- **Registry** — `crates/mev-metrics/src/registry.rs:L109`
- **.observe_event()** — `crates/mev-metrics/src/registry.rs:L153`
- **.new()** — `crates/mev-metrics/src/registry.rs:L140`
- **.render()** — `crates/mev-metrics/src/registry.rs:L261`
- **Value** — `graph`
- **u()** — `crates/mev-metrics/src/registry.rs:L114`
- **.observe_advisory()** — `crates/mev-metrics/src/registry.rs:L234`
- **.observe_census()** — `crates/mev-metrics/src/registry.rs:L201`
- **.observe_sched()** — `crates/mev-metrics/src/registry.rs:L163`
- **add_map()** — `crates/mev-metrics/src/registry.rs:L118`
- **passes_accumulate_once_and_replays_are_ignored()** — `crates/mev-metrics/src/registry.rs:L431`
- **State** — `crates/mev-metrics/src/registry.rs:L102`
- **.observe_refresh()** — `crates/mev-metrics/src/registry.rs:L218`
- **advisory_counters_accumulate_across_passes_and_flag_a_mandatory_change()** — `crates/mev-metrics/src/registry.rs:L450`
- **AdvisoryState** — `crates/mev-metrics/src/registry.rs:L82`
- **census_and_refresh_events_become_gauges_and_counters()** — `crates/mev-metrics/src/registry.rs:L398`
- **distinct_label_values_are_capped()** — `crates/mev-metrics/src/registry.rs:L490`
- **malformed_events_and_address_keys_are_harmless()** — `crates/mev-metrics/src/registry.rs:L473`
- **.render_into()** — `crates/mev-metrics/src/registry.rs:L272`
- **now_secs()** — `crates/mev-metrics/src/registry.rs:L133`
- **sched()** — `crates/mev-metrics/src/registry.rs:L386`
- **String** — `graph`
- **BTreeMap** — `graph`
- **PipelineState** — `crates/mev-metrics/src/registry.rs:L92`
- **SchedState** — `crates/mev-metrics/src/registry.rs:L70`
- **Collector** — `graph`
- **Mutex** — `graph`
- **nothing_is_emitted_before_anything_was_measured()** — `crates/mev-metrics/src/registry.rs:L426`
- **Option** — `graph`
- **Self** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C299 - .add_collector|.add_collector]] — 2 cross-community edges
- [[docs/knowledge/communities/C075 - Exposition|Exposition]] — 2 cross-community edges
- [[docs/knowledge/communities/C001 - lib.rs (C001)|lib.rs (C001)]] — 1 cross-community edges
