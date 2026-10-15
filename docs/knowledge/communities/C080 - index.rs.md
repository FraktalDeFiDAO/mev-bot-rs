---
graphify-generated: true
graphify-community: "80"
graphify-node-count: 28
graphify-internal-edges: 117
tags:
  - graphify/community
---

# index.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/tests/index.rs` — 24 nodes
- `graph` — 4 nodes

## Nodes (28)

- **index.rs** — `crates/mev-liq-state/tests/index.rs:L1`
- **batch()** — `crates/mev-liq-state/tests/index.rs:L32`
- **pt()** — `crates/mev-liq-state/tests/index.rs:L18`
- **repo()** — `crates/mev-liq-state/tests/index.rs:L27`
- **borrow()** — `crates/mev-liq-state/tests/index.rs:L37`
- **pool()** — `crates/mev-liq-state/tests/index.rs:L9`
- **who()** — `crates/mev-liq-state/tests/index.rs:L12`
- **reverse_index_follows_collateral_toggles_and_fans_out_to_holders_only()** — `crates/mev-liq-state/tests/index.rs:L136`
- **asset()** — `crates/mev-liq-state/tests/index.rs:L15`
- **fanout_is_bounded_per_call_durable_and_drains_riskiest_first()** — `crates/mev-liq-state/tests/index.rs:L375`
- **reorg_rewind_drops_orphan_discoveries_flags_touched_borrowers_and_invalidates_snapshots()** — `crates/mev-liq-state/tests/index.rs:L551`
- **zero_debt_borrower_is_deactivated_and_leaves_the_index_until_it_borrows_again()** — `crates/mev-liq-state/tests/index.rs:L241`
- **dirty_entries_are_ordered_by_urgency_with_most_urgent_reason()** — `crates/mev-liq-state/tests/index.rs:L473`
- **migration_two_applies_and_rolls_back_without_touching_version_one_data()** — `crates/mev-liq-state/tests/index.rs:L67`
- **new_borrower_is_registered_and_marked_discovered_and_event_dirty()** — `crates/mev-liq-state/tests/index.rs:L98`
- **price_baselines_persist_and_moves_mark_holders()** — `crates/mev-liq-state/tests/index.rs:L684`
- **reconcile_refuses_data_older_than_the_newest_touching_event()** — `crates/mev-liq-state/tests/index.rs:L295`
- **unreconciled_borrowers_count_as_holders_of_everything()** — `crates/mev-liq-state/tests/index.rs:L220`
- **dirty()** — `crates/mev-liq-state/tests/index.rs:L53`
- **overload_escalates_to_all_borrowers_and_never_drops_a_trigger()** — `crates/mev-liq-state/tests/index.rs:L337`
- **cursor_ancestry_and_gaps_commit_atomically_with_the_marks()** — `crates/mev-liq-state/tests/index.rs:L498`
- **events_for_unknown_borrowers_are_ignored_unless_they_register()** — `crates/mev-liq-state/tests/index.rs:L119`
- **pool_flags_set_clear_and_survive_reopen()** — `crates/mev-liq-state/tests/index.rs:L657`
- **Address** — `graph`
- **coalescing_keeps_the_most_urgent_reason_and_latest_chain_ref()** — `crates/mev-liq-state/tests/index.rs:L311`
- **Vec** — `graph`
- **SnapshotRepository** — `graph`
- **TempDir** — `graph`

## Related communities

- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 5 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 1 cross-community edges
