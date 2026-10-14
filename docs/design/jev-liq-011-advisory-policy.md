# JEV-LIQ-011: advisory policy validation (shadow only)

Code: `crates/mev-liq-sched/src/advisory.rs`, core hook in `policy.rs`, shadow wiring in `shadow.rs` and
`mev-liquidate::ShadowSchedule`. Spec: `docs/copper_jev_liquidation_architecture.md` sections 0 (inv. 2, 3, 6), 5 (steps 5-6),
7, 10, 13, 14. Nothing in this task changes live dispatch; `Active` is unreachable.

## Structure (no duplicated scheduler)

`schedule` (005) is unchanged in behaviour and still never reads `optional_decisions`. It now delegates to a private
`schedule_inner(input, cfg, Option<BonusFn>)`; `schedule_with_advisory` passes an evaluator. The core, not the evaluator,
enforces the safety properties for any evaluator output:

* the evaluator is called for tier-P2 jobs only; the result is clamped to `MAX_ADVISORY_BONUS` (20);
* a P2 job with `now - earliest_due >= aging_ns` keeps bonus 0 (strict oldest-first aging);
* the bonus is a sort key of the discretionary lane only: `(earliest_due, tier, Reverse(bonus), key)`. It orders jobs with an
  equal deadline; P0/P1 lanes, reserved shares, backlog, `deadline_misses`, coverage, `earliest_due` never see it.

`AdvisoryPolicy` computes the baseline first, then `would_be_schedule` (pure; also the REPLAY_JEV arm). In `Off`/`Shadow` the
dispatched `ScheduleOutput` is the baseline object itself (byte-identical). `ShadowAdvisoryReport` describes the active ordering.

## Mode and config

`AdvisoryMode { Off (default), Shadow, Active }`. `AdvisoryMode::parse_config("active")` returns
`AdvisoryConfigError::ActiveRequiresActivationGuard` (message names JEV-LIQ-019); `AdvisoryPolicy::new(Active, ..)` returns the same
error. The variant stays so 019 can add the guarded path. `AdvisoryConfig` (the spec's `PolicyConfig` side; renamed because the
scheduler already owns `PolicyConfig`): `policy_version`, `feature_schema`, `allowed_models` (non-empty), `confidence_threshold`
(0.80, in (0,1]), `max_bonus` (<= 20).

## Predicate -> fallback reason (all: `advisory_bonus = 0`, reason `BASELINE_FALLBACK`)

Evaluated per P2 job and per stored decision, first failing clause wins; with several decisions for one account the best valid one
applies, else the smallest reason (order independent). Mode (shadow evaluates as if active) and `mandatory_tier == P2` are structural.

| Clause | `FallbackReason` (`copper_jev_fallback_total{reason}`) |
|---|---|
| no decision for the account | `no_decision` |
| jev health Degraded / Open (010 breaker gauge 1 / >=2) | `jev_degraded` / `jev_open` |
| `policy_version` | `policy_version_mismatch` |
| `feature_schema` | `feature_schema_mismatch` |
| decision revision != snapshot revision, or export revision != snapshot revision (older or newer) | `revision_mismatch` |
| decision `feature_digest` != current export digest, or no export row | `digest_mismatch` |
| `chain_id` | `chain_mismatch` |
| `pool` | `pool_mismatch` |
| `expires_at <= now` | `expired` |
| model not in allow-list | `model_not_allowed` |
| NaN/inf/out-of-range confidence, prep_probability ([0,1]), value_score ([0,4]) | `invalid_answer` |
| `high` with confidence < threshold | `below_confidence` |
| job past the aging threshold | `aged_job` |

Digest note: the decision's `feature_digest` is the digest of the exported feature payload (008 `advisor_current`), not the
snapshot's own digest, so the scheduler cannot derive it. Callers pass an `ExportView` (`key -> (revision, digest)`, from
`SnapshotReader::current_export`); a missing entry fails closed.

## Bonus (phase 1, positive only)

`bonus_for`: `watch_tier == High` and `confidence >= threshold` gives `1 + (max-1)*(c-t)/(1000-t)` in permille integers, so the
range is `1..=max_bonus` and monotonic non-decreasing in confidence; `Normal`/`Low` and any non-finite value give 0. Never negative
(`u8`). 008's `valid_decision_for` already pre-filters revision/digest/TTL/model; the policy re-checks every clause because the
two reads are not one transaction.

## Shadow report and counters

`ShadowAdvisoryReport`: `p2_jobs`, `applied (account, bonus)`, `counters` (`copper_jev_fallback_total{reason}`,
`copper_jev_stale_response_total` = revision+digest+expired fallbacks, `copper_jev_applied_bonus_total{tier}` by watch tier),
`stale_decisions`, `expired_decisions`, `baseline_p2_order`, `would_be_p2_order`, `order_changed`, `mandatory_unchanged`
(self-check; the shadow pass logs an ALERT if it were ever false). Counts are per P2 job. Metric labels are fixed enums; borrower
keys appear only inside the event record, like the existing `liq_sched_shadow` job list.

`mev_liquidate::ShadowSchedule::with_advisory(AdvisoryShadow { policy, decisions, health })` (default none). Each pass runs
`run_shadow_pass_advisory`, records the usual `liq_sched_shadow`/`liq_sched_counters` from the baseline dispatch and, unless mode is
`Off`, the new event `liq_advisory_shadow` (+ tracing target `liq_advisory_shadow`).

## REPLAY_JEV (task 017)

`mev_liq_replay::replay_with_policy(stream, cfg, rev, &dyn RankingPolicy, label)` (additive; `replay` calls it with the baseline).
`ReplayAdvisoryPolicy { sched, advisory, health, decisions }` implements `RankingPolicy` with the active ordering; `decisions` maps
each scheduler call's input to `AdvisoryInputs` (the harness itself passes none). Equal budgets: pass the same `ReplayConfig`
(its `policy` is the scheduler config) to both arms.

## Changes to existing types

`OptionalDecision` now carries the 008 row fields (chain, pool, policy_version, feature_schema, revision, digest, expiry, model,
watch tier, confidence, prep_probability, value_score) and no `bonus`: the requested bonus is never an input.
`OptionalDecision::from_stored(&AdvisoryDecision)` converts a stored row.

## Tests

`crates/mev-liq-sched/tests/advisory.rs` (clause-by-clause, ties, aging, shadow byte identity, proptest over adversarial payloads),
`tests/advisory_store.rs` (real SQLite `AdvisorStore` + `valid_decision_for` into the policy, including r after r+1 and TTL),
`crates/mev-liq-replay/tests/advisory_replay.rs` (equal budgets, unchanged P0/P1 metrics), two `mev-liquidate` shadow tests.

## Gaps

* `Active` dispatch is intentionally absent; 019 owns the guard, calibration of the 0.80 threshold, and activation.
* The bonus only breaks ties between equal `earliest_due`; with realistic event streams ties are common only for bursts. 017 must
  report how often the order changed (`order_changed`) before any claim of value.
* No production caller supplies decisions to `ShadowSchedule` yet: 012 (worker) must provide `DecisionFn` (`valid_decision_for` +
  `current_export`) and `HealthFn` (010 `circuit_state`).
* Stale-count metrics are per shadow pass snapshots, not cumulative Prometheus counters; 013 should accumulate them.

## Consumers

* 012: build `AdvisoryShadow` (decisions via `OptionalDecision::from_stored`, exports via `SnapshotReader::current_export`, health via
  `JevHealth::from_circuit_state`).
* 013: export `AdvisoryCounters` names; alert on `mandatory_unchanged == false`.
* 017: `ReplayAdvisoryPolicy` + `replay_with_policy`, label `REPLAY_JEV`.
* 019: replace the `Active` rejection in `AdvisoryMode::parse_config` / `AdvisoryPolicy::new` behind the activation guard; the arm
  in `AdvisoryPolicy::run` already returns the active ordering.
