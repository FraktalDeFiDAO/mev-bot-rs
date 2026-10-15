---
graphify-generated: true
graphify-community: "173"
graphify-node-count: 14
graphify-internal-edges: 29
tags:
  - graphify/community
---

# history.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/tests/history.rs` — 11 nodes
- `graph` — 3 nodes

## Nodes (14)

- **history.rs** — `crates/mev-liq-state/tests/history.rs:L1`
- **canon()** — `crates/mev-liq-state/tests/history.rs:L51`
- **repo()** — `crates/mev-liq-state/tests/history.rs:L45`
- **key()** — `crates/mev-liq-state/tests/history.rs:L6`
- **snap()** — `crates/mev-liq-state/tests/history.rs:L18`
- **only_canonical_snapshots_enter_history()** — `crates/mev-liq-state/tests/history.rs:L63`
- **previous_canonical_never_returns_a_row_at_or_after_the_cutoff()** — `crates/mev-liq-state/tests/history.rs:L82`
- **migration_three_rolls_back_and_reapplies_without_touching_snapshots()** — `crates/mev-liq-state/tests/history.rs:L172`
- **reorg_removes_history_on_the_dropped_fork()** — `crates/mev-liq-state/tests/history.rs:L132`
- **retention_is_bounded_and_keeps_the_newest()** — `crates/mev-liq-state/tests/history.rs:L112`
- **hash()** — `crates/mev-liq-state/tests/history.rs:L14`
- **SnapshotRepository** — `graph`
- **B256** — `graph`
- **TempDir** — `graph`

## Related communities

- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C233 - trend_with|trend_with]] — 1 cross-community edges
