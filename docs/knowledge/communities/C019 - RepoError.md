---
graphify-generated: true
graphify-community: "19"
graphify-node-count: 48
graphify-internal-edges: 147
tags:
  - graphify/community
---

# RepoError

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/src/repo.rs` — 28 nodes
- `graph` — 15 nodes
- `crates/mev-liq-state/src/history.rs` — 3 nodes
- `crates/mev-liq-state/src/domain.rs` — 1 node
- `crates/mev-liq-state/src/tiering.rs` — 1 node

## Nodes (48)

- **RepoError** — `crates/mev-liq-state/src/repo.rs:L86`
- **AccountKey** — `crates/mev-liq-state/src/domain.rs:L7`
- **Result** — `graph`
- **repo.rs** — `crates/mev-liq-state/src/repo.rs:L1`
- **mark_at()** — `crates/mev-liq-state/src/repo.rs:L282`
- **SnapshotRepository** — `crates/mev-liq-state/src/repo.rs:L120`
- **b256_of()** — `crates/mev-liq-state/src/repo.rs:L135`
- **.conn()** — `crates/mev-liq-state/src/repo.rs:L426`
- **load_all_keys()** — `crates/mev-liq-state/src/repo.rs:L251`
- **load()** — `crates/mev-liq-state/src/repo.rs:L213`
- **row_snapshot()** — `crates/mev-liq-state/src/repo.rs:L175`
- **invalidate_in_tx()** — `crates/mev-liq-state/src/repo.rs:L633`
- **upsert_cas_in()** — `crates/mev-liq-state/src/repo.rs:L322`
- **.mark_dirty()** — `crates/mev-liq-state/src/repo.rs:L529`
- **.previous_canonical()** — `crates/mev-liq-state/src/history.rs:L96`
- **decode_reserves()** — `crates/mev-liq-state/src/repo.rs:L157`
- **mark()** — `crates/mev-liq-state/src/repo.rs:L276`
- **u256_of()** — `crates/mev-liq-state/src/repo.rs:L130`
- **.checkpoint_canonical()** — `crates/mev-liq-state/src/repo.rs:L581`
- **.fetch_by_key()** — `crates/mev-liq-state/src/repo.rs:L483`
- **.list_expired()** — `crates/mev-liq-state/src/repo.rs:L505`
- **.list_dirty()** — `crates/mev-liq-state/src/repo.rs:L519`
- **.store_rechecks()** — `crates/mev-liq-state/src/tiering.rs:L303`
- **Connection** — `graph`
- **.invalidate_from_reorg()** — `crates/mev-liq-state/src/repo.rs:L619`
- **.open()** — `crates/mev-liq-state/src/repo.rs:L414`
- **.upsert_cas()** — `crates/mev-liq-state/src/repo.rs:L491`
- **.migrate_up()** — `crates/mev-liq-state/src/repo.rs:L430`
- **Vec** — `graph`
- **.history_len()** — `crates/mev-liq-state/src/history.rs:L137`
- **.rollback_to()** — `crates/mev-liq-state/src/repo.rs:L461`
- **.schema_version()** — `crates/mev-liq-state/src/repo.rs:L475`
- **DirtyTarget** — `crates/mev-liq-state/src/repo.rs:L109`
- **encode_reserves()** — `crates/mev-liq-state/src/repo.rs:L148`
- **Option** — `graph`
- **B256** — `graph`
- **Path** — `graph`
- **SnapshotRepository** — `crates/mev-liq-state/src/history.rs:L92`
- **Error** — `graph`
- **Migration** — `crates/mev-liq-state/src/repo.rs:L25`
- **Mutex** — `graph`
- **MutexGuard** — `graph`
- **Option** — `graph`
- **Self** — `graph`
- **String** — `graph`
- **ToSql** — `graph`
- **Transaction** — `graph`
- **U256** — `graph`

## Related communities

- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 53 cross-community edges
- [[docs/knowledge/communities/C006 - Result|Result]] — 37 cross-community edges
- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 17 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 16 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 8 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 3 cross-community edges
- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 3 cross-community edges
- [[docs/knowledge/communities/C039 - AaveTriggerIndexer|AaveTriggerIndexer]] — 3 cross-community edges
- [[docs/knowledge/communities/C040 - sched.rs|sched.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 3 cross-community edges
- [[docs/knowledge/communities/C071 - replay_with_policy|replay_with_policy]] — 2 cross-community edges
- [[docs/knowledge/communities/C101 - repo.rs|repo.rs]] — 1 cross-community edges
