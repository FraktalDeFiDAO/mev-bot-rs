---
graphify-generated: true
graphify-community: "33"
graphify-node-count: 83
graphify-internal-edges: 190
tags:
  - graphify/community
---

# lib.rs (C033)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-state/src/lib.rs` — 23 nodes
- `graph` — 19 nodes
- `crates/mev-dex/src/lib.rs` — 17 nodes
- `crates/mev-search/src/lib.rs` — 11 nodes
- `crates/mev-types/src/lib.rs` — 8 nodes
- `crates/mev-analytics/src/lib.rs` — 5 nodes

## Nodes (83)

- **PoolStore** — `crates/mev-state/src/lib.rs:L11`
- **Pool** — `crates/mev-dex/src/lib.rs:L20`
- **lib.rs** — `crates/mev-search/src/lib.rs:L1`
- **Address** — `graph`
- **PoolInfo** — `crates/mev-types/src/lib.rs:L34`
- **lib.rs** — `crates/mev-state/src/lib.rs:L1`
- **two_pool_arbs()** — `crates/mev-search/src/lib.rs:L20`
- **lib.rs** — `crates/mev-dex/src/lib.rs:L1`
- **pool()** — `crates/mev-state/src/lib.rs:L159`
- **.insert()** — `crates/mev-state/src/lib.rs:L21`
- **.new()** — `crates/mev-dex/src/lib.rs:L29`
- **.pools_for()** — `crates/mev-state/src/lib.rs:L57`
- **best_route()** — `crates/mev-search/src/lib.rs:L74`
- **lib.rs** — `crates/mev-analytics/src/lib.rs:L1`
- **info()** — `crates/mev-dex/src/lib.rs:L131`
- **PoolState** — `crates/mev-dex/src/lib.rs:L11`
- **.amount_out()** — `crates/mev-dex/src/lib.rs:L78`
- **.get()** — `crates/mev-state/src/lib.rs:L29`
- **.siblings()** — `crates/mev-state/src/lib.rs:L50`
- **Address** — `graph`
- **lib.rs** — `crates/mev-types/src/lib.rs:L1`
- **run()** — `crates/mev-search/src/lib.rs:L163`
- **SearchParams** — `crates/mev-search/src/lib.rs:L10`
- **.apply_log()** — `crates/mev-state/src/lib.rs:L78`
- **.rate()** — `crates/mev-state/src/lib.rs:L131`
- **.reserves()** — `crates/mev-dex/src/lib.rs:L52`
- **.usd_per_raw_unit()** — `crates/mev-state/src/lib.rs:L110`
- **Address** — `graph`
- **Address** — `graph`
- **pair_spreads()** — `crates/mev-analytics/src/lib.rs:L19`
- **PoolKind** — `crates/mev-types/src/lib.rs:L8`
- **v2()** — `crates/mev-search/src/lib.rs:L146`
- **.fee()** — `crates/mev-dex/src/lib.rs:L69`
- **.marginal_rate()** — `crates/mev-dex/src/lib.rs:L88`
- **.pools()** — `crates/mev-state/src/lib.rs:L45`
- **Option** — `graph`
- **pair_key()** — `crates/mev-state/src/lib.rs:L16`
- **rpc_log()** — `crates/mev-state/src/lib.rs:L172`
- **usd_price_direct_and_via_native()** — `crates/mev-state/src/lib.rs:L340`
- **v2_matches_uniswap_formula()** — `crates/mev-dex/src/lib.rs:L142`
- **v3_virtual_reserves_give_spot_price()** — `crates/mev-dex/src/lib.rs:L157`
- **Vec** — `graph`
- **.addresses()** — `crates/mev-state/src/lib.rs:L41`
- **.reserve_of()** — `crates/mev-dex/src/lib.rs:L112`
- **.set_state()** — `crates/mev-state/src/lib.rs:L71`
- **applies_v2_sync()** — `crates/mev-state/src/lib.rs:L224`
- **applies_v3_and_pancake_swaps()** — `crates/mev-state/src/lib.rs:L255`
- **best_route_prefers_deeper_pool()** — `crates/mev-search/src/lib.rs:L189`
- **best_size()** — `crates/mev-search/src/lib.rs:L125`
- **detects_positive_and_negative_edges()** — `crates/mev-analytics/src/lib.rs:L83`
- **empty_pool_returns_zero()** — `crates/mev-dex/src/lib.rs:L172`
- **finds_arb_when_prices_diverge()** — `crates/mev-search/src/lib.rs:L175`
- **hop()** — `crates/mev-search/src/lib.rs:L110`
- **ignores_unknown_pools_and_events()** — `crates/mev-state/src/lib.rs:L309`
- **indexes_by_pair_and_ignores_duplicate_inserts()** — `crates/mev-state/src/lib.rs:L180`
- **PairSpread** — `crates/mev-analytics/src/lib.rs:L8`
- **sqrt_price_from_x96()** — `crates/mev-dex/src/lib.rs:L119`
- **v2()** — `crates/mev-analytics/src/lib.rs:L65`
- **v2()** — `crates/mev-dex/src/lib.rs:L193`
- **Vec** — `graph`
- **.has()** — `crates/mev-types/src/lib.rs:L51`
- **.other()** — `crates/mev-types/src/lib.rs:L43`
- **.replace()** — `crates/mev-state/src/lib.rs:L65`
- **.with_directional_fees()** — `crates/mev-dex/src/lib.rs:L45`
- **Address** — `graph`
- **Fn** — `graph`
- **HashMap** — `graph`
- **HashMap** — `graph`
- **Log** — `graph`
- **no_arb_when_prices_match()** — `crates/mev-search/src/lib.rs:L200`
- **Option** — `graph`
- **Self** — `graph`
- **.hop_kind()** — `crates/mev-types/src/lib.rs:L23`
- **.is_empty()** — `crates/mev-state/src/lib.rs:L37`
- **.len()** — `crates/mev-state/src/lib.rs:L33`
- **.price0()** — `crates/mev-dex/src/lib.rs:L98`
- **hop_kinds_match_executor_constants()** — `crates/mev-types/src/lib.rs:L86`
- **Item** — `graph`
- **Iterator** — `graph`
- **LogData** — `graph`
- **pool_info_other_and_has()** — `crates/mev-types/src/lib.rs:L96`
- **String** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 10 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 7 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 5 cross-community edges
- [[docs/knowledge/communities/C020 - Simulator|Simulator]] — 1 cross-community edges
