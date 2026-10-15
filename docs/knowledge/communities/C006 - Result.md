---
graphify-generated: true
graphify-community: "6"
graphify-node-count: 79
graphify-internal-edges: 264
tags:
  - graphify/community
---

# Result

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/src/advisor.rs` — 64 nodes
- `graph` — 13 nodes
- `crates/mev-liq-state/src/repo.rs` — 2 nodes

## Nodes (79)

- **Result** — `graph`
- **AdvisorError** — `crates/mev-liq-state/src/advisor.rs:L386`
- **AdvisorStore** — `crates/mev-liq-state/src/advisor.rs:L655`
- **advisor.rs** — `crates/mev-liq-state/src/advisor.rs:L1`
- **i64_of()** — `crates/mev-liq-state/src/repo.rs:L124`
- **u64_of()** — `crates/mev-liq-state/src/repo.rs:L127`
- **.conn()** — `crates/mev-liq-state/src/advisor.rs:L691`
- **AdvisoryDecision** — `crates/mev-liq-state/src/advisor.rs:L600`
- **.enqueue_with_snapshot()** — `crates/mev-liq-state/src/advisor.rs:L157`
- **.valid_decision_for()** — `crates/mev-liq-state/src/advisor.rs:L1173`
- **.current_export()** — `crates/mev-liq-state/src/advisor.rs:L315`
- **SnapshotReader** — `crates/mev-liq-state/src/advisor.rs:L281`
- **.apply_decision()** — `crates/mev-liq-state/src/advisor.rs:L1019`
- **.claim()** — `crates/mev-liq-state/src/advisor.rs:L808`
- **.export_after()** — `crates/mev-liq-state/src/advisor.rs:L334`
- **.pull()** — `crates/mev-liq-state/src/advisor.rs:L750`
- **.commit_decision()** — `crates/mev-liq-state/src/advisor.rs:L1075`
- **.fail()** — `crates/mev-liq-state/src/advisor.rs:L906`
- **.fetch_by_key()** — `crates/mev-liq-state/src/advisor.rs:L310`
- **.stats()** — `crates/mev-liq-state/src/advisor.rs:L979`
- **EnqueueOutcome** — `crates/mev-liq-state/src/advisor.rs:L88`
- **.policy_outcomes()** — `crates/mev-liq-state/src/advisor.rs:L1301`
- **DecisionConfig** — `crates/mev-liq-state/src/advisor.rs:L622`
- **PolicyOutcome** — `crates/mev-liq-state/src/advisor.rs:L638`
- **.open()** — `crates/mev-liq-state/src/advisor.rs:L668`
- **B256** — `graph`
- **Claimed** — `crates/mev-liq-state/src/advisor.rs:L517`
- **Connection** — `graph`
- **held_attempts()** — `crates/mev-liq-state/src/advisor.rs:L1406`
- **shed_over_bound()** — `crates/mev-liq-state/src/advisor.rs:L238`
- **shed_total_on()** — `crates/mev-liq-state/src/advisor.rs:L231`
- **validate_export()** — `crates/mev-liq-state/src/advisor.rs:L119`
- **validate_shape()** — `crates/mev-liq-state/src/advisor.rs:L1362`
- **.dead_letter()** — `crates/mev-liq-state/src/advisor.rs:L958`
- **.decision_count()** — `crates/mev-liq-state/src/advisor.rs:L1256`
- **.release()** — `crates/mev-liq-state/src/advisor.rs:L942`
- **meta_get()** — `crates/mev-liq-state/src/advisor.rs:L1424`
- **Option** — `graph`
- **QueueConfig** — `crates/mev-liq-state/src/advisor.rs:L497`
- **String** — `graph`
- **.ack()** — `crates/mev-liq-state/src/advisor.rs:L889`
- **.export_len()** — `crates/mev-liq-state/src/advisor.rs:L218`
- **.migrate_up()** — `crates/mev-liq-state/src/advisor.rs:L695`
- **.open()** — `crates/mev-liq-state/src/advisor.rs:L287`
- **.parse()** — `crates/mev-liq-state/src/advisor.rs:L589`
- **.record_policy_outcome()** — `crates/mev-liq-state/src/advisor.rs:L1271`
- **.shed_total()** — `crates/mev-liq-state/src/advisor.rs:L226`
- **.shed_total()** — `crates/mev-liq-state/src/advisor.rs:L376`
- **ExportRow** — `crates/mev-liq-state/src/advisor.rs:L268`
- **meta_set()** — `crates/mev-liq-state/src/advisor.rs:L1434`
- **opt_i64()** — `crates/mev-liq-state/src/advisor.rs:L661`
- **QueueStats** — `crates/mev-liq-state/src/advisor.rs:L543`
- **Vec** — `graph`
- **.parse()** — `crates/mev-liq-state/src/advisor.rs:L566`
- **.rollback_to()** — `crates/mev-liq-state/src/advisor.rs:L726`
- **.schema_version()** — `crates/mev-liq-state/src/advisor.rs:L737`
- **DecisionMode** — `crates/mev-liq-state/src/advisor.rs:L577`
- **event_key()** — `crates/mev-liq-state/src/advisor.rs:L99`
- **ExportItem** — `crates/mev-liq-state/src/advisor.rs:L74`
- **OutboxConfig** — `crates/mev-liq-state/src/advisor.rs:L83`
- **Self** — `graph`
- **WatchTier** — `crates/mev-liq-state/src/advisor.rs:L552`
- **.conn()** — `crates/mev-liq-state/src/advisor.rs:L306`
- **ApplyOutcome** — `crates/mev-liq-state/src/advisor.rs:L630`
- **Error** — `graph`
- **Path** — `graph`
- **SnapshotRepository** — `crates/mev-liq-state/src/advisor.rs:L149`
- **.backoff_ns()** — `crates/mev-liq-state/src/advisor.rs:L507`
- **.reader()** — `crates/mev-liq-state/src/advisor.rs:L687`
- **FailOutcome** — `crates/mev-liq-state/src/advisor.rs:L530`
- **Mutex** — `graph`
- **MutexGuard** — `graph`
- **PullOutcome** — `crates/mev-liq-state/src/advisor.rs:L536`
- **snapshot_reader_cannot_write()** — `crates/mev-liq-state/src/advisor.rs:L1450`
- **Transaction** — `graph`
- **.as_str()** — `crates/mev-liq-state/src/advisor.rs:L559`
- **.as_str()** — `crates/mev-liq-state/src/advisor.rs:L583`
- **high_water_refuses_older_revision_after_newer_commit()** — `crates/mev-liq-state/src/advisor.rs:L1474`
- **U256** — `graph`

## Related communities

- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 37 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 14 cross-community edges
- [[docs/knowledge/communities/C015 - Worker|Worker]] — 11 cross-community edges
- [[docs/knowledge/communities/C027 - advisor.rs|advisor.rs]] — 11 cross-community edges
- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 9 cross-community edges
- [[docs/knowledge/communities/C142 - advisory_store.rs|advisory_store.rs]] — 6 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 6 cross-community edges
- [[docs/knowledge/communities/C107 - AdvisorSnapshot|AdvisorSnapshot]] — 2 cross-community edges
- [[docs/knowledge/communities/C199 - reasons|reasons]] — 1 cross-community edges
- [[docs/knowledge/communities/C024 - RuleSet|RuleSet]] — 1 cross-community edges
- [[docs/knowledge/communities/C049 - response.rs|response.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 1 cross-community edges
