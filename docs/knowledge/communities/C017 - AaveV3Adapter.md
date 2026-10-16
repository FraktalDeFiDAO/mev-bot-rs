---
graphify-generated: true
graphify-community: "17"
graphify-node-count: 53
graphify-internal-edges: 124
tags:
  - graphify/community
---

# AaveV3Adapter

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-lending-aave-v3/src/lib.rs` — 34 nodes
- `graph` — 19 nodes

## Nodes (53)

- **AaveV3Adapter** — `crates/mev-lending-aave-v3/src/lib.rs:L60`
- **Address** — `graph`
- **Vec** — `graph`
- **.new()** — `crates/mev-lending-aave-v3/src/lib.rs:L92`
- **Result** — `graph`
- **.build_plan()** — `crates/mev-lending-aave-v3/src/lib.rs:L458`
- **.plan()** — `crates/mev-lending-aave-v3/src/lib.rs:L572`
- **.positions_on()** — `crates/mev-lending-aave-v3/src/lib.rs:L364`
- **lib.rs** — `crates/mev-lending-aave-v3/src/lib.rs:L1`
- **.reserves_info_on()** — `crates/mev-lending-aave-v3/src/lib.rs:L313`
- **Option** — `graph`
- **.attach_triggers()** — `crates/mev-lending-aave-v3/src/lib.rs:L207`
- **.health()** — `crates/mev-lending-aave-v3/src/lib.rs:L537`
- **.position_from()** — `crates/mev-lending-aave-v3/src/lib.rs:L392`
- **.position()** — `crates/mev-lending-aave-v3/src/lib.rs:L356`
- **.predict()** — `crates/mev-lending-aave-v3/src/lib.rs:L578`
- **.refresh_snapshot()** — `crates/mev-lending-aave-v3/src/lib.rs:L426`
- **.reserves_info()** — `crates/mev-lending-aave-v3/src/lib.rs:L309`
- **Snapshot** — `crates/mev-lending-aave-v3/src/lib.rs:L84`
- **.account()** — `crates/mev-lending-aave-v3/src/lib.rs:L301`
- **.accounts()** — `crates/mev-lending-aave-v3/src/lib.rs:L530`
- **.on_price_update_tx()** — `crates/mev-lending-aave-v3/src/lib.rs:L257`
- **.price_update_assets()** — `crates/mev-lending-aave-v3/src/lib.rs:L248`
- **migrate_v1_cache()** — `crates/mev-lending-aave-v3/src/lib.rs:L620`
- **flash_amount()** — `crates/mev-lending-aave-v3/src/lib.rs:L493`
- **Position** — `graph`
- **.affected()** — `crates/mev-lending-aave-v3/src/lib.rs:L600`
- **.assets()** — `crates/mev-lending-aave-v3/src/lib.rs:L614`
- **.index()** — `crates/mev-lending-aave-v3/src/lib.rs:L509`
- **.price_feeds()** — `crates/mev-lending-aave-v3/src/lib.rs:L596`
- **.refresh_user_masks()** — `crates/mev-lending-aave-v3/src/lib.rs:L270`
- **.targets()** — `crates/mev-lending-aave-v3/src/lib.rs:L610`
- **.trigger_indexer()** — `crates/mev-lending-aave-v3/src/lib.rs:L228`
- **Plan** — `graph`
- **reserve_bits()** — `crates/mev-lending-aave-v3/src/lib.rs:L56`
- **U256** — `graph`
- **.id()** — `crates/mev-lending-aave-v3/src/lib.rs:L501`
- **.spawn_trigger_task()** — `crates/mev-lending-aave-v3/src/lib.rs:L236`
- **DynProvider** — `graph`
- **HashMap** — `graph`
- **v1_cache_is_migrated_to_market_account_pairs()** — `crates/mev-lending-aave-v3/src/lib.rs:L652`
- **Arc** — `graph`
- **Bytes** — `graph`
- **Duration** — `graph`
- **Mutex** — `graph`
- **Path** — `graph`
- **PathBuf** — `graph`
- **ProtocolId** — `graph`
- **reserve_bits_cover_borrow_and_collateral_flags()** — `crates/mev-lending-aave-v3/src/lib.rs:L686`
- **Self** — `graph`
- **Snapshot** — `graph`
- **SnapshotRepository** — `graph`
- **the_flash_amount_is_exact_above_f64_precision()** — `crates/mev-lending-aave-v3/src/lib.rs:L702`

## Related communities

- [[docs/knowledge/communities/C028 - AccountRef|AccountRef]] — 9 cross-community edges
- [[docs/knowledge/communities/C079 - math.rs (C079)|math.rs (C079)]] — 5 cross-community edges
- [[docs/knowledge/communities/C170 - AaveConfig|AaveConfig]] — 4 cross-community edges
- [[docs/knowledge/communities/C039 - AaveTriggerIndexer|AaveTriggerIndexer]] — 3 cross-community edges
- [[docs/knowledge/communities/C130 - AdapterStatus|AdapterStatus]] — 2 cross-community edges
- [[docs/knowledge/communities/C003 - CompoundV3Adapter|CompoundV3Adapter]] — 2 cross-community edges
- [[docs/knowledge/communities/C177 - topic_address|topic_address]] — 1 cross-community edges
- [[docs/knowledge/communities/C081 - Address|Address]] — 1 cross-community edges
- [[docs/knowledge/communities/C089 - EventIndex|EventIndex]] — 1 cross-community edges
