---
graphify-generated: true
graphify-community: "33"
graphify-node-count: 76
graphify-internal-edges: 165
tags:
  - graphify/community
---

# lib.rs (C033)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 20 nodes
- `crates/mev-state/src/lib.rs` — 16 nodes
- `crates/mev-dex/src/lib.rs` — 15 nodes
- `crates/mev-search/src/lib.rs` — 11 nodes
- `crates/mev-types/src/lib.rs` — 8 nodes
- `crates/mev-analytics/src/lib.rs` — 3 nodes
- `crates/mev-bindings/src/lib.rs` — 3 nodes

## Nodes (76)

- **PoolStore** — `crates/mev-state/src/lib.rs:L11`
- **Pool** — `crates/mev-dex/src/lib.rs:L20`
- **lib.rs** — `crates/mev-search/src/lib.rs:L1`
- **PoolInfo** — `crates/mev-types/src/lib.rs:L34`
- **Opportunity** — `crates/mev-types/src/lib.rs:L69`
- **Address** — `graph`
- **two_pool_arbs()** — `crates/mev-search/src/lib.rs:L20`
- **.pools_for()** — `crates/mev-state/src/lib.rs:L57`
- **best_route()** — `crates/mev-search/src/lib.rs:L74`
- **lib.rs** — `crates/mev-dex/src/lib.rs:L1`
- **.new()** — `crates/mev-dex/src/lib.rs:L29`
- **Hop** — `crates/mev-types/src/lib.rs:L58`
- **info()** — `crates/mev-dex/src/lib.rs:L131`
- **.amount_out()** — `crates/mev-dex/src/lib.rs:L78`
- **.get()** — `crates/mev-state/src/lib.rs:L29`
- **.siblings()** — `crates/mev-state/src/lib.rs:L50`
- **Address** — `graph`
- **run()** — `crates/mev-search/src/lib.rs:L163`
- **SearchParams** — `crates/mev-search/src/lib.rs:L10`
- **.apply_log()** — `crates/mev-state/src/lib.rs:L78`
- **.rate()** — `crates/mev-state/src/lib.rs:L131`
- **.reserves()** — `crates/mev-dex/src/lib.rs:L52`
- **Address** — `graph`
- **Address** — `graph`
- **PoolState** — `crates/mev-dex/src/lib.rs:L11`
- **v2()** — `crates/mev-search/src/lib.rs:L146`
- **.fee()** — `crates/mev-dex/src/lib.rs:L69`
- **.marginal_rate()** — `crates/mev-dex/src/lib.rs:L88`
- **.pools()** — `crates/mev-state/src/lib.rs:L45`
- **.usd_per_raw_unit()** — `crates/mev-state/src/lib.rs:L110`
- **lib.rs** — `crates/mev-analytics/src/lib.rs:L1`
- **lib.rs** — `crates/mev-types/src/lib.rs:L1`
- **Option** — `graph`
- **pair_key()** — `crates/mev-state/src/lib.rs:L16`
- **pair_spreads()** — `crates/mev-analytics/src/lib.rs:L19`
- **PoolKind** — `crates/mev-types/src/lib.rs:L8`
- **v2_matches_uniswap_formula()** — `crates/mev-dex/src/lib.rs:L142`
- **v3_virtual_reserves_give_spot_price()** — `crates/mev-dex/src/lib.rs:L157`
- **Vec** — `graph`
- **.addresses()** — `crates/mev-state/src/lib.rs:L41`
- **.insert()** — `crates/mev-state/src/lib.rs:L21`
- **.reserve_of()** — `crates/mev-dex/src/lib.rs:L112`
- **.set_state()** — `crates/mev-state/src/lib.rs:L71`
- **best_route_prefers_deeper_pool()** — `crates/mev-search/src/lib.rs:L189`
- **best_size()** — `crates/mev-search/src/lib.rs:L125`
- **empty_pool_returns_zero()** — `crates/mev-dex/src/lib.rs:L172`
- **finds_arb_when_prices_diverge()** — `crates/mev-search/src/lib.rs:L175`
- **hop()** — `crates/mev-search/src/lib.rs:L110`
- **lib.rs** — `crates/mev-state/src/lib.rs:L1`
- **PairSpread** — `crates/mev-analytics/src/lib.rs:L8`
- **Vec** — `graph`
- **.has()** — `crates/mev-types/src/lib.rs:L51`
- **.other()** — `crates/mev-types/src/lib.rs:L43`
- **.replace()** — `crates/mev-state/src/lib.rs:L65`
- **.with_directional_fees()** — `crates/mev-dex/src/lib.rs:L45`
- **Address** — `graph`
- **Fn** — `graph`
- **HashMap** — `graph`
- **HashMap** — `graph`
- **lib.rs** — `crates/mev-bindings/src/lib.rs:L1`
- **no_arb_when_prices_match()** — `crates/mev-search/src/lib.rs:L200`
- **Option** — `graph`
- **Self** — `graph`
- **to_sol_call()** — `crates/mev-bindings/src/lib.rs:L139`
- **to_sol_hop()** — `crates/mev-bindings/src/lib.rs:L128`
- **.hop_kind()** — `crates/mev-types/src/lib.rs:L23`
- **.is_empty()** — `crates/mev-state/src/lib.rs:L37`
- **.len()** — `crates/mev-state/src/lib.rs:L33`
- **.price0()** — `crates/mev-dex/src/lib.rs:L98`
- **Item** — `graph`
- **Iterator** — `graph`
- **Log** — `graph`
- **String** — `graph`
- **U256** — `graph`
- **Vec** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 10 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 9 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 2 cross-community edges
- [[docs/knowledge/communities/C020 - Simulator|Simulator]] — 1 cross-community edges
- [[docs/knowledge/communities/C025 - lib.rs (C025)|lib.rs (C025)]] — 1 cross-community edges
