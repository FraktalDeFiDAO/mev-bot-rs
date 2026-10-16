---
graphify-generated: true
graphify-community: "3"
graphify-node-count: 94
graphify-internal-edges: 248
tags:
  - graphify/community
---

# CompoundV3Adapter

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-lending-compound-v3/src/lib.rs` — 34 nodes
- `crates/mev-lending-compound-v3/src/math.rs` — 20 nodes
- `graph` — 19 nodes
- `crates/mev-lending-aave-v3/tests/live.rs` — 8 nodes
- `crates/mev-lending-compound-v3/tests/live.rs` — 4 nodes
- `crates/mev-lending-morpho/tests/live.rs` — 4 nodes
- `crates/mev-lending-fluid/tests/live.rs` — 3 nodes
- `crates/mev-config/src/lib.rs` — 1 node
- `crates/mev-lending/src/lib.rs` — 1 node

## Nodes (94)

- **CompoundV3Adapter** — `crates/mev-lending-compound-v3/src/lib.rs:L107`
- **Address** — `graph`
- **AdapterRpc** — `crates/mev-lending/src/lib.rs:L46`
- **Market** — `crates/mev-lending-compound-v3/src/math.rs:L23`
- **.health()** — `crates/mev-lending-compound-v3/src/lib.rs:L625`
- **.plan()** — `crates/mev-lending-compound-v3/src/lib.rs:L694`
- **Comet** — `crates/mev-lending-compound-v3/src/lib.rs:L89`
- **math.rs** — `crates/mev-lending-compound-v3/src/math.rs:L1`
- **lib.rs** — `crates/mev-lending-compound-v3/src/lib.rs:L1`
- **Vec** — `graph`
- **.build_plan()** — `crates/mev-lending-compound-v3/src/lib.rs:L295`
- **.new()** — `crates/mev-lending-compound-v3/src/lib.rs:L202`
- **.positions()** — `crates/mev-lending-compound-v3/src/lib.rs:L522`
- **.asset()** — `crates/mev-lending-compound-v3/src/math.rs:L60`
- **.debtor_positions()** — `crates/mev-lending-compound-v3/src/lib.rs:L494`
- **.storefront_state()** — `crates/mev-lending-compound-v3/src/lib.rs:L451`
- **.health()** — `crates/mev-lending-compound-v3/src/math.rs:L69`
- **.predict()** — `crates/mev-lending-compound-v3/src/lib.rs:L745`
- **.quote()** — `crates/mev-lending-compound-v3/src/math.rs:L110`
- **.refresh_snapshot()** — `crates/mev-lending-compound-v3/src/lib.rs:L355`
- **Address** — `graph`
- **Result** — `graph`
- **.comet()** — `crates/mev-lending-compound-v3/src/lib.rs:L405`
- **.prices()** — `crates/mev-lending-compound-v3/src/lib.rs:L409`
- **.storefront()** — `crates/mev-lending-compound-v3/src/math.rs:L124`
- **cfg()** — `crates/mev-lending-aave-v3/tests/live.rs:L23`
- **CompoundV3Config** — `crates/mev-config/src/lib.rs:L205`
- **live.rs** — `crates/mev-lending-aave-v3/tests/live.rs:L1`
- **rpc()** — `crates/mev-lending-aave-v3/tests/live.rs:L11`
- **.best_collateral()** — `crates/mev-lending-compound-v3/src/math.rs:L92`
- **.discount()** — `crates/mev-lending-compound-v3/src/math.rs:L104`
- **.first()** — `crates/mev-lending-compound-v3/src/lib.rs:L142`
- **.market()** — `crates/mev-lending-compound-v3/src/lib.rs:L567`
- **.update()** — `crates/mev-lending-compound-v3/src/lib.rs:L156`
- **.value_units()** — `crates/mev-lending-compound-v3/src/math.rs:L64`
- **CometSnap** — `crates/mev-lending-compound-v3/src/lib.rs:L173`
- **Option** — `graph`
- **AssetParams** — `crates/mev-lending-compound-v3/src/math.rs:L12`
- **Elasticity** — `crates/mev-lending-compound-v3/src/lib.rs:L135`
- **Position** — `crates/mev-lending-compound-v3/src/math.rs:L33`
- **Prices** — `crates/mev-lending-compound-v3/src/lib.rs:L101`
- **Snapshot** — `crates/mev-lending-compound-v3/src/lib.rs:L122`
- **usdc_comet()** — `crates/mev-lending-compound-v3/src/math.rs:L149`
- **DynProvider** — `graph`
- **HashMap** — `graph`
- **Option** — `graph`
- **rpc()** — `crates/mev-lending-compound-v3/tests/live.rs:L9`
- **rpc()** — `crates/mev-lending-morpho/tests/live.rs:L14`
- **.accounts()** — `crates/mev-lending-compound-v3/src/lib.rs:L609`
- **.affected()** — `crates/mev-lending-compound-v3/src/lib.rs:L821`
- **.assets()** — `crates/mev-lending-compound-v3/src/lib.rs:L831`
- **.index()** — `crates/mev-lending-compound-v3/src/lib.rs:L598`
- **.price_feeds()** — `crates/mev-lending-compound-v3/src/lib.rs:L741`
- **.snapshot_summary()** — `crates/mev-lending-compound-v3/src/lib.rs:L281`
- **.targets()** — `crates/mev-lending-compound-v3/src/lib.rs:L827`
- **adapter_reads_real_reserves_feeds_and_a_real_borrower()** — `crates/mev-lending-aave-v3/tests/live.rs:L32`
- **Asset** — `crates/mev-lending-compound-v3/src/lib.rs:L80`
- **Buy** — `crates/mev-lending-compound-v3/src/lib.rs:L185`
- **eth_priced_comet_converts_to_usd()** — `crates/mev-lending-compound-v3/src/math.rs:L226`
- **every_price_dependency_of_every_reserve_is_watched()** — `crates/mev-lending-aave-v3/tests/live.rs:L164`
- **health_matches_comet_liquidation_condition()** — `crates/mev-lending-compound-v3/src/math.rs:L175`
- **learns_proportional_inverse_and_unrelated_responses()** — `crates/mev-lending-compound-v3/src/lib.rs:L859`
- **live.rs** — `crates/mev-lending-compound-v3/tests/live.rs:L1`
- **live.rs** — `crates/mev-lending-morpho/tests/live.rs:L1`
- **no_borrow_is_infinitely_healthy()** — `crates/mev-lending-compound-v3/src/math.rs:L198`
- **pinned_deployment_matches_chain()** — `crates/mev-lending-aave-v3/tests/live.rs:L233`
- **Plan** — `graph`
- **Position** — `graph`
- **predicts_liquidations_from_a_price_report_on_real_positions()** — `crates/mev-lending-aave-v3/tests/live.rs:L91`
- **quote_applies_storefront_discount()** — `crates/mev-lending-compound-v3/src/math.rs:L207`
- **rechecker_snapshot_is_exact_and_stable_at_a_pinned_block()** — `crates/mev-lending-aave-v3/tests/live.rs:L253`
- **rpc()** — `crates/mev-lending-fluid/tests/live.rs:L8`
- **Self** — `graph`
- **tiny_moves_keep_the_old_sample()** — `crates/mev-lending-compound-v3/src/lib.rs:L872`
- **.id()** — `crates/mev-lending-compound-v3/src/lib.rs:L590`
- **HealthOut** — `crates/mev-lending-compound-v3/src/math.rs:L39`
- **live.rs** — `crates/mev-lending-fluid/tests/live.rs:L1`
- **predicts_absorbs_from_a_price_report_on_real_positions()** — `crates/mev-lending-compound-v3/tests/live.rs:L67`
- **predicts_liquidations_from_a_price_report_on_real_positions()** — `crates/mev-lending-morpho/tests/live.rs:L75`
- **Quote** — `crates/mev-lending-compound-v3/src/math.rs:L46`
- **reads_a_real_wbtc_usdc_borrower()** — `crates/mev-lending-morpho/tests/live.rs:L28`
- **reads_all_comets_storefronts_and_builds_valid_plans()** — `crates/mev-lending-compound-v3/tests/live.rs:L23`
- **resolver_quotes_decode_for_configured_vaults()** — `crates/mev-lending-fluid/tests/live.rs:L21`
- **Storefront** — `crates/mev-lending-compound-v3/src/math.rs:L54`
- **Vec** — `graph`
- **best_collateral_is_highest_value()** — `crates/mev-lending-compound-v3/src/math.rs:L217`
- **DynProvider** — `graph`
- **HashSet** — `graph`
- **Mutex** — `graph`
- **PathBuf** — `graph`
- **plain_proxy_is_proportional_from_the_first_sample()** — `crates/mev-lending-compound-v3/src/lib.rs:L844`
- **ProtocolId** — `graph`
- **Snapshot** — `graph`
- **SweepSnap** — `graph`

## Related communities

- [[docs/knowledge/communities/C028 - AccountRef|AccountRef]] — 6 cross-community edges
- [[docs/knowledge/communities/C108 - ChainConfig|ChainConfig]] — 4 cross-community edges
- [[docs/knowledge/communities/C130 - AdapterStatus|AdapterStatus]] — 3 cross-community edges
- [[docs/knowledge/communities/C017 - AaveV3Adapter|AaveV3Adapter]] — 2 cross-community edges
- [[docs/knowledge/communities/C053 - MorphoAdapter|MorphoAdapter]] — 2 cross-community edges
- [[docs/knowledge/communities/C061 - FluidAdapter|FluidAdapter]] — 2 cross-community edges
- [[docs/knowledge/communities/C177 - topic_address|topic_address]] — 1 cross-community edges
- [[docs/knowledge/communities/C086 - lib.rs (C086)|lib.rs (C086)]] — 1 cross-community edges
- [[docs/knowledge/communities/C089 - EventIndex|EventIndex]] — 1 cross-community edges
