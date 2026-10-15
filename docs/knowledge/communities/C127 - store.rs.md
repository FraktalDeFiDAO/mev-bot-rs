---
graphify-generated: true
graphify-community: "127"
graphify-node-count: 20
graphify-internal-edges: 49
tags:
  - graphify/community
---

# store.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/seqscope-store/tests/store.rs` — 18 nodes
- `graph` — 2 nodes

## Nodes (20)

- **store.rs** — `crates/seqscope-store/tests/store.rs:L1`
- **.new()** — `crates/seqscope-store/tests/store.rs:L33`
- **write()** — `crates/seqscope-store/tests/store.rs:L96`
- **row()** — `crates/seqscope-store/tests/store.rs:L50`
- **.drop()** — `crates/seqscope-store/tests/store.rs:L43`
- **Db** — `crates/seqscope-store/tests/store.rs:L31`
- **a_database_from_a_newer_build_is_refused()** — `crates/seqscope-store/tests/store.rs:L266`
- **pruning_removes_old_rows_only()** — `crates/seqscope-store/tests/store.rs:L281`
- **readers_work_while_the_writer_writes()** — `crates/seqscope-store/tests/store.rs:L301`
- **the_engine_feeds_the_store_end_to_end()** — `crates/seqscope-store/tests/store.rs:L340`
- **the_feed_side_never_waits_and_every_row_is_accounted_for()** — `crates/seqscope-store/tests/store.rs:L222`
- **bare()** — `crates/seqscope-store/tests/store.rs:L78`
- **free_form_sql_reads_but_never_writes()** — `crates/seqscope-store/tests/store.rs:L188`
- **queries_filter_order_and_limit()** — `crates/seqscope-store/tests/store.rs:L129`
- **rows_round_trip_exactly_including_missing_values()** — `crates/seqscope-store/tests/store.rs:L112`
- **rows_survive_closing_and_reopening_and_writes_append()** — `crates/seqscope-store/tests/store.rs:L258`
- **shutdown_does_not_wait_for_other_holders_of_the_sender_and_still_accounts_for_every_row()** — `crates/seqscope-store/tests/store.rs:L408`
- **sql_limits_bound_values_work_and_leave_the_connection_usable()** — `crates/seqscope-store/tests/store.rs:L15`
- **Drop** — `graph`
- **PathBuf** — `graph`

## Related communities

- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 1 cross-community edges
- [[docs/knowledge/communities/C056 - engine.rs|engine.rs]] — 1 cross-community edges
