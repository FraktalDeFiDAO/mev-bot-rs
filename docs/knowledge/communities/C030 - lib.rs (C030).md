---
graphify-generated: true
graphify-community: "30"
graphify-node-count: 49
graphify-internal-edges: 154
tags:
  - graphify/community
---

# lib.rs (C030)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-state/src/lib.rs` — 47 nodes
- `graph` — 2 nodes

## Nodes (49)

- **lib.rs** — `crates/mev-state/src/lib.rs:L1`
- **.insert()** — `crates/mev-state/src/lib.rs:L138`
- **pool()** — `crates/mev-state/src/lib.rs:L889`
- **.apply_log()** — `crates/mev-state/src/lib.rs:L641`
- **v2_store()** — `crates/mev-state/src/lib.rs:L1633`
- **rpc_log()** — `crates/mev-state/src/lib.rs:L902`
- **v4_pool()** — `crates/mev-state/src/lib.rs:L1102`
- **.version()** — `crates/mev-state/src/lib.rs:L167`
- **v3_liquidity_log()** — `crates/mev-state/src/lib.rs:L1480`
- **sync_log()** — `crates/mev-state/src/lib.rs:L1622`
- **exact_v3_store()** — `crates/mev-state/src/lib.rs:L1945`
- **v4_swap_log()** — `crates/mev-state/src/lib.rs:L1142`
- **.versions_snapshot()** — `crates/mev-state/src/lib.rs:L172`
- **a_background_read_never_overwrites_a_pool_an_event_updated_since()** — `crates/mev-state/src/lib.rs:L1414`
- **a_chain_read_replaces_exact_state_and_a_superseded_read_cannot()** — `crates/mev-state/src/lib.rs:L2039`
- **out_of_range_v4_liquidity_changes_update_both_tick_boundaries_and_versions()** — `crates/mev-state/src/lib.rs:L1261`
- **v3_mint_and_burn_move_active_liquidity_only_when_the_price_is_in_range()** — `crates/mev-state/src/lib.rs:L1508`
- **v4_liquidity_log()** — `crates/mev-state/src/lib.rs:L1247`
- **v4_swap_and_liquidity_events_keep_the_exact_state_in_the_pools_own_orientation()** — `crates/mev-state/src/lib.rs:L2118`
- **a_flipped_v4_pool_stores_the_mirrored_price_and_fees()** — `crates/mev-state/src/lib.rs:L1188`
- **a_pool_version_moves_only_when_its_state_changes()** — `crates/mev-state/src/lib.rs:L1377`
- **a_sync_keeps_a_camelot_pairs_directional_fees()** — `crates/mev-state/src/lib.rs:L2078`
- **a_v4_swap_event_updates_the_pool_it_names_and_the_fee_of_its_direction()** — `crates/mev-state/src/lib.rs:L1158`
- **an_algebra_pool_keeps_its_fee_bounds_through_swaps_and_liquidity_events()** — `crates/mev-state/src/lib.rs:L2164`
- **liquidity_changes_apply_only_when_the_current_tick_is_inside_the_range()** — `crates/mev-state/src/lib.rs:L1319`
- **observe_only_reports_the_miss_without_touching_the_state()** — `crates/mev-state/src/lib.rs:L1864`
- **pool_with_addr()** — `crates/mev-state/src/lib.rs:L1123`
- **v3_mint_and_burn_keep_the_tick_table_exact()** — `crates/mev-state/src/lib.rs:L1556`
- **a_real_log_confirms_a_tentative_state_so_it_is_never_rolled_back()** — `crates/mev-state/src/lib.rs:L1768`
- **exact_state_follows_the_events_that_move_the_pool()** — `crates/mev-state/src/lib.rs:L1975`
- **journal_only_events_leave_pool_state_and_versions_alone()** — `crates/mev-state/src/lib.rs:L1899`
- **raw_integer_corrections_are_recorded_even_when_display_floats_match()** — `crates/mev-state/src/lib.rs:L1649`
- **stale_coherent_snapshot_cannot_replace_newer_price_or_ticks()** — `crates/mev-state/src/lib.rs:L1291`
- **v4_pools_are_watched_through_their_manager()** — `crates/mev-state/src/lib.rs:L1228`
- **a_sync_event_sets_exact_reserves()** — `crates/mev-state/src/lib.rs:L2026`
- **a_tentative_state_is_rolled_back_when_nothing_confirms_it()** — `crates/mev-state/src/lib.rs:L1716`
- **the_range_test_uses_the_pools_own_orientation_when_flipped()** — `crates/mev-state/src/lib.rs:L1357`
- **usd_costs_and_token_floors_use_raw_reserves_without_float_conversion()** — `crates/mev-state/src/lib.rs:L1684`
- **usd_price_direct_and_via_native()** — `crates/mev-state/src/lib.rs:L1070`
- **v4_exact_pool()** — `crates/mev-state/src/lib.rs:L2099`
- **applies_v2_sync()** — `crates/mev-state/src/lib.rs:L954`
- **applies_v3_and_pancake_swaps()** — `crates/mev-state/src/lib.rs:L985`
- **B256** — `graph`
- **ignores_unknown_pools_and_events()** — `crates/mev-state/src/lib.rs:L1039`
- **indexes_by_pair_and_ignores_duplicate_inserts()** — `crates/mev-state/src/lib.rs:L910`
- **v4_events_for_other_pools_or_other_emitters_are_ignored()** — `crates/mev-state/src/lib.rs:L1207`
- **events_for_unknown_pools_leave_no_tentative_state()** — `crates/mev-state/src/lib.rs:L1853`
- **LogData** — `graph`
- **rounding_noise_is_not_a_correction_but_a_real_change_is()** — `crates/mev-state/src/lib.rs:L1590`

## Related communities

- [[docs/knowledge/communities/C025 - PoolStore|PoolStore]] — 75 cross-community edges
- [[docs/knowledge/communities/C045 - PoolInfo|PoolInfo]] — 2 cross-community edges
- [[docs/knowledge/communities/C032 - Pool|Pool]] — 1 cross-community edges
