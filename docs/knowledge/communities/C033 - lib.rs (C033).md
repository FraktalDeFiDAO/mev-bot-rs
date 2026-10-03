---
graphify-generated: true
graphify-community: "33"
graphify-node-count: 62
graphify-internal-edges: 140
tags:
  - graphify/community
---

# lib.rs (C033)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-dex/src/lib.rs` — 16 nodes
- `crates/mev-state/src/lib.rs` — 16 nodes
- `graph` — 16 nodes
- `crates/mev-search/src/lib.rs` — 11 nodes
- `crates/mev-analytics/src/lib.rs` — 3 nodes

## Nodes (62)

- **PoolStore** — `crates/mev-state/src/lib.rs:L11`
- **Pool** — `crates/mev-dex/src/lib.rs:L20`
- **lib.rs** — `crates/mev-search/src/lib.rs:L1`
- **Address** — `graph`
- **two_pool_arbs()** — `crates/mev-search/src/lib.rs:L20`
- **.pools_for()** — `crates/mev-state/src/lib.rs:L57`
- **best_route()** — `crates/mev-search/src/lib.rs:L74`
- **lib.rs** — `crates/mev-dex/src/lib.rs:L1`
- **.new()** — `crates/mev-dex/src/lib.rs:L29`
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
- **PoolState** — `crates/mev-dex/src/lib.rs:L11`
- **v2()** — `crates/mev-search/src/lib.rs:L146`
- **.fee()** — `crates/mev-dex/src/lib.rs:L69`
- **.marginal_rate()** — `crates/mev-dex/src/lib.rs:L88`
- **.pools()** — `crates/mev-state/src/lib.rs:L45`
- **.usd_per_raw_unit()** — `crates/mev-state/src/lib.rs:L110`
- **lib.rs** — `crates/mev-analytics/src/lib.rs:L1`
- **Option** — `graph`
- **pair_key()** — `crates/mev-state/src/lib.rs:L16`
- **pair_spreads()** — `crates/mev-analytics/src/lib.rs:L19`
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
- **sqrt_price_from_x96()** — `crates/mev-dex/src/lib.rs:L119`
- **Vec** — `graph`
- **.replace()** — `crates/mev-state/src/lib.rs:L65`
- **.with_directional_fees()** — `crates/mev-dex/src/lib.rs:L45`
- **Address** — `graph`
- **Fn** — `graph`
- **HashMap** — `graph`
- **HashMap** — `graph`
- **no_arb_when_prices_match()** — `crates/mev-search/src/lib.rs:L200`
- **Option** — `graph`
- **Self** — `graph`
- **.is_empty()** — `crates/mev-state/src/lib.rs:L37`
- **.len()** — `crates/mev-state/src/lib.rs:L33`
- **.price0()** — `crates/mev-dex/src/lib.rs:L98`
- **Item** — `graph`
- **Iterator** — `graph`
- **Log** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C037 - PoolInfo|PoolInfo]] — 7 cross-community edges
- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 6 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 5 cross-community edges
- [[docs/knowledge/communities/C020 - Simulator|Simulator]] — 1 cross-community edges
