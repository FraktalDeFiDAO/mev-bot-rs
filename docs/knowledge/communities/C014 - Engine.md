---
graphify-generated: true
graphify-community: "14"
graphify-node-count: 55
graphify-internal-edges: 158
tags:
  - graphify/community
---

# Engine

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-engine/src/lib.rs` — 41 nodes
- `graph` — 14 nodes

## Nodes (55)

- **Engine** — `crates/mev-engine/src/lib.rs:L175`
- **.on_pool_update()** — `crates/mev-engine/src/lib.rs:L2281`
- **Option** — `graph`
- **Address** — `graph`
- **.on_backrun()** — `crates/mev-engine/src/lib.rs:L2832`
- **.on_plan()** — `crates/mev-engine/src/lib.rs:L2969`
- **TxDone** — `crates/mev-engine/src/lib.rs:L303`
- **Vec** — `graph`
- **.insert()** — `crates/mev-engine/src/lib.rs:L3459`
- **.liq_precheck()** — `crates/mev-engine/src/lib.rs:L2774`
- **.shadow_floor()** — `crates/mev-engine/src/lib.rs:L1363`
- **.final_profit_floor()** — `crates/mev-engine/src/lib.rs:L1380`
- **.pga_bid_for()** — `crates/mev-engine/src/lib.rs:L1821`
- **.route()** — `crates/mev-engine/src/lib.rs:L1528`
- **.pga_bid()** — `crates/mev-engine/src/lib.rs:L1809`
- **.price()** — `crates/mev-engine/src/lib.rs:L2180`
- **.scan_once()** — `crates/mev-engine/src/lib.rs:L1471`
- **last_trigger_per_pool()** — `crates/mev-engine/src/lib.rs:L20`
- **LiqCtx** — `crates/mev-engine/src/lib.rs:L288`
- **.gas_price()** — `crates/mev-engine/src/lib.rs:L1571`
- **.heal_route()** — `crates/mev-engine/src/lib.rs:L1721`
- **.on_runner_event()** — `crates/mev-engine/src/lib.rs:L2749`
- **.price_integer()** — `crates/mev-engine/src/lib.rs:L1369`
- **.record_overwrite()** — `crates/mev-engine/src/lib.rs:L2067`
- **.revm_shadow_compare()** — `crates/mev-engine/src/lib.rs:L1391`
- **.shadow_candidate()** — `crates/mev-engine/src/lib.rs:L1454`
- **RecentHashes** — `crates/mev-engine/src/lib.rs:L3443`
- **ScanResult** — `crates/mev-engine/src/lib.rs:L294`
- **.apply_maint()** — `crates/mev-engine/src/lib.rs:L1999`
- **.native_price_integer()** — `crates/mev-engine/src/lib.rs:L1374`
- **TxHash** — `graph`
- **Value** — `graph`
- **.aave_flash_pool()** — `crates/mev-engine/src/lib.rs:L1883`
- **.claim_pools()** — `crates/mev-engine/src/lib.rs:L1603`
- **.reconcile_subscription()** — `crates/mev-engine/src/lib.rs:L1330`
- **.unvetted()** — `crates/mev-engine/src/lib.rs:L1706`
- **fetch_gas_price()** — `crates/mev-engine/src/lib.rs:L3434`
- **Plan** — `graph`
- **U256** — `graph`
- **.fundable()** — `crates/mev-engine/src/lib.rs:L1891`
- **.pools_busy()** — `crates/mev-engine/src/lib.rs:L1595`
- **.rebuild_cycles()** — `crates/mev-engine/src/lib.rs:L2174`
- **.superseded()** — `crates/mev-engine/src/lib.rs:L2092`
- **profit_floor()** — `crates/mev-engine/src/lib.rs:L3409`
- **route_json()** — `crates/mev-engine/src/lib.rs:L94`
- **.release_pools()** — `crates/mev-engine/src/lib.rs:L1613`
- **.roll_back_unconfirmed()** — `crates/mev-engine/src/lib.rs:L1969`
- **.pool_count()** — `crates/mev-engine/src/lib.rs:L1521`
- **Continuity** — `graph`
- **Duration** — `graph`
- **Instant** — `graph`
- **Mutex** — `graph`
- **PresimRx** — `graph`
- **VecDeque** — `graph`
- **Watchlist** — `graph`

## Related communities

- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 47 cross-community edges
- [[docs/knowledge/communities/C164 - Maint|Maint]] — 19 cross-community edges
- [[docs/knowledge/communities/C190 - now_secs|now_secs]] — 17 cross-community edges
- [[docs/knowledge/communities/C281 - .submission_guard_for|.submission_guard_for]] — 6 cross-community edges
- [[docs/knowledge/communities/C163 - Opportunity|Opportunity]] — 5 cross-community edges
- [[docs/knowledge/communities/C171 - Fees|Fees]] — 4 cross-community edges
- [[docs/knowledge/communities/C069 - Item|Item]] — 4 cross-community edges
- [[docs/knowledge/communities/C124 - lib.rs (C124)|lib.rs (C124)]] — 3 cross-community edges
- [[docs/knowledge/communities/C097 - lib.rs (C097)|lib.rs (C097)]] — 3 cross-community edges
- [[docs/knowledge/communities/C000 - liquidation.rs|liquidation.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C011 - gate.rs|gate.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C013 - Runner|Runner]] — 2 cross-community edges
