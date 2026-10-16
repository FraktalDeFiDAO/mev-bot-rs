---
graphify-generated: true
graphify-community: "70"
graphify-node-count: 30
graphify-internal-edges: 77
tags:
  - graphify/community
---

# triggers.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-lending-aave-v3/tests/triggers.rs` — 25 nodes
- `graph` — 4 nodes
- `crates/mev-lending-index/tests/support/fake_node.rs` — 1 node

## Nodes (30)

- **triggers.rs** — `crates/mev-lending-aave-v3/tests/triggers.rs:L1`
- **a()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L48`
- **tcfg()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L374`
- **grow()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L423`
- **hash_of()** — `crates/mev-lending-index/tests/support/fake_node.rs:L50`
- **backfill_resumes_after_a_crash_between_ranges_with_no_gap_or_duplicate()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L747`
- **reasons()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L431`
- **rehome()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L54`
- **reserve_data_update_marks_only_holders_once_dependencies_are_reconciled()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L524`
- **upgrade_event_and_implementation_slot_both_raise_the_reverification_flag()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L810`
- **Value** — `graph`
- **fixtures()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L37`
- **fx()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L41`
- **removed_logs_are_never_applied_and_the_range_becomes_a_healable_gap()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L667`
- **reorg_drops_orphaned_discoveries_flags_touched_borrowers_and_invalidates_snapshots()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L571`
- **rpc_log()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L65`
- **decode_ignores_foreign_emitters_and_rejects_malformed_data()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L163`
- **event_effects_by_mode()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L215`
- **every_recorded_event_decodes_to_the_expected_variant_and_fields()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L72`
- **indexer_runs_in_its_own_task_while_the_marker_keeps_marking()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L947`
- **live_events_register_borrowers_mark_dirty_and_build_the_reverse_index()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L449`
- **oracle_source_events_and_feed_matches_mark_asset_holders_with_bounded_fanout()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L908`
- **Vec** — `graph`
- **Address** — `graph`
- **rate_limited_scan_records_a_gap_instead_of_aborting_and_heals_later()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L704`
- **single_429_is_absorbed_by_retry()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L736`
- **user_configuration_bitmap_becomes_collateral_and_debt_dependencies()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L318`
- **Log** — `graph`
- **log_guard_mirrors_engine_continuity_rules()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L184`
- **price_move_threshold_is_exact_integer_math()** — `crates/mev-lending-aave-v3/tests/triggers.rs:L337`

## Related communities

- [[docs/knowledge/communities/C170 - AaveConfig|AaveConfig]] — 10 cross-community edges
- [[docs/knowledge/communities/C112 - fake_node.rs|fake_node.rs]] — 5 cross-community edges
- [[docs/knowledge/communities/C119 - chain|chain]] — 4 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 1 cross-community edges
- [[docs/knowledge/communities/C039 - AaveTriggerIndexer|AaveTriggerIndexer]] — 1 cross-community edges
- [[docs/knowledge/communities/C009 - audit.rs|audit.rs]] — 1 cross-community edges
