---
graphify-generated: true
graphify-community: "40"
graphify-node-count: 44
graphify-internal-edges: 133
tags:
  - graphify/community
---

# sched.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-sched/tests/sched.rs` — 27 nodes
- `graph` — 6 nodes
- `crates/mev-liq-sched/tests/common/mod.rs` — 5 nodes
- `crates/mev-liq-sched/tests/advisory_store.rs` — 3 nodes
- `crates/mev-liq-sched/src/types.rs` — 1 node
- `crates/mev-liq-sched/tests/advisory.rs` — 1 node
- `crates/mev-liq-state/src/index.rs` — 1 node

## Nodes (44)

- **sched.rs** — `crates/mev-liq-sched/tests/sched.rs:L1`
- **key()** — `crates/mev-liq-sched/tests/advisory_store.rs:L13`
- **cfg()** — `crates/mev-liq-sched/tests/sched.rs:L10`
- **DirtyEntry** — `crates/mev-liq-state/src/index.rs:L282`
- **healthy()** — `crates/mev-liq-sched/tests/common/mod.rs:L62`
- **snap()** — `crates/mev-liq-sched/tests/advisory_store.rs:L21`
- **dirty()** — `crates/mev-liq-sched/tests/advisory_store.rs:L132`
- **hf()** — `crates/mev-liq-sched/tests/common/mod.rs:L58`
- **find()** — `crates/mev-liq-sched/tests/common/mod.rs:L111`
- **one()** — `crates/mev-liq-sched/tests/sched.rs:L14`
- **p0_account()** — `crates/mev-liq-sched/tests/sched.rs:L295`
- **run_at()** — `crates/mev-liq-sched/tests/common/mod.rs:L88`
- **aging_prevents_starvation_under_prolonged_p0_flood()** — `crates/mev-liq-sched/tests/sched.rs:L488`
- **build()** — `crates/mev-liq-sched/tests/advisory.rs:L572`
- **p2_account()** — `crates/mev-liq-sched/tests/sched.rs:L304`
- **near_threshold_events_are_p0_far_events_p2()** — `crates/mev-liq-sched/tests/sched.rs:L66`
- **reorg_gap_and_flag_force_p1_until_a_later_check_covers_them()** — `crates/mev-liq-sched/tests/sched.rs:L208`
- **dedupe_keeps_most_urgent_reason_newest_ref_earliest_deadline()** — `crates/mev-liq-sched/tests/sched.rs:L88`
- **events_already_reflected_in_the_snapshot_are_rejected_not_scheduled()** — `crates/mev-liq-sched/tests/sched.rs:L128`
- **reserved_shares_under_saturation()** — `crates/mev-liq-sched/tests/sched.rs:L321`
- **run()** — `crates/mev-liq-sched/tests/common/mod.rs:L84`
- **hf_boundary_is_exact()** — `crates/mev-liq-sched/tests/sched.rs:L21`
- **InFlight** — `crates/mev-liq-sched/src/types.rs:L101`
- **max_age_promotes_to_p1_without_any_event()** — `crates/mev-liq-sched/tests/sched.rs:L155`
- **overload_records_backlog_misses_and_violation_instead_of_dropping()** — `crates/mev-liq-sched/tests/sched.rs:L370`
- **p1_account()** — `crates/mev-liq-sched/tests/sched.rs:L301`
- **unexpired_lease_is_running_expired_lease_is_restored()** — `crates/mev-liq-sched/tests/sched.rs:L273`
- **AccountState** — `graph`
- **awaiting_verification_candidate_is_p0()** — `crates/mev-liq-sched/tests/sched.rs:L258`
- **eligible_account_is_throttled_after_a_fresh_check_but_still_p0()** — `crates/mev-liq-sched/tests/sched.rs:L38`
- **invalidated_eligible_snapshot_is_not_trusted_as_verified()** — `crates/mev-liq-sched/tests/sched.rs:L47`
- **never_checked_digest_mismatch_and_unreconciled_are_p1()** — `crates/mev-liq-sched/tests/sched.rs:L188`
- **optional_decisions_never_change_anything_here()** — `crates/mev-liq-sched/tests/sched.rs:L538`
- **p0_does_not_use_unreserved_capacity_while_p2_shares_stay_when_p0_is_small()** — `crates/mev-liq-sched/tests/sched.rs:L349`
- **tier_counts()** — `crates/mev-liq-sched/tests/sched.rs:L311`
- **unknown_account_is_p1_even_for_a_weak_reason()** — `crates/mev-liq-sched/tests/sched.rs:L144`
- **AccountState** — `graph`
- **cohorts_share_a_saturated_lane_fairly()** — `crates/mev-liq-sched/tests/sched.rs:L455`
- **coverage_violation_when_budget_cannot_cover_tracked_accounts()** — `crates/mev-liq-sched/tests/sched.rs:L413`
- **invalid_config_is_an_error_not_a_guess()** — `crates/mev-liq-sched/tests/sched.rs:L436`
- **U256** — `graph`
- **Option** — `graph`
- **PoolState** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 21 cross-community edges
- [[docs/knowledge/communities/C042 - advisory.rs (C042)|advisory.rs (C042)]] — 15 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 10 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 6 cross-community edges
- [[docs/knowledge/communities/C179 - World|World]] — 6 cross-community edges
- [[docs/knowledge/communities/C142 - advisory_store.rs|advisory_store.rs]] — 5 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 3 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 3 cross-community edges
- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 2 cross-community edges
- [[docs/knowledge/communities/C079 - math.rs (C079)|math.rs (C079)]] — 1 cross-community edges
