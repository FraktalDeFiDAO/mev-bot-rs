---
graphify-generated: true
graphify-community: "75"
graphify-node-count: 29
graphify-internal-edges: 62
tags:
  - graphify/community
---

# Exposition

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-metrics/src/prom.rs` — 24 nodes
- `graph` — 4 nodes
- `crates/mev-metrics/src/lib.rs` — 1 node

## Nodes (29)

- **Exposition** — `crates/mev-metrics/src/prom.rs:L37`
- **prom.rs** — `crates/mev-metrics/src/prom.rs:L1`
- **.labels_text()** — `crates/mev-metrics/src/prom.rs:L148`
- **String** — `graph`
- **.family()** — `crates/mev-metrics/src/prom.rs:L133`
- **.new()** — `crates/mev-metrics/src/prom.rs:L118`
- **.histogram()** — `crates/mev-metrics/src/prom.rs:L198`
- **.sample()** — `crates/mev-metrics/src/prom.rs:L175`
- **.admit()** — `crates/mev-metrics/src/prom.rs:L164`
- **histogram_is_cumulative_with_inf_sum_and_count()** — `crates/mev-metrics/src/prom.rs:L293`
- **invalid_names_drop_the_sample_instead_of_emitting_garbage()** — `crates/mev-metrics/src/prom.rs:L255`
- **.finish()** — `crates/mev-metrics/src/prom.rs:L233`
- **duplicate_family_and_series_cap()** — `crates/mev-metrics/src/prom.rs:L270`
- **histogram_with_wrong_shape_is_dropped()** — `crates/mev-metrics/src/prom.rs:L306`
- **bound_label_value()** — `crates/mev-metrics/src/prom.rs:L79`
- **fmt_value()** — `crates/mev-metrics/src/prom.rs:L105`
- **lib.rs** — `crates/mev-metrics/src/lib.rs:L1`
- **Option** — `graph`
- **.family_names()** — `crates/mev-metrics/src/prom.rs:L128`
- **.sample_u64()** — `crates/mev-metrics/src/prom.rs:L192`
- **address_like()** — `crates/mev-metrics/src/prom.rs:L59`
- **escape_help()** — `crates/mev-metrics/src/prom.rs:L101`
- **escape_label_value()** — `crates/mev-metrics/src/prom.rs:L88`
- **valid_label_name()** — `crates/mev-metrics/src/prom.rs:L51`
- **valid_metric_name()** — `crates/mev-metrics/src/prom.rs:L45`
- **.dropped()** — `crates/mev-metrics/src/prom.rs:L123`
- **label_values_are_escaped_and_bounded()** — `crates/mev-metrics/src/prom.rs:L243`
- **Self** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C091 - events.rs|events.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C064 - registry.rs|registry.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C008 - Metrics|Metrics]] — 2 cross-community edges
- [[docs/knowledge/communities/C107 - AdvisorSnapshot|AdvisorSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C299 - .add_collector|.add_collector]] — 1 cross-community edges
