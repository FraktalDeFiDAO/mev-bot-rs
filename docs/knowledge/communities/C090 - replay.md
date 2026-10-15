---
graphify-generated: true
graphify-community: "90"
graphify-node-count: 25
graphify-internal-edges: 99
tags:
  - graphify/community
---

# replay

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-replay/tests/replay.rs` — 21 nodes
- `graph` — 3 nodes
- `crates/mev-liq-replay/src/replay.rs` — 1 node

## Nodes (25)

- **replay.rs** — `crates/mev-liq-replay/tests/replay.rs:L1`
- **replay()** — `crates/mev-liq-replay/src/replay.rs:L331`
- **stream()** — `crates/mev-liq-replay/tests/replay.rs:L48`
- **event()** — `crates/mev-liq-replay/tests/replay.rs:L29`
- **healthy()** — `crates/mev-liq-replay/tests/replay.rs:L52`
- **config()** — `crates/mev-liq-replay/tests/replay.rs:L66`
- **borrower()** — `crates/mev-liq-replay/tests/replay.rs:L8`
- **state()** — `crates/mev-liq-replay/tests/replay.rs:L38`
- **warmup_and_right_censoring_are_explicit()** — `crates/mev-liq-replay/tests/replay.rs:L250`
- **completion_uses_pinned_start_state_and_does_not_credit_a_closed_episode()** — `crates/mev-liq-replay/tests/replay.rs:L97`
- **post_read_borrower_event_survives_completion_of_older_check()** — `crates/mev-liq-replay/tests/replay.rs:L159`
- **reorg_restores_ancestor_truth_and_invalidates_inflight_work()** — `crates/mev-liq-replay/tests/replay.rs:L118`
- **truth_only_changes_never_trigger_checks_or_leak_future_values()** — `crates/mev-liq-replay/tests/replay.rs:L76`
- **unknown_dependency_participates_in_oracle_sweep()** — `crates/mev-liq-replay/tests/replay.rs:L186`
- **overload_is_reported_and_accounts_are_not_dropped()** — `crates/mev-liq-replay/tests/replay.rs:L216`
- **wad_boundary_zero_debt_and_maximum_are_exact()** — `crates/mev-liq-replay/tests/replay.rs:L422`
- **gap_forces_a_safety_recheck_without_advisor_or_truth_changes()** — `crates/mev-liq-replay/tests/replay.rs:L318`
- **strict_loader_rejects_unknown_fields_bad_labels_gaps_and_order()** — `crates/mev-liq-replay/tests/replay.rs:L359`
- **header()** — `crates/mev-liq-replay/tests/replay.rs:L12`
- **absent_state_is_reported_and_never_becomes_a_successful_safe_read()** — `crates/mev-liq-replay/tests/replay.rs:L298`
- **oversized_input_and_invalid_decimal_money_fail_before_replay()** — `crates/mev-liq-replay/tests/replay.rs:L344`
- **resource_and_clock_limits_fail_explicitly()** — `crates/mev-liq-replay/tests/replay.rs:L274`
- **Vec** — `graph`
- **Address** — `graph`
- **U256** — `graph`

## Related communities

- [[docs/knowledge/communities/C046 - event.rs|event.rs]] — 10 cross-community edges
- [[docs/knowledge/communities/C071 - replay_with_policy|replay_with_policy]] — 7 cross-community edges
- [[docs/knowledge/communities/C178 - advisory_policy|advisory_policy]] — 2 cross-community edges
- [[docs/knowledge/communities/C248 - Args|Args]] — 1 cross-community edges
