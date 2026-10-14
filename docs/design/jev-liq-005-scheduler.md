# JEV-LIQ-005 deterministic mandatory scheduler

Crate `mev-liq-sched` (leaf: alloy, `mev-liq-state` types, serde). `schedule(&ScheduleInput, &PolicyConfig)`
is pure: no I/O, no clock, no randomness, no read of `optional_decisions` (type present for task 011;
`advisory_bonus` is always 0). Input order never matters (BTreeMap keyed by `AccountKey`, total-order
tie-breaks); output is byte-identical serialized JSON for identical input.
`RankingPolicy` trait + `BaselinePolicy` wrap it for task 006.

## Tiers (recomputed each call from snapshot + events + pool facts, never from stored advisory data)

| Tier | Reasons |
|---|---|
| P0 | valid snapshot with `eligible()` (HF < 1e18 exact U256, debt > 0); `PriceMove`/`BorrowerEvent` with debt > 0 and HF < `near_threshold_wad` (1.05e18); `awaiting_verification` candidate |
| P1 | no snapshot; never checked; invalidated snapshot; digest mismatch; Reorg/Stale marks; `BorrowerEvent` on a zero-debt account; `ReserveConfig` near threshold; unreconciled deps; max age expired or inside the 60 s expiring window; `full_reverification`/`backlog_escalated` flag or open coverage gap newer than the account's last check |
| P2 | any other unreflected event (far from threshold, Manual, Discovered with snapshot) |
| P3 | healthy covered account not checked for `background_after` (150 s) |

Job tier = min over reasons; `earliest_due` = earliest deadline of the mandatory (P0/P1) reasons.
Max age (300 s) is a hard deadline `last_check + 300 s`; it needs no event. Events at or below the block
the valid snapshot was read at are rejected as stale (`stale_rejected`), except Reorg marks. Duplicate
accounts keep the newest snapshot (block, revision, check time, digest), all reasons, earliest due.
A verified-eligible account is rechecked at most every `eligible_refresh_ns` (5 s) unless a newer event
exists (`not_before_ns`; still counted P0).

## Dispatch (free slots = concurrency - unexpired leases)

Reserved shares (permille 500/300/200, rounded up for P0, P1; P2 = rest). Unused reserve flows down the
priority order; P0 overflow preempts only the not-yet-running P2 share, except `p2_floor_slots` (1) when
an aged P2/P3 job (past due by `aging_ns`) exists. P1 is never preempted. Within a lane: earliest-due
first (strict oldest-first, so discretionary work cannot starve), round-robin across fairness cohorts
(`cohort_of`: largest-debt reserve, else debt-size bucket). Expired leases are ignored, which restores the
job after a restart or a crashed worker; unexpired leases are never re-dispatched or cancelled.

## Overload and coverage

P0/P1 that do not fit go to `backlog` (never dropped) and every passed deadline becomes
`DeadlineMiss{tier, account, overdue_ns, dispatched}`. `CoverageViolation::SweepCapacity` when tracked
accounts exceed `concurrency * max_age / recheck_cost * p1_share`; `MandatoryBacklogOverdue` when overdue
P0/P1 work is queued. Neither is ever hidden; `ShadowReport::needs_alert()` is true for either or any P0/P1 miss.

## Wiring (additive)

`mev_liquidate::ShadowSchedule` (default disabled) is attached with `Runner::with_shadow_schedule`. The
existing `select!` loop ticks it; the pass runs on a blocking thread (single-flight), reads the repository
(`load_inputs`: active borrowers, snapshots, `dirty_entries`, `list_unreconciled`, `flags`, `open_gaps`),
and records tracing target `liq_sched_shadow` plus two events through an optional sink shaped like
`Metrics::event(kind, data)`: `liq_sched_shadow` (counters + job list + backlog + misses, capped by
`max_records`) and `liq_sched_counters`. It never changes polling, planning or sending. Not yet wired from
`mev-engine` config, and the engine does not yet call `AaveV3Adapter::attach_triggers` (task 004), so the
repository only has marks when those are attached.

## Not covered

- Clean feed of `mark_dirty` from the live engine; the `Runner` still dispatches by its own timers.
- aToken/debt-token transfers and PoolConfigurator events (task 004 gaps) reach the scheduler only
  through max age.
- Per-account last-check timestamps exist only in repository snapshots; the legacy runner records none,
  so the 006 comparison needs the shadow job list vs. the runner's own poll log.
