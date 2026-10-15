---
graphify-generated: true
graphify-community: "27"
graphify-node-count: 50
graphify-internal-edges: 153
tags:
  - graphify/community
---

# advisor.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/tests/advisor.rs` — 42 nodes
- `graph` — 8 nodes

## Nodes (50)

- **advisor.rs** — `crates/mev-liq-state/tests/advisor.rs:L1`
- **enqueue()** — `crates/mev-liq-state/tests/advisor.rs:L102`
- **.new()** — `crates/mev-liq-state/tests/advisor.rs:L22`
- **.store()** — `crates/mev-liq-state/tests/advisor.rs:L33`
- **live()** — `crates/mev-liq-state/tests/advisor.rs:L580`
- **qcfg()** — `crates/mev-liq-state/tests/advisor.rs:L38`
- **Env** — `crates/mev-liq-state/tests/advisor.rs:L16`
- **filled()** — `crates/mev-liq-state/tests/advisor.rs:L345`
- **snap()** — `crates/mev-liq-state/tests/advisor.rs:L71`
- **decision()** — `crates/mev-liq-state/tests/advisor.rs:L119`
- **json()** — `crates/mev-liq-state/tests/advisor.rs:L98`
- **raw()** — `crates/mev-liq-state/tests/advisor.rs:L145`
- **bounded_log_sheds_oldest_with_counted_record_and_advisor_counts_missed()** — `crates/mev-liq-state/tests/advisor.rs:L295`
- **claim_is_exclusive_across_handles_and_threads()** — `crates/mev-liq-state/tests/advisor.rs:L375`
- **failure_after_cas_rolls_back_snapshot_and_export_together()** — `crates/mev-liq-state/tests/advisor.rs:L210`
- **ocfg()** — `crates/mev-liq-state/tests/advisor.rs:L48`
- **stale_cas_enqueues_nothing()** — `crates/mev-liq-state/tests/advisor.rs:L272`
- **.adv_path()** — `crates/mev-liq-state/tests/advisor.rs:L30`
- **.snap_path()** — `crates/mev-liq-state/tests/advisor.rs:L27`
- **a_digest()** — `crates/mev-liq-state/tests/advisor.rs:L181`
- **advisor_schema_rolls_back_and_reapplies()** — `crates/mev-liq-state/tests/advisor.rs:L871`
- **crash_restart_restores_unacked_work_and_watermark()** — `crates/mev-liq-state/tests/advisor.rs:L462`
- **expired_ack_is_refused_and_max_in_flight_is_enforced()** — `crates/mev-liq-state/tests/advisor.rs:L440`
- **expired_digest_policy_chain_model_and_nan_are_rejected()** — `crates/mev-liq-state/tests/advisor.rs:L661`
- **key()** — `crates/mev-liq-state/tests/advisor.rs:L63`
- **migration_4_up_down_preserves_v1_to_v3_data()** — `crates/mev-liq-state/tests/advisor.rs:L846`
- **poison_row_that_keeps_expiring_is_dead_lettered()** — `crates/mev-liq-state/tests/advisor.rs:L532`
- **pull_is_idempotent_and_respects_max_pending()** — `crates/mev-liq-state/tests/advisor.rs:L356`
- **retry_backoff_and_dead_letter_schedule_is_exact()** — `crates/mev-liq-state/tests/advisor.rs:L490`
- **stale_future_and_newer_revision_race_are_rejected()** — `crates/mev-liq-state/tests/advisor.rs:L628`
- **tables()** — `crates/mev-liq-state/tests/advisor.rs:L835`
- **account_updated_without_export_has_no_current_export()** — `crates/mev-liq-state/tests/advisor.rs:L751`
- **apply_decision_happy_path_and_read_helper()** — `crates/mev-liq-state/tests/advisor.rs:L588`
- **dcfg()** — `crates/mev-liq-state/tests/advisor.rs:L54`
- **enqueue_rejects_json_leaking_account_addresses_before_any_write()** — `crates/mev-liq-state/tests/advisor.rs:L186`
- **enqueue_stores_snapshot_and_export_and_dedups_by_event_key()** — `crates/mev-liq-state/tests/advisor.rs:L152`
- **one_decision_per_key_and_refresh_on_conflict()** — `crates/mev-liq-state/tests/advisor.rs:L768`
- **Connection** — `graph`
- **PathBuf** — `graph`
- **policy_outcome_round_trips()** — `crates/mev-liq-state/tests/advisor.rs:L810`
- **dead_letter_requires_the_lease_and_is_final()** — `crates/mev-liq-state/tests/advisor.rs:L560`
- **expired_lease_is_reclaimed_and_old_holder_cannot_ack()** — `crates/mev-liq-state/tests/advisor.rs:L415`
- **release_refunds_the_attempt_and_is_immediately_claimable()** — `crates/mev-liq-state/tests/advisor.rs:L547`
- **String** — `graph`
- **B256** — `graph`
- **reader_requires_schema_4_and_never_creates_files()** — `crates/mev-liq-state/tests/advisor.rs:L884`
- **Self** — `graph`
- **SnapshotRepository** — `graph`
- **TempDir** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C006 - Result|Result]] — 11 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C093 - Strategy|Strategy]] — 1 cross-community edges
