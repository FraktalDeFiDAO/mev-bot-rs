---
graphify-generated: true
graphify-community: "42"
graphify-node-count: 41
graphify-internal-edges: 117
tags:
  - graphify/community
---

# advisory.rs (C042)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-sched/tests/advisory.rs` — 31 nodes
- `graph` — 8 nodes
- `crates/mev-liq-features/tests/features.rs` — 1 node
- `crates/mev-liq-sched/tests/common/mod.rs` — 1 node

## Nodes (41)

- **advisory.rs** — `crates/mev-liq-sched/tests/advisory.rs:L1`
- **good()** — `crates/mev-liq-sched/tests/advisory.rs:L20`
- **adv_cfg()** — `crates/mev-liq-sched/tests/common/mod.rs:L141`
- **p2_world()** — `crates/mev-liq-sched/tests/advisory.rs:L31`
- **wb()** — `crates/mev-liq-sched/tests/advisory.rs:L60`
- **input()** — `crates/mev-liq-sched/tests/advisory.rs:L41`
- **stale_or_out_of_order_decision_is_rejected()** — `crates/mev-liq-sched/tests/advisory.rs:L243`
- **exports()** — `crates/mev-liq-sched/tests/advisory.rs:L16`
- **aged_p2_job_gets_no_bonus_and_is_not_overtaken()** — `crates/mev-liq-sched/tests/advisory.rs:L427`
- **missing_decision_and_missing_export_fail_closed()** — `crates/mev-liq-sched/tests/advisory.rs:L208`
- **single()** — `crates/mev-liq-sched/tests/advisory.rs:L138`
- **verdict()** — `crates/mev-liq-sched/tests/advisory.rs:L69`
- **W** — `crates/mev-liq-sched/tests/advisory.rs:L24`
- **assert_fallback()** — `crates/mev-liq-sched/tests/advisory.rs:L145`
- **bonus_breaks_ties_between_equal_deadlines_only()** — `crates/mev-liq-sched/tests/advisory.rs:L359`
- **bonus_never_overtakes_an_earlier_deadline()** — `crates/mev-liq-sched/tests/advisory.rs:L397`
- **cfg1()** — `crates/mev-liq-sched/tests/advisory.rs:L53`
- **off_mode_reports_nothing()** — `crates/mev-liq-sched/tests/advisory.rs:L517`
- **only_p2_jobs_are_evaluated_and_p0_ignores_a_perfect_decision()** — `crates/mev-liq-sched/tests/advisory.rs:L286`
- **shadow_dispatches_baseline_and_reports_the_would_be_order()** — `crates/mev-liq-sched/tests/advisory.rs:L468`
- **jev_health_gates_everything()** — `crates/mev-liq-sched/tests/advisory.rs:L228`
- **digest_of()** — `crates/mev-liq-sched/tests/advisory.rs:L12`
- **Spec** — `crates/mev-liq-sched/tests/advisory.rs:L529`
- **adversarial()** — `crates/mev-liq-sched/tests/advisory.rs:L594`
- **bonus_is_bounded_monotonic_and_positive_at_threshold()** — `crates/mev-liq-sched/tests/advisory.rs:L321`
- **each_clause_has_its_own_fallback_reason()** — `crates/mev-liq-sched/tests/advisory.rs:L159`
- **mandatory()** — `crates/mev-liq-sched/tests/advisory.rs:L622`
- **normal_and_low_never_earn_or_lose_anything()** — `crates/mev-liq-sched/tests/advisory.rs:L309`
- **order()** — `crates/mev-liq-sched/tests/advisory.rs:L73`
- **Vec** — `graph`
- **bad_stale_or_sparse_oracle_data_is_unknown()** — `crates/mev-liq-features/tests/features.rs:L361`
- **config_validation()** — `crates/mev-liq-sched/tests/advisory.rs:L115`
- **default_mode_is_off_and_active_cannot_be_configured()** — `crates/mev-liq-sched/tests/advisory.rs:L83`
- **valid_decision_earns_a_bonus()** — `crates/mev-liq-sched/tests/advisory.rs:L151`
- **AccountState** — `graph`
- **B256** — `graph`
- **ExportView** — `graph`
- **FallbackReason** — `graph`
- **FnOnce** — `graph`
- **String** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C040 - sched.rs|sched.rs]] — 15 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 15 cross-community edges
- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 5 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 3 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 1 cross-community edges
- [[docs/knowledge/communities/C188 - main.rs (C188)|main.rs (C188)]] — 1 cross-community edges
- [[docs/knowledge/communities/C067 - features.rs (C067)|features.rs (C067)]] — 1 cross-community edges
- [[docs/knowledge/communities/C093 - Strategy|Strategy]] — 1 cross-community edges
