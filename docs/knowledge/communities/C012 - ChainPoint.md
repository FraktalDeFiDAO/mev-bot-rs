---
graphify-generated: true
graphify-community: "12"
graphify-node-count: 60
graphify-internal-edges: 225
tags:
  - graphify/community
---

# ChainPoint

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/src/index.rs` — 50 nodes
- `graph` — 9 nodes
- `crates/mev-liq-state/src/domain.rs` — 1 node

## Nodes (60)

- **ChainPoint** — `crates/mev-liq-state/src/domain.rs:L47`
- **Result** — `graph`
- **Address** — `graph`
- **SnapshotRepository** — `crates/mev-liq-state/src/index.rs:L629`
- **index.rs** — `crates/mev-liq-state/src/index.rs:L1`
- **Option** — `graph`
- **apply_effect()** — `crates/mev-liq-state/src/index.rs:L484`
- **.dirty_entries_where()** — `crates/mev-liq-state/src/index.rs:L1370`
- **fan_out()** — `crates/mev-liq-state/src/index.rs:L415`
- **registered()** — `crates/mev-liq-state/src/index.rs:L308`
- **Vec** — `graph`
- **.drain_fanout()** — `crates/mev-liq-state/src/index.rs:L1186`
- **.rewind_scan()** — `crates/mev-liq-state/src/index.rs:L778`
- **Effect** — `crates/mev-liq-state/src/index.rs:L152`
- **ScanBatch** — `crates/mev-liq-state/src/index.rs:L197`
- **point_cols()** — `crates/mev-liq-state/src/index.rs:L290`
- **.mark_asset_holders()** — `crates/mev-liq-state/src/index.rs:L1312`
- **.record_price_moves()** — `crates/mev-liq-state/src/index.rs:L1270`
- **holders()** — `crates/mev-liq-state/src/index.rs:L381`
- **register()** — `crates/mev-liq-state/src/index.rs:L346`
- **.register_borrowers()** — `crates/mev-liq-state/src/index.rs:L958`
- **key()** — `crates/mev-liq-state/src/index.rs:L300`
- **set_flag_in()** — `crates/mev-liq-state/src/index.rs:L459`
- **.apply_scan()** — `crates/mev-liq-state/src/index.rs:L705`
- **.flags()** — `crates/mev-liq-state/src/index.rs:L923`
- **.reconcile_borrower()** — `crates/mev-liq-state/src/index.rs:L1123`
- **.scan_cursor()** — `crates/mev-liq-state/src/index.rs:L630`
- **.borrower()** — `crates/mev-liq-state/src/index.rs:L1015`
- **.from_code()** — `crates/mev-liq-state/src/index.rs:L111`
- **.open_gaps()** — `crates/mev-liq-state/src/index.rs:L869`
- **.oracle_price()** — `crates/mev-liq-state/src/index.rs:L1248`
- **.scan_ancestry()** — `crates/mev-liq-state/src/index.rs:L684`
- **.seed_prices()** — `crates/mev-liq-state/src/index.rs:L1337`
- **.set_flag()** — `crates/mev-liq-state/src/index.rs:L903`
- **.active_borrowers()** — `crates/mev-liq-state/src/index.rs:L1024`
- **.asset_borrowers()** — `crates/mev-liq-state/src/index.rs:L1044`
- **.borrower_dependencies()** — `crates/mev-liq-state/src/index.rs:L1072`
- **.dirty_entries()** — `crates/mev-liq-state/src/index.rs:L1364`
- **.import_borrowers()** — `crates/mev-liq-state/src/index.rs:L984`
- **.init_scan_cursor()** — `crates/mev-liq-state/src/index.rs:L663`
- **.list_unreconciled()** — `crates/mev-liq-state/src/index.rs:L1099`
- **ApplyOutcome** — `crates/mev-liq-state/src/index.rs:L238`
- **Connection** — `graph`
- **CoverageGap** — `crates/mev-liq-state/src/index.rs:L131`
- **ScanDir** — `crates/mev-liq-state/src/index.rs:L99`
- **.has_flag()** — `crates/mev-liq-state/src/index.rs:L952`
- **.new()** — `crates/mev-liq-state/src/index.rs:L218`
- **PoolFlag** — `crates/mev-liq-state/src/index.rs:L261`
- **ScanCursor** — `crates/mev-liq-state/src/index.rs:L123`
- **String** — `graph`
- **.clear_flag()** — `crates/mev-liq-state/src/index.rs:L915`
- **.fanout_pending()** — `crates/mev-liq-state/src/index.rs:L1239`
- **NewGap** — `crates/mev-liq-state/src/index.rs:L143`
- **BorrowerRecord** — `crates/mev-liq-state/src/index.rs:L252`
- **RewindOutcome** — `crates/mev-liq-state/src/index.rs:L245`
- **U256** — `graph`
- **ReconcileOutcome** — `crates/mev-liq-state/src/index.rs:L269`
- **Self** — `graph`
- **.code()** — `crates/mev-liq-state/src/index.rs:L105`
- **Fn** — `graph`

## Related communities

- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 53 cross-community edges
- [[docs/knowledge/communities/C006 - Result|Result]] — 14 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 9 cross-community edges
- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 8 cross-community edges
- [[docs/knowledge/communities/C039 - AaveTriggerIndexer|AaveTriggerIndexer]] — 7 cross-community edges
- [[docs/knowledge/communities/C040 - sched.rs|sched.rs]] — 6 cross-community edges
- [[docs/knowledge/communities/C080 - index.rs|index.rs]] — 5 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 4 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 3 cross-community edges
- [[docs/knowledge/communities/C071 - replay_with_policy|replay_with_policy]] — 3 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 1 cross-community edges
