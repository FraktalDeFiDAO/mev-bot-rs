---
graphify-generated: true
graphify-community: "91"
graphify-node-count: 24
graphify-internal-edges: 53
tags:
  - graphify/community
---

# events.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-decode/src/events.rs` — 17 nodes
- `graph` — 5 nodes
- `crates/mev-metrics/src/prom.rs` — 2 nodes

## Nodes (24)

- **events.rs** — `crates/mev-decode/src/events.rs:L1`
- **kinds()** — `crates/mev-decode/src/events.rs:L12`
- **describe()** — `crates/mev-decode/src/events.rs:L53`
- **Kind** — `crates/mev-metrics/src/prom.rs:L20`
- **a_v3_mint_reads_the_owner_the_ticks_and_the_amounts()** — `crates/mev-decode/src/events.rs:L409`
- **a_v3_swap_keeps_the_sign_of_its_amounts_and_reads_the_tick()** — `crates/mev-decode/src/events.rs:L384`
- **addr_topic()** — `crates/mev-decode/src/events.rs:L371`
- **classify()** — `crates/mev-decode/src/events.rs:L37`
- **pool_event_topics()** — `crates/mev-decode/src/events.rs:L20`
- **word()** — `crates/mev-decode/src/events.rs:L368`
- **B256** — `graph`
- **price_update()** — `crates/mev-decode/src/events.rs:L43`
- **every_event_in_the_table_is_described_or_deliberately_has_no_fields()** — `crates/mev-decode/src/events.rs:L444`
- **the_pool_subscription_topics_cover_every_pool_event_and_no_other_emitter()** — `crates/mev-decode/src/events.rs:L478`
- **unknown_kinds_and_short_data_give_no_invented_fields()** — `crates/mev-decode/src/events.rs:L460`
- **Vec** — `graph`
- **every_kind_has_a_distinct_topic_and_name()** — `crates/mev-decode/src/events.rs:L311`
- **field()** — `crates/mev-decode/src/events.rs:L376`
- **Option** — `graph`
- **prices_are_routed_to_the_owning_protocol()** — `crates/mev-decode/src/events.rs:L356`
- **String** — `graph`
- **the_definitions_agree_with_the_bot_bindings()** — `crates/mev-decode/src/events.rs:L322`
- **U256** — `graph`
- **.as_str()** — `crates/mev-metrics/src/prom.rs:L27`

## Related communities

- [[docs/knowledge/communities/C075 - Exposition|Exposition]] — 3 cross-community edges
- [[docs/knowledge/communities/C115 - lib.rs (C115)|lib.rs (C115)]] — 1 cross-community edges
- [[docs/knowledge/communities/C072 - lib.rs (C072)|lib.rs (C072)]] — 1 cross-community edges
- [[docs/knowledge/communities/C092 - event_line|event_line]] — 1 cross-community edges
