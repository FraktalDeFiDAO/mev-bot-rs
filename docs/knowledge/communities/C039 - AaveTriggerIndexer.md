---
graphify-generated: true
graphify-community: "39"
graphify-node-count: 45
graphify-internal-edges: 154
tags:
  - graphify/community
---

# AaveTriggerIndexer

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-lending-aave-v3/src/triggers.rs` — 35 nodes
- `graph` — 10 nodes

## Nodes (45)

- **AaveTriggerIndexer** — `crates/mev-lending-aave-v3/src/triggers.rs:L326`
- **.new()** — `crates/mev-lending-aave-v3/src/triggers.rs:L338`
- **Result** — `graph`
- **StepReport** — `crates/mev-lending-aave-v3/src/triggers.rs:L96`
- **.process()** — `crates/mev-lending-aave-v3/src/triggers.rs:L708`
- **.step()** — `crates/mev-lending-aave-v3/src/triggers.rs:L442`
- **triggers.rs** — `crates/mev-lending-aave-v3/src/triggers.rs:L1`
- **Address** — `graph`
- **now_ns()** — `crates/mev-lending-aave-v3/src/triggers.rs:L310`
- **apply_event()** — `crates/mev-lending-aave-v3/src/triggers.rs:L182`
- **.with_reserves()** — `crates/mev-lending-aave-v3/src/triggers.rs:L351`
- **.forward()** — `crates/mev-lending-aave-v3/src/triggers.rs:L616`
- **.heal_gaps()** — `crates/mev-lending-aave-v3/src/triggers.rs:L831`
- **.backfill()** — `crates/mev-lending-aave-v3/src/triggers.rs:L896`
- **.check_upgrade()** — `crates/mev-lending-aave-v3/src/triggers.rs:L527`
- **.skip_as_gap()** — `crates/mev-lending-aave-v3/src/triggers.rs:L678`
- **.backward_gap()** — `crates/mev-lending-aave-v3/src/triggers.rs:L950`
- **.check_reserves()** — `crates/mev-lending-aave-v3/src/triggers.rs:L576`
- **.poll_prices()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1028`
- **.query_parts()** — `crates/mev-lending-aave-v3/src/triggers.rs:L430`
- **PriceMarker** — `crates/mev-lending-aave-v3/src/triggers.rs:L1117`
- **Vec** — `graph`
- **.canonical_point()** — `crates/mev-lending-aave-v3/src/triggers.rs:L417`
- **.check_reorg()** — `crates/mev-lending-aave-v3/src/triggers.rs:L491`
- **.fetch_reserves()** — `crates/mev-lending-aave-v3/src/triggers.rs:L382`
- **.mark_assets_price_moved()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1085`
- **.mark_assets_price_moved()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1125`
- **.reconcile()** — `crates/mev-lending-aave-v3/src/triggers.rs:L973`
- **.import_known_borrowers()** — `crates/mev-lending-aave-v3/src/triggers.rs:L412`
- **Arc** — `graph`
- **deps_from_user_config()** — `crates/mev-lending-aave-v3/src/triggers.rs:L292`
- **SnapshotRepository** — `graph`
- **String** — `graph`
- **.marker()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1091`
- **.repo()** — `crates/mev-lending-aave-v3/src/triggers.rs:L370`
- **.run()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1102`
- **DynProvider** — `graph`
- **Mode** — `crates/mev-lending-aave-v3/src/triggers.rs:L116`
- **moved_enough()** — `crates/mev-lending-aave-v3/src/triggers.rs:L1145`
- **Option** — `graph`
- **Processed** — `crates/mev-lending-aave-v3/src/triggers.rs:L317`
- **.pool()** — `crates/mev-lending-aave-v3/src/triggers.rs:L374`
- **.reserves()** — `crates/mev-lending-aave-v3/src/triggers.rs:L378`
- **U256** — `graph`
- **BTreeSet** — `graph`

## Related communities

- [[docs/knowledge/communities/C170 - AaveConfig|AaveConfig]] — 14 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 7 cross-community edges
- [[docs/knowledge/communities/C232 - LogGuard|LogGuard]] — 7 cross-community edges
- [[docs/knowledge/communities/C111 - scanner.rs|scanner.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C017 - AaveV3Adapter|AaveV3Adapter]] — 3 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 3 cross-community edges
- [[docs/knowledge/communities/C047 - lib.rs (C047)|lib.rs (C047)]] — 2 cross-community edges
- [[docs/knowledge/communities/C098 - deployment.rs|deployment.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 1 cross-community edges
- [[docs/knowledge/communities/C176 - decode (C176)|decode (C176)]] — 1 cross-community edges
- [[docs/knowledge/communities/C070 - triggers.rs|triggers.rs]] — 1 cross-community edges
