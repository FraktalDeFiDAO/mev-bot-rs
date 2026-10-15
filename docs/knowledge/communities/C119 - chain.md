---
graphify-generated: true
graphify-community: "119"
graphify-node-count: 20
graphify-internal-edges: 41
tags:
  - graphify/community
---

# chain

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-lending-index/tests/index.rs` — 11 nodes
- `crates/mev-lending-aave-v3/tests/recheck.rs` — 4 nodes
- `graph` — 4 nodes
- `crates/mev-lending-index/tests/support/fake_node.rs` — 1 node

## Nodes (20)

- **chain()** — `crates/mev-lending-index/tests/support/fake_node.rs:L323`
- **index.rs** — `crates/mev-lending-index/tests/index.rs:L1`
- **cfg()** — `crates/mev-lending-index/tests/index.rs:L39`
- **fast_retry()** — `crates/mev-lending-index/tests/index.rs:L52`
- **reorg_rewinds_the_cursor_to_the_common_ancestor_and_rescans()** — `crates/mev-lending-index/tests/index.rs:L132`
- **raw_log()** — `crates/mev-lending-index/tests/index.rs:L24`
- **resume_after_a_crash_between_range_steps_has_no_gap_or_duplicate()** — `crates/mev-lending-index/tests/index.rs:L175`
- **a_lagging_backend_that_lacks_the_tip_does_not_trigger_a_rewind()** — `crates/mev-lending-index/tests/index.rs:L223`
- **acct()** — `crates/mev-lending-index/tests/index.rs:L20`
- **exhausted_retries_record_gaps_instead_of_aborting_and_later_steps_heal_them()** — `crates/mev-lending-index/tests/index.rs:L85`
- **provider_range_limit_shrinks_without_creating_gaps()** — `crates/mev-lending-index/tests/index.rs:L115`
- **rate_limited_scan_retries_with_backoff_and_succeeds()** — `crates/mev-lending-index/tests/index.rs:L66`
- **recheck.rs** — `crates/mev-lending-aave-v3/tests/recheck.rs:L1`
- **Address** — `graph`
- **account()** — `crates/mev-lending-aave-v3/tests/recheck.rs:L19`
- **recheck_many_fails_whole_batch_when_the_request_fails()** — `crates/mev-lending-aave-v3/tests/recheck.rs:L98`
- **recheck_many_reads_every_account_at_one_lagged_block()** — `crates/mev-lending-aave-v3/tests/recheck.rs:L32`
- **Bytes** — `graph`
- **Path** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C112 - fake_node.rs|fake_node.rs]] — 5 cross-community edges
- [[docs/knowledge/communities/C070 - triggers.rs|triggers.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C177 - topic_address|topic_address]] — 3 cross-community edges
- [[docs/knowledge/communities/C111 - scanner.rs|scanner.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C120 - lib.rs (C120)|lib.rs (C120)]] — 1 cross-community edges
- [[docs/knowledge/communities/C170 - AaveConfig|AaveConfig]] — 1 cross-community edges
