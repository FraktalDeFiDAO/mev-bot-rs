---
graphify-generated: true
graphify-community: "18"
graphify-node-count: 37
graphify-internal-edges: 67
tags:
  - graphify/community
---

# BorrowerSnapshot

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 15 nodes
- `crates/mev-liq-state/src/domain.rs` — 12 nodes
- `crates/mev-lending-aave-v3/src/recheck.rs` — 7 nodes
- `crates/mev-liq-state/src/history.rs` — 3 nodes

## Nodes (37)

- **BorrowerSnapshot** — `crates/mev-liq-state/src/domain.rs:L125`
- **domain.rs** — `crates/mev-liq-state/src/domain.rs:L1`
- **.recheck_many()** — `crates/mev-lending-aave-v3/src/recheck.rs:L103`
- **AaveAccountRechecker** — `crates/mev-lending-aave-v3/src/recheck.rs:L29`
- **.recheck()** — `crates/mev-lending-aave-v3/src/recheck.rs:L47`
- **.snapshot()** — `crates/mev-lending-aave-v3/src/recheck.rs:L63`
- **ChainRef** — `crates/mev-liq-state/src/domain.rs:L38`
- **ReserveBalance** — `crates/mev-liq-state/src/domain.rs:L110`
- **.recheck_at_head()** — `crates/mev-lending-aave-v3/src/recheck.rs:L157`
- **record()** — `crates/mev-liq-state/src/history.rs:L41`
- **.compute_digest()** — `crates/mev-liq-state/src/domain.rs:L155`
- **Address** — `graph`
- **B256** — `graph`
- **history.rs** — `crates/mev-liq-state/src/history.rs:L1`
- **now_ns()** — `crates/mev-lending-aave-v3/src/recheck.rs:L37`
- **prune_reorg()** — `crates/mev-liq-state/src/history.rs:L79`
- **Result** — `graph`
- **.from_code()** — `crates/mev-liq-state/src/domain.rs:L27`
- **.from_code()** — `crates/mev-liq-state/src/domain.rs:L92`
- **.seal()** — `crates/mev-liq-state/src/domain.rs:L185`
- **Option** — `graph`
- **Result** — `graph`
- **Self** — `graph`
- **.code()** — `crates/mev-liq-state/src/domain.rs:L65`
- **.digest_matches()** — `crates/mev-liq-state/src/domain.rs:L190`
- **Address** — `graph`
- **Connection** — `graph`
- **recheck.rs** — `crates/mev-lending-aave-v3/src/recheck.rs:L1`
- **SnapshotStatus** — `crates/mev-liq-state/src/domain.rs:L117`
- **U256** — `graph`
- **.eligible()** — `crates/mev-liq-state/src/domain.rs:L147`
- **DynProvider** — `graph`
- **getUserAccountDataReturn** — `graph`
- **Option** — `graph`
- **String** — `graph`
- **Vec** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 16 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 6 cross-community edges
- [[docs/knowledge/communities/C006 - Result|Result]] — 6 cross-community edges
- [[docs/knowledge/communities/C233 - trend_with|trend_with]] — 4 cross-community edges
- [[docs/knowledge/communities/C048 - FeatureV1|FeatureV1]] — 4 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 4 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 3 cross-community edges
- [[docs/knowledge/communities/C044 - liq_shadow.rs|liq_shadow.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 3 cross-community edges
- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 2 cross-community edges
- [[docs/knowledge/communities/C067 - features.rs (C067)|features.rs (C067)]] — 2 cross-community edges
- [[docs/knowledge/communities/C100 - build_features|build_features]] — 1 cross-community edges
