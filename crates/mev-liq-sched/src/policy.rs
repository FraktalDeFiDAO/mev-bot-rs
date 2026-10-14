//! The pure scheduling function. No I/O, no clock reads, no randomness: every time-dependent
//! decision uses `ScheduleInput::now_ns`, and every collection is ordered by a total key, so
//! identical input gives byte-identical output regardless of input order.

use std::collections::{BTreeMap, BTreeSet};

use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainPoint, DirtyReason, FLAG_BACKLOG_ESCALATED,
    FLAG_FULL_REVERIFICATION, SnapshotStatus,
};

use crate::types::*;

/// Ranking interface task 006 and 011 implement against. The baseline never reads
/// `optional_decisions`.
pub trait RankingPolicy {
    fn schedule(&self, input: &ScheduleInput<'_>) -> Result<ScheduleOutput, ScheduleError>;
}

/// The mandatory, Jev-free baseline.
#[derive(Debug, Clone, Default)]
pub struct BaselinePolicy {
    pub config: PolicyConfig,
}

impl RankingPolicy for BaselinePolicy {
    fn schedule(&self, input: &ScheduleInput<'_>) -> Result<ScheduleOutput, ScheduleError> {
        schedule(input, &self.config)
    }
}

#[derive(Debug, Clone, Copy)]
struct Reason {
    tier: Tier,
    code: ReasonCode,
    due: u64,
    not_before: u64,
}

#[derive(Debug, Clone, Copy)]
struct Ev {
    reason: DirtyReason,
    chain: Option<ChainPoint>,
    at_ns: u64,
}

struct Acc {
    snapshot: Option<BorrowerSnapshot>,
    unreconciled: bool,
    awaiting: Option<u64>,
    cohort: Option<u64>,
    events: Vec<Ev>,
}

fn newer(a: &BorrowerSnapshot, b: &BorrowerSnapshot) -> bool {
    // Total order (ties broken by check time and digest) so input order never matters.
    (
        a.chain.block_number,
        a.revision,
        a.last_successful_check_ns,
        a.feature_digest,
    ) > (
        b.chain.block_number,
        b.revision,
        b.last_successful_check_ns,
        b.feature_digest,
    )
}

/// Hard ceiling of any advisory bonus (spec section 5 step 6: 0-20 points). Enforced inside the
/// scheduler core, independent of what an evaluator returns.
pub const MAX_ADVISORY_BONUS: u8 = 20;

/// Per-job advisory evaluator used by `schedule_with_advisory`. Called once per P2 job (with
/// the job's mandatory decision already computed); the result is clamped by the core.
pub type BonusFn<'a> = &'a dyn Fn(&RecheckJob) -> u8;

/// Build the deterministic mandatory schedule. See `docs/design/jev-liq-005-scheduler.md`.
/// Never reads `input.optional_decisions`.
pub fn schedule(
    input: &ScheduleInput<'_>,
    cfg: &PolicyConfig,
) -> Result<ScheduleOutput, ScheduleError> {
    schedule_inner(input, cfg, None)
}

/// The same schedule with a bounded advisory tie-breaker for P2 jobs (JEV-LIQ-011). The core
/// grants a bonus only to a P2 job that is not past its aging threshold, clamps it to
/// `MAX_ADVISORY_BONUS`, and uses it only to order jobs with an equal `earliest_due` inside
/// the discretionary lane (after deadline and tier). Everything else is the baseline code path.
pub fn schedule_with_advisory(
    input: &ScheduleInput<'_>,
    cfg: &PolicyConfig,
    bonus: BonusFn<'_>,
) -> Result<ScheduleOutput, ScheduleError> {
    schedule_inner(input, cfg, Some(bonus))
}

fn schedule_inner(
    input: &ScheduleInput<'_>,
    cfg: &PolicyConfig,
    advisory: Option<BonusFn<'_>>,
) -> Result<ScheduleOutput, ScheduleError> {
    cfg.validate()?;
    let now = input.now_ns;

    // Step 3: dedupe by AccountKey (newest ChainRef, all reasons, earliest deadlines survive).
    let mut accs: BTreeMap<AccountKey, Acc> = BTreeMap::new();
    let blank = || Acc {
        snapshot: None,
        unreconciled: false,
        awaiting: None,
        cohort: None,
        events: vec![],
    };
    for a in input.accounts {
        let e = accs.entry(a.key).or_insert_with(blank);
        if let Some(s) = &a.snapshot
            && e.snapshot.as_ref().is_none_or(|o| newer(s, o))
        {
            e.snapshot = Some(s.clone());
        }
        e.unreconciled |= a.unreconciled;
        e.awaiting = match (e.awaiting, a.awaiting_verification_since_ns) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        };
        e.cohort = Some(e.cohort.map_or(a.cohort, |c| c.min(a.cohort)));
    }
    for d in input.dirty {
        let e = accs.entry(d.key).or_insert_with(blank);
        for r in &d.reasons {
            e.events.push(Ev {
                reason: *r,
                chain: d.latest_chain,
                at_ns: d.marked_at_ns,
            });
        }
    }
    for u in input.urgent_events {
        accs.entry(u.key).or_insert_with(blank).events.push(Ev {
            reason: u.reason,
            chain: u.chain,
            at_ns: u.at_ns,
        });
    }

    // Pool-wide facts: (chain, pool) -> [(code, since_ns)].
    let mut pool_facts: BTreeMap<(u64, alloy::primitives::Address), Vec<(ReasonCode, u64)>> =
        BTreeMap::new();
    for p in input.pools {
        let v = pool_facts.entry((p.chain_id, p.pool)).or_default();
        for f in &p.flags {
            let code = match f.flag.as_str() {
                FLAG_FULL_REVERIFICATION => ReasonCode::FullReverification,
                FLAG_BACKLOG_ESCALATED => ReasonCode::BacklogEscalated,
                // Unknown flag: we cannot tell what it invalidated, so treat as full reverification.
                _ => ReasonCode::FullReverification,
            };
            v.push((code, f.set_ns));
        }
        for g in &p.gaps {
            v.push((ReasonCode::CoverageGap, g.recorded_ns));
        }
    }

    let mut stale = Vec::new();
    let mut jobs: Vec<RecheckJob> = Vec::new();
    for (key, acc) in &accs {
        let empty = Vec::new();
        let facts = pool_facts.get(&(key.chain_id, key.pool)).unwrap_or(&empty);
        let (reasons, newest_event_block) = evaluate(now, key, acc, facts, cfg, &mut stale);
        let Some(top) = reasons.iter().map(|r| r.tier).min() else {
            continue;
        };
        let mandatory: Vec<&Reason> = reasons.iter().filter(|r| r.tier <= Tier::P1).collect();
        let due = if mandatory.is_empty() {
            reasons.iter().map(|r| r.due).min()
        } else {
            mandatory.iter().map(|r| r.due).min()
        }
        .unwrap_or(now);
        let codes: BTreeSet<ReasonCode> = reasons.iter().map(|r| r.code).collect();
        let s = acc.snapshot.as_ref();
        let mut job = RecheckJob {
            key: *key,
            decision: ScheduleDecision {
                tier: top,
                advisory_bonus: 0,
                earliest_due: due,
                reason_codes: codes.into_iter().collect(),
                source_version: SourceVersion {
                    snapshot_revision: s.map_or(0, |s| s.revision),
                    snapshot_block: s.map(|s| s.chain.block_number),
                    feature_digest: s.map(|s| s.feature_digest),
                    newest_event_block,
                    policy_version: cfg.policy_version,
                },
            },
            cohort: acc.cohort.unwrap_or(0),
            not_before_ns: reasons.iter().map(|r| r.not_before).min().unwrap_or(0),
        };
        if let Some(f) = advisory
            && job.decision.tier == Tier::P2
        {
            let b = f(&job).min(MAX_ADVISORY_BONUS);
            // An aged job keeps its strict oldest-first position: no bonus at all.
            if now.saturating_sub(job.decision.earliest_due) < cfg.aging_ns {
                job.decision.advisory_bonus = b;
            }
        }
        jobs.push(job);
    }
    stale.sort_by(|a, b| {
        (a.key, a.reason.urgency(), a.event_block).cmp(&(b.key, b.reason.urgency(), b.event_block))
    });

    let mut candidates = TierCounts::default();
    for j in &jobs {
        candidates.add(j.decision.tier);
    }

    // Leases: an unexpired lease keeps the account out of this round (it is running); an expired
    // lease is simply ignored, which restores the job.
    let leased: BTreeSet<AccountKey> = input
        .in_flight
        .iter()
        .filter(|f| f.lease_expires_ns > now)
        .map(|f| f.key)
        .collect();
    let running_jobs = jobs.iter().filter(|j| leased.contains(&j.key)).count() as u64;
    let running_total = leased.len() as u64;
    let free = cfg
        .concurrency
        .saturating_sub(u32::try_from(running_total).unwrap_or(u32::MAX));

    let mut not_ready = 0u64;
    let mut lane0: Vec<RecheckJob> = vec![];
    let mut lane1: Vec<RecheckJob> = vec![];
    let mut lane2: Vec<RecheckJob> = vec![];
    for j in jobs.iter().filter(|j| !leased.contains(&j.key)) {
        if j.not_before_ns > now {
            not_ready += 1;
            continue;
        }
        match j.decision.tier {
            Tier::P0 => lane0.push(j.clone()),
            Tier::P1 => lane1.push(j.clone()),
            Tier::P2 | Tier::P3 => lane2.push(j.clone()),
        }
    }
    let edf = |a: &RecheckJob, b: &RecheckJob| {
        (a.decision.earliest_due, a.decision.tier, a.key).cmp(&(
            b.decision.earliest_due,
            b.decision.tier,
            b.key,
        ))
    };
    lane0.sort_by(edf);
    lane1.sort_by(edf);
    // Discretionary lane: deadline, then tier, then (advisory only) a higher bonus first, then
    // the key. The bonus is 0 for every non-P2 job and for every baseline job, so the baseline
    // order is unchanged.
    lane2.sort_by(|a, b| {
        let k = |j: &RecheckJob| {
            (
                j.decision.earliest_due,
                j.decision.tier,
                std::cmp::Reverse(j.decision.advisory_bonus),
                j.key,
            )
        };
        k(a).cmp(&k(b))
    }); // strict oldest-first: P2/P3 can never be starved by newer P2 arrivals

    // Steps 4 and 7: reserved shares, P0 preempts only not-yet-running P2 capacity.
    let c = free;
    let r0 = share_ceil(c, cfg.p0_share_permille);
    let r1 = share_ceil(c, cfg.p1_share_permille).min(c - r0);
    let r2 = c - r0 - r1;
    let (n0, n1, n2) = (lane0.len() as u32, lane1.len() as u32, lane2.len() as u32);
    let (mut a0, mut a1, mut a2) = (n0.min(r0), n1.min(r1), n2.min(r2));
    let mut left = c - a0 - a1 - a2;
    let t = (n0 - a0).min(left);
    a0 += t;
    left -= t;
    if n0 > a0 {
        let aged = lane2
            .iter()
            .any(|j| now.saturating_sub(j.decision.earliest_due) >= cfg.aging_ns);
        let floor = if aged { a2.min(cfg.p2_floor_slots) } else { 0 };
        let take = (n0 - a0).min(a2 - floor);
        a0 += take;
        a2 -= take;
    }
    let t = (n1 - a1).min(left);
    a1 += t;
    left -= t;
    let t = (n2 - a2).min(left);
    a2 += t;

    let (d0, rest0) = pick(lane0, a0 as usize);
    let (d1, rest1) = pick(lane1, a1 as usize);
    let (d2, rest2) = pick(lane2, a2 as usize);
    let deferred_discretionary = rest2.len() as u64;

    let mut dispatched: BTreeSet<AccountKey> = BTreeSet::new();
    let mut out_jobs = d0;
    out_jobs.extend(d1);
    out_jobs.extend(d2);
    for j in &out_jobs {
        dispatched.insert(j.key);
    }
    let mut backlog = rest0;
    backlog.extend(rest1);
    backlog.sort_by(|a, b| {
        (a.decision.tier, a.decision.earliest_due, a.key).cmp(&(
            b.decision.tier,
            b.decision.earliest_due,
            b.key,
        ))
    });

    // Deadline misses over every candidate (including leased jobs: they are late too).
    let mut misses: Vec<DeadlineMiss> = jobs
        .iter()
        .filter(|j| now > j.decision.earliest_due)
        .map(|j| DeadlineMiss {
            tier: j.decision.tier,
            account: j.key,
            overdue_ns: now - j.decision.earliest_due,
            dispatched: dispatched.contains(&j.key) || leased.contains(&j.key),
        })
        .collect();
    misses.sort_by(|a, b| {
        (a.tier, std::cmp::Reverse(a.overdue_ns), a.account).cmp(&(
            b.tier,
            std::cmp::Reverse(b.overdue_ns),
            b.account,
        ))
    });

    // Coverage feasibility: every tracked account needs one successful recheck per max-age
    // window; the reserved P1 share must be able to carry that.
    let mut violations = Vec::new();
    let (window_capacity, sweep_capacity) = sweep_capacity(cfg);
    let tracked = accs.len() as u64;
    if tracked > sweep_capacity {
        violations.push(CoverageViolation::SweepCapacity {
            tracked_accounts: tracked,
            sweep_capacity_per_window: sweep_capacity,
            window_capacity,
        });
    }
    let late_backlog: Vec<u64> = backlog
        .iter()
        .filter(|j| now > j.decision.earliest_due)
        .map(|j| now - j.decision.earliest_due)
        .collect();
    if !late_backlog.is_empty() {
        violations.push(CoverageViolation::MandatoryBacklogOverdue {
            overdue_jobs: late_backlog.len() as u64,
            max_overdue_ns: late_backlog.iter().copied().max().unwrap_or(0),
        });
    }

    Ok(ScheduleOutput {
        now_ns: now,
        jobs: out_jobs,
        backlog,
        deadline_misses: misses,
        coverage_violations: violations,
        stale_rejected: stale,
        candidates,
        deferred_discretionary,
        not_ready,
        running: running_jobs,
        free_slots: free,
    })
}

/// Rechecks one max-age window can hold on every slot, and the P1 share of it (the most accounts
/// a full sweep can cover once per window). Behind `CoverageViolation::SweepCapacity`.
pub fn sweep_capacity(cfg: &PolicyConfig) -> (u64, u64) {
    let window = (u128::from(cfg.concurrency) * u128::from(cfg.max_account_age_ns)
        / u128::from(cfg.recheck_cost_ns.max(1)))
    .min(u128::from(u64::MAX)) as u64;
    let sweep = (u128::from(window) * u128::from(cfg.p1_share_permille) / 1000)
        .min(u128::from(u64::MAX)) as u64;
    (window, sweep)
}

fn share_ceil(c: u32, permille: u32) -> u32 {
    (u64::from(c) * u64::from(permille)).div_ceil(1000) as u32
}

/// Take `n` jobs from an earliest-deadline-first lane, round-robin across cohorts so one reserve
/// or size class cannot monopolize the slots. Within a cohort the order stays earliest-due first;
/// across cohorts each round serves the cohort heads in earliest-due order. A single cohort
/// degenerates to plain EDF. Returns (picked in dispatch order, rest in EDF order).
fn pick(lane: Vec<RecheckJob>, n: usize) -> (Vec<RecheckJob>, Vec<RecheckJob>) {
    if n == 0 || lane.is_empty() {
        return (vec![], lane);
    }
    let mut groups: BTreeMap<u64, std::collections::VecDeque<usize>> = BTreeMap::new();
    for (i, j) in lane.iter().enumerate() {
        groups.entry(j.cohort).or_default().push_back(i);
    }
    let mut order: Vec<usize> = Vec::with_capacity(n.min(lane.len()));
    while order.len() < n && groups.values().any(|g| !g.is_empty()) {
        let mut heads: Vec<usize> = groups.values().filter_map(|g| g.front().copied()).collect();
        // Lane indices are already in EDF order, so index order == deadline order.
        heads.sort_unstable();
        for h in heads {
            if order.len() == n {
                break;
            }
            order.push(h);
            if let Some(g) = groups.get_mut(&lane[h].cohort) {
                g.pop_front();
            }
        }
    }
    let chosen: BTreeSet<usize> = order.iter().copied().collect();
    let picked = order.iter().map(|&i| lane[i].clone()).collect();
    let rest = lane
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !chosen.contains(i))
        .map(|(_, j)| j)
        .collect();
    (picked, rest)
}

/// Step 2: recompute the mandatory tier from deterministic state only.
fn evaluate(
    now: u64,
    key: &AccountKey,
    acc: &Acc,
    facts: &[(ReasonCode, u64)],
    cfg: &PolicyConfig,
    stale: &mut Vec<StaleRejection>,
) -> (Vec<Reason>, Option<u64>) {
    let mut out: Vec<Reason> = vec![];
    let mut push = |tier: Tier, code: ReasonCode, due: u64, not_before: u64| {
        out.push(Reason {
            tier,
            code,
            due,
            not_before,
        })
    };
    let snap = acc.snapshot.as_ref();

    // Step 3 stale-ref rejection: an event at or below the block the valid snapshot was read at
    // is already reflected in it. Reorg marks are never dropped.
    let mut events: Vec<Ev> = vec![];
    for e in &acc.events {
        if let (Some(s), Some(c)) = (snap, e.chain)
            && s.status == SnapshotStatus::Valid
            && e.reason != DirtyReason::Reorg
            && c.block_number <= s.chain.block_number
        {
            stale.push(StaleRejection {
                key: *key,
                reason: e.reason,
                event_block: c.block_number,
                snapshot_block: s.chain.block_number,
            });
            continue;
        }
        events.push(*e);
    }
    let newest_event_block = events
        .iter()
        .filter_map(|e| e.chain.map(|c| c.block_number))
        .max();

    if let Some(since) = acc.awaiting {
        push(
            Tier::P0,
            ReasonCode::AwaitingVerification,
            since.saturating_add(cfg.p0_deadline_ns),
            0,
        );
    }
    if acc.unreconciled {
        let due = snap
            .and_then(|s| s.last_successful_check_ns)
            .map_or(now, |l| l.saturating_add(cfg.max_account_age_ns));
        push(Tier::P1, ReasonCode::UnreconciledDependencies, due, 0);
    }

    match snap {
        None => {
            let at = events.iter().map(|e| e.at_ns).min().unwrap_or(now);
            push(
                Tier::P1,
                ReasonCode::UnknownState,
                at.saturating_add(cfg.p1_deadline_ns),
                0,
            );
        }
        Some(s) => {
            let observed = s.chain.observed_at_ns;
            let last = s.last_successful_check_ns;
            let base = last.unwrap_or(observed);
            if s.status == SnapshotStatus::Invalidated {
                push(
                    Tier::P1,
                    ReasonCode::SnapshotInvalidated,
                    base.saturating_add(cfg.p1_deadline_ns),
                    0,
                );
            }
            if !s.digest_matches() {
                push(
                    Tier::P1,
                    ReasonCode::DigestMismatch,
                    base.saturating_add(cfg.p1_deadline_ns),
                    0,
                );
            }
            match last {
                None => push(
                    Tier::P1,
                    ReasonCode::NeverChecked,
                    observed.saturating_add(cfg.p1_deadline_ns),
                    0,
                ),
                Some(l) => {
                    let deadline = l.saturating_add(cfg.max_account_age_ns);
                    if now.saturating_add(cfg.expiring_window_ns) >= deadline {
                        let code = if now > deadline {
                            ReasonCode::MaxAgeExpired
                        } else {
                            ReasonCode::MaxAgeExpiring
                        };
                        push(Tier::P1, code, deadline, 0);
                    } else if now.saturating_sub(l) >= cfg.background_after_ns {
                        push(
                            Tier::P3,
                            ReasonCode::Background,
                            deadline.saturating_sub(cfg.expiring_window_ns),
                            0,
                        );
                    }
                }
            }
            if s.status == SnapshotStatus::Valid && s.eligible() {
                let refresh_at = last
                    .unwrap_or(observed)
                    .saturating_add(cfg.eligible_refresh_ns);
                // A live event newer than the snapshot overrides the refresh throttle.
                let nb = if events.is_empty() { refresh_at } else { 0 };
                push(
                    Tier::P0,
                    ReasonCode::VerifiedEligible,
                    refresh_at.saturating_add(cfg.p0_deadline_ns),
                    nb,
                );
            }
            let has_debt = !s.total_debt_base.is_zero();
            let near = has_debt && s.health_factor_wad < cfg.near_threshold_wad;
            for e in &events {
                let p0_due = e.at_ns.saturating_add(cfg.p0_deadline_ns);
                let p1_due = e.at_ns.saturating_add(cfg.p1_deadline_ns);
                let p2_due = e.at_ns.saturating_add(cfg.p2_deadline_ns);
                match e.reason {
                    DirtyReason::Reorg => push(Tier::P1, ReasonCode::ReorgResync, p1_due, 0),
                    DirtyReason::Stale => push(Tier::P1, ReasonCode::StaleMark, p1_due, 0),
                    DirtyReason::PriceMove if near => {
                        push(Tier::P0, ReasonCode::RiskEventPriceMove, p0_due, 0)
                    }
                    DirtyReason::BorrowerEvent if !has_debt => {
                        push(Tier::P1, ReasonCode::NewDebtPossible, p1_due, 0)
                    }
                    DirtyReason::BorrowerEvent if near => {
                        push(Tier::P0, ReasonCode::RiskEventBorrower, p0_due, 0)
                    }
                    DirtyReason::ReserveConfig if near => {
                        push(Tier::P1, ReasonCode::RiskEventReserveConfig, p1_due, 0)
                    }
                    _ => push(Tier::P2, ReasonCode::DiscretionaryEvent, p2_due, 0),
                }
            }
        }
    }

    // Pool-wide: flags and open gaps put every account whose last check predates them in P1.
    for (code, since) in facts {
        let covered = snap
            .and_then(|s| s.last_successful_check_ns)
            .is_some_and(|l| l >= *since);
        if !covered {
            push(
                Tier::P1,
                *code,
                since.saturating_add(cfg.max_account_age_ns),
                0,
            );
        }
    }
    (out, newest_event_block)
}
