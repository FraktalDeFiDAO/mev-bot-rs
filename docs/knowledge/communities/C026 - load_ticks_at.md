---
graphify-generated: true
graphify-community: "26"
graphify-node-count: 50
graphify-internal-edges: 95
tags:
  - graphify/community
---

# load_ticks_at

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-discovery/src/ticks.rs` — 26 nodes
- `graph` — 10 nodes
- `crates/mev-discovery/tests/exact_algebra_live.rs` — 2 nodes
- `crates/mev-discovery/tests/exact_v4_live.rs` — 2 nodes
- `crates/mev-discovery/tests/exact_vs_quoter_live.rs` — 2 nodes
- `crates/mev-discovery/tests/route_vs_executor_live.rs` — 2 nodes
- `crates/mev-discovery/tests/ticks_live.rs` — 2 nodes
- `crates/mev-discovery/tests/v4_ticks_live.rs` — 2 nodes
- `crates/mev-bindings/src/lib.rs` — 1 node
- `crates/mev-dex/src/lib.rs` — 1 node

## Nodes (50)

- **load_ticks_at()** — `crates/mev-discovery/src/ticks.rs:L215`
- **ticks.rs** — `crates/mev-discovery/src/ticks.rs:L1`
- **load_ticks()** — `crates/mev-discovery/src/ticks.rs:L206`
- **Plan** — `crates/mev-discovery/src/ticks.rs:L92`
- **TickMap** — `crates/mev-dex/src/lib.rs:L29`
- **Option** — `graph`
- **Address** — `graph`
- **.bitmap_call()** — `crates/mev-discovery/src/ticks.rs:L112`
- **.tick_call()** — `crates/mev-discovery/src/ticks.rs:L133`
- **.finish()** — `crates/mev-discovery/src/ticks.rs:L177`
- **.parse_net()** — `crates/mev-discovery/src/ticks.rs:L167`
- **.parse_slot0()** — `crates/mev-discovery/src/ticks.rs:L157`
- **v4_slot_plus()** — `crates/mev-bindings/src/lib.rs:L247`
- **.slot0_call()** — `crates/mev-discovery/src/ticks.rs:L102`
- **current_tick()** — `crates/mev-discovery/src/ticks.rs:L54`
- **liquidity_net_exact()** — `crates/mev-discovery/src/ticks.rs:L41`
- **mapping_slot()** — `crates/mev-discovery/src/ticks.rs:L62`
- **Style** — `crates/mev-discovery/src/ticks.rs:L81`
- **ticks_in_word()** — `crates/mev-discovery/src/ticks.rs:L18`
- **v4_liquidity_net_exact()** — `crates/mev-discovery/src/ticks.rs:L75`
- **Vec** — `graph`
- **window()** — `crates/mev-discovery/src/ticks.rs:L27`
- **algebra_lower_bound_never_exceeds_the_quoter()** — `crates/mev-discovery/tests/exact_algebra_live.rs:L39`
- **Bytes** — `graph`
- **HashMap** — `graph`
- **integer_swap_math_equals_the_quoter()** — `crates/mev-discovery/tests/exact_vs_quoter_live.rs:L36`
- **liquidity_net()** — `crates/mev-discovery/src/ticks.rs:L36`
- **model_versus_executor_on_logged_routes()** — `crates/mev-discovery/tests/route_vs_executor_live.rs:L33`
- **Result** — `graph`
- **slot0_hint()** — `crates/mev-discovery/src/ticks.rs:L48`
- **tick_aware_pricing_matches_the_quoter_better_than_the_flat_model()** — `crates/mev-discovery/tests/ticks_live.rs:L28`
- **v4_integer_math_equals_the_quoter()** — `crates/mev-discovery/tests/exact_v4_live.rs:L29`
- **v4_liquidity_net()** — `crates/mev-discovery/src/ticks.rs:L71`
- **v4_tick_model_prices_swaps_across_ticks_like_the_quoter()** — `crates/mev-discovery/tests/v4_ticks_live.rs:L31`
- **a_flipped_pool_is_mirrored()** — `crates/mev-discovery/src/ticks.rs:L382`
- **B256** — `graph`
- **exact_algebra_live.rs** — `crates/mev-discovery/tests/exact_algebra_live.rs:L1`
- **the_window_is_centred_on_the_current_word()** — `crates/mev-discovery/src/ticks.rs:L339`
- **bitmap_bits_map_to_ticks_including_negative_words()** — `crates/mev-discovery/src/ticks.rs:L330`
- **BlockId** — `graph`
- **exact_v4_live.rs** — `crates/mev-discovery/tests/exact_v4_live.rs:L1`
- **exact_vs_quoter_live.rs** — `crates/mev-discovery/tests/exact_vs_quoter_live.rs:L1`
- **liquidity_net_reads_the_signed_second_word()** — `crates/mev-discovery/src/ticks.rs:L351`
- **mapping_slots_sign_extend_negative_keys()** — `crates/mev-discovery/src/ticks.rs:L361`
- **route_vs_executor_live.rs** — `crates/mev-discovery/tests/route_vs_executor_live.rs:L1`
- **ticks_live.rs** — `crates/mev-discovery/tests/ticks_live.rs:L1`
- **U256** — `graph`
- **v4_liquidity_net_is_the_signed_high_half()** — `crates/mev-discovery/src/ticks.rs:L374`
- **v4_ticks_live.rs** — `crates/mev-discovery/tests/v4_ticks_live.rs:L1`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C032 - Pool|Pool]] — 9 cross-community edges
- [[docs/knowledge/communities/C051 - Rpc|Rpc]] — 8 cross-community edges
- [[docs/knowledge/communities/C074 - lib.rs (C074)|lib.rs (C074)]] — 4 cross-community edges
- [[docs/knowledge/communities/C025 - PoolStore|PoolStore]] — 1 cross-community edges
- [[docs/knowledge/communities/C269 - applying_a_real_modify_liquidity_log_reproduces_the_managers_liquidity|applying_a_real_modify_liquidity_log_reproduces_the_managers_liquidity]] — 1 cross-community edges
- [[docs/knowledge/communities/C298 - Pool_flat|Pool_flat]] — 1 cross-community edges
