mod common;

use alloy::primitives::{B256, U256};
use common::*;
use mev_liq_sched::*;
use mev_liq_state::{
    CoverageGap, DirtyReason, FLAG_FULL_REVERIFICATION, PoolFlag, ScanDir, SnapshotStatus, WAD,
};

fn cfg() -> PolicyConfig {
    PolicyConfig::default()
}

fn one(accounts: Vec<AccountState>) -> ScheduleOutput {
    run(&accounts, &[], &cfg())
}

// ---------------------------------------------------------------- exact boundaries

#[test]
fn hf_boundary_is_exact() {
    let below = AccountState::new(key(1), Some(snap(1, WAD - U256::from(1u8), 1, 100, 10)));
    let at = AccountState::new(key(2), Some(snap(2, WAD, 1, 100, 10)));
    let above = AccountState::new(key(3), Some(snap(3, WAD + U256::from(1u8), 1, 100, 10)));
    let zero_debt_max = AccountState::new(key(4), Some(snap(4, U256::MAX, 0, 100, 10)));
    let zero_debt_zero_hf = AccountState::new(key(5), Some(snap(5, U256::ZERO, 0, 100, 10)));
    let o = one(vec![below, at, above, zero_debt_max, zero_debt_zero_hf]);
    let j = find(&o, 1).expect("HF = 1e18-1 with debt is P0");
    assert_eq!(j.decision.tier, Tier::P0);
    assert_eq!(j.decision.reason_codes, vec![ReasonCode::VerifiedEligible]);
    for n in [2, 3, 4, 5] {
        assert!(find(&o, n).is_none(), "account {n} must not be scheduled");
    }
    assert_eq!(o.jobs.len(), 1);
}

#[test]
fn eligible_account_is_throttled_after_a_fresh_check_but_still_p0() {
    let fresh = AccountState::new(key(1), Some(snap(1, WAD - U256::from(1u8), 1, 100, 1)));
    let o = one(vec![fresh]);
    assert!(o.jobs.is_empty());
    assert_eq!(o.not_ready, 1);
    assert_eq!(o.candidates.p0, 1);
}

#[test]
fn invalidated_eligible_snapshot_is_not_trusted_as_verified() {
    let mut s = snap(1, WAD - U256::from(1u8), 1, 100, 10);
    s.status = SnapshotStatus::Invalidated;
    let o = one(vec![AccountState::new(key(1), Some(s.seal()))]);
    let j = find(&o, 1).unwrap();
    assert_eq!(j.decision.tier, Tier::P1);
    assert!(
        j.decision
            .reason_codes
            .contains(&ReasonCode::SnapshotInvalidated)
    );
    assert!(
        !j.decision
            .reason_codes
            .contains(&ReasonCode::VerifiedEligible)
    );
}

#[test]
fn near_threshold_events_are_p0_far_events_p2() {
    let near = AccountState::new(key(1), Some(snap(1, hf(1049), 10, 100, 10)));
    let at_limit = AccountState::new(key(2), Some(snap(2, hf(1050), 10, 100, 10)));
    let far = healthy(3, 10);
    let d = [
        dirty(1, &[DirtyReason::PriceMove], 200, NOW - SEC),
        dirty(2, &[DirtyReason::PriceMove], 200, NOW - SEC),
        dirty(3, &[DirtyReason::BorrowerEvent], 200, NOW - SEC),
    ];
    let o = run(&[near, at_limit, far], &d, &cfg());
    assert_eq!(find(&o, 1).unwrap().decision.tier, Tier::P0);
    assert_eq!(
        find(&o, 1).unwrap().decision.reason_codes,
        vec![ReasonCode::RiskEventPriceMove]
    );
    assert_eq!(find(&o, 2).unwrap().decision.tier, Tier::P2);
    assert_eq!(find(&o, 3).unwrap().decision.tier, Tier::P2);
}

// ---------------------------------------------------------------- dedupe / stale refs

#[test]
fn dedupe_keeps_most_urgent_reason_newest_ref_earliest_deadline() {
    let old = snap(1, hf(1010), 10, 90, 10);
    let new = snap(1, hf(1010), 10, 100, 10);
    let accounts = [
        AccountState::new(key(1), Some(old)),
        AccountState::new(key(1), Some(new.clone())),
    ];
    let d = [
        dirty(
            1,
            &[DirtyReason::Manual, DirtyReason::PriceMove],
            200,
            NOW - 5 * SEC,
        ),
        dirty(1, &[DirtyReason::BorrowerEvent], 210, NOW - SEC),
    ];
    let o = run(&accounts, &d, &cfg());
    assert_eq!(o.jobs.len(), 1);
    let j = &o.jobs[0];
    assert_eq!(j.decision.tier, Tier::P0);
    assert_eq!(j.decision.source_version.snapshot_block, Some(100));
    assert_eq!(j.decision.source_version.newest_event_block, Some(210));
    assert!(
        j.decision
            .reason_codes
            .contains(&ReasonCode::RiskEventPriceMove)
    );
    assert!(
        j.decision
            .reason_codes
            .contains(&ReasonCode::RiskEventBorrower)
    );
    // earliest of the two P0 deadlines (the older event), not the later one
    assert_eq!(
        j.decision.earliest_due,
        NOW - 5 * SEC + cfg().p0_deadline_ns
    );
}

#[test]
fn events_already_reflected_in_the_snapshot_are_rejected_not_scheduled() {
    let a = AccountState::new(key(1), Some(snap(1, hf(1010), 10, 100, 10)));
    let d = [dirty(1, &[DirtyReason::PriceMove], 100, NOW - SEC)];
    let o = run(&[a], &d, &cfg());
    assert!(o.jobs.is_empty());
    assert_eq!(o.stale_rejected.len(), 1);
    assert_eq!(o.stale_rejected[0].event_block, 100);
    // A reorg mark at the same height is never dropped.
    let a = AccountState::new(key(1), Some(snap(1, hf(3000), 10, 100, 10)));
    let d = [dirty(1, &[DirtyReason::Reorg], 100, NOW - SEC)];
    let o = run(&[a], &d, &cfg());
    assert_eq!(find(&o, 1).unwrap().decision.tier, Tier::P1);
    assert!(o.stale_rejected.is_empty());
}

#[test]
fn unknown_account_is_p1_even_for_a_weak_reason() {
    let d = [dirty(7, &[DirtyReason::Discovered], 50, NOW - SEC)];
    let o = run(&[], &d, &cfg());
    let j = find(&o, 7).unwrap();
    assert_eq!(j.decision.tier, Tier::P1);
    assert_eq!(j.decision.reason_codes, vec![ReasonCode::UnknownState]);
}

// ---------------------------------------------------------------- max age / P1 triggers

#[test]
fn max_age_promotes_to_p1_without_any_event() {
    let c = cfg();
    let max = c.max_account_age_ns / SEC; // 300
    let win = c.expiring_window_ns / SEC; // 60
    // fresh (well inside): nothing; background candidate after 150 s
    assert!(one(vec![healthy(1, 10)]).jobs.is_empty());
    let o = one(vec![healthy(2, 160)]);
    assert_eq!(find(&o, 2).unwrap().decision.tier, Tier::P3);
    // inside the expiring window: P1 and not yet late
    let o = one(vec![healthy(3, max - win)]);
    let j = find(&o, 3).unwrap();
    assert_eq!(j.decision.tier, Tier::P1);
    assert_eq!(j.decision.reason_codes, vec![ReasonCode::MaxAgeExpiring]);
    assert!(o.deadline_misses.is_empty());
    // age == max is the deadline itself: still on time
    let o = one(vec![healthy(4, max)]);
    assert_eq!(
        find(&o, 4).unwrap().decision.reason_codes,
        vec![ReasonCode::MaxAgeExpiring]
    );
    assert!(o.deadline_misses.is_empty());
    // one ns past: expired, and a typed miss is recorded
    let mut late = healthy(5, max);
    late.snapshot.as_mut().unwrap().last_successful_check_ns = Some(NOW - max * SEC - 1);
    let o = one(vec![late]);
    let j = find(&o, 5).unwrap();
    assert_eq!(j.decision.reason_codes, vec![ReasonCode::MaxAgeExpired]);
    assert_eq!(o.deadline_misses.len(), 1);
    assert_eq!(o.deadline_misses[0].overdue_ns, 1);
    assert_eq!(o.deadline_misses[0].tier, Tier::P1);
}

#[test]
fn never_checked_digest_mismatch_and_unreconciled_are_p1() {
    let mut never = healthy(1, 10);
    never.snapshot.as_mut().unwrap().last_successful_check_ns = None;
    let mut bad = healthy(2, 10);
    bad.snapshot.as_mut().unwrap().total_debt_base = U256::from(12345u64); // digest now wrong
    let mut unrec = healthy(3, 10);
    unrec.unreconciled = true;
    let o = one(vec![never, bad, unrec]);
    for (n, code) in [
        (1, ReasonCode::NeverChecked),
        (2, ReasonCode::DigestMismatch),
        (3, ReasonCode::UnreconciledDependencies),
    ] {
        let j = find(&o, n).unwrap();
        assert_eq!(j.decision.tier, Tier::P1, "account {n}");
        assert!(j.decision.reason_codes.contains(&code), "account {n}");
    }
}

#[test]
fn reorg_gap_and_flag_force_p1_until_a_later_check_covers_them() {
    let gap = CoverageGap {
        id: 1,
        name: "n".into(),
        dir: ScanDir::Forward,
        from_block: 10,
        to_block: 20,
        reason: "rpc".into(),
        recorded_ns: NOW - 30 * SEC,
        attempts: 1,
    };
    let flag = PoolFlag {
        flag: FLAG_FULL_REVERIFICATION.into(),
        block_number: None,
        detail: String::new(),
        set_ns: NOW - 20 * SEC,
    };
    let pools = [PoolState {
        chain_id: 42161,
        pool: pool(),
        flags: vec![flag],
        gaps: vec![gap],
    }];
    // account 1 checked 100 s ago (before both), account 2 checked 10 s ago (after both)
    let o = run_at(
        NOW,
        &[healthy(1, 100), healthy(2, 10)],
        &[],
        &pools,
        &[],
        &cfg(),
    );
    let j = find(&o, 1).unwrap();
    assert_eq!(j.decision.tier, Tier::P1);
    assert!(j.decision.reason_codes.contains(&ReasonCode::CoverageGap));
    assert!(
        j.decision
            .reason_codes
            .contains(&ReasonCode::FullReverification)
    );
    assert!(find(&o, 2).is_none());
    // reorg marks on a healthy fresh account
    let d = [dirty(2, &[DirtyReason::Reorg], 5, NOW - SEC)];
    let o = run(&[healthy(2, 10)], &d, &cfg());
    let j = find(&o, 2).unwrap();
    assert_eq!(j.decision.tier, Tier::P1);
    assert!(j.decision.reason_codes.contains(&ReasonCode::ReorgResync));
}

#[test]
fn awaiting_verification_candidate_is_p0() {
    let mut a = healthy(1, 10);
    a.awaiting_verification_since_ns = Some(NOW - SEC);
    let o = one(vec![a]);
    let j = find(&o, 1).unwrap();
    assert_eq!(j.decision.tier, Tier::P0);
    assert_eq!(
        j.decision.reason_codes,
        vec![ReasonCode::AwaitingVerification]
    );
}

// ---------------------------------------------------------------- restart / lease

#[test]
fn unexpired_lease_is_running_expired_lease_is_restored() {
    let accounts = [healthy(1, 290), healthy(2, 290)];
    let lease = |n, exp| InFlight {
        key: key(n),
        lease_expires_ns: exp,
    };
    let o = run_at(
        NOW,
        &accounts,
        &[],
        &[],
        &[lease(1, NOW + 5 * SEC), lease(2, NOW)],
        &cfg(),
    );
    assert!(find(&o, 1).is_none(), "leased job is not dispatched twice");
    assert!(find(&o, 2).is_some(), "expired lease restores the job");
    assert_eq!(o.running, 1);
    assert_eq!(o.free_slots, cfg().concurrency - 1);
}

// ---------------------------------------------------------------- capacity

fn p0_account(n: u32) -> (AccountState, mev_liq_state::DirtyEntry) {
    (
        AccountState::new(key(n), Some(snap(n, hf(1010), 10, 100, 10))),
        dirty(n, &[DirtyReason::PriceMove], 200, NOW - SEC),
    )
}
fn p1_account(n: u32) -> AccountState {
    healthy(n, 280)
}
fn p2_account(n: u32, at_ns: u64) -> (AccountState, mev_liq_state::DirtyEntry) {
    (
        healthy(n, 10),
        dirty(n, &[DirtyReason::BorrowerEvent], 200, at_ns),
    )
}

fn tier_counts(jobs: &[RecheckJob]) -> (usize, usize, usize) {
    let c = |f: fn(Tier) -> bool| jobs.iter().filter(|j| f(j.decision.tier)).count();
    (
        c(|t| t == Tier::P0),
        c(|t| t == Tier::P1),
        c(|t| matches!(t, Tier::P2 | Tier::P3)),
    )
}

#[test]
fn reserved_shares_under_saturation() {
    let mut c = cfg();
    c.concurrency = 10;
    let (mut acc, mut d) = (vec![], vec![]);
    for n in 0..100 {
        let (a, e) = p0_account(n);
        acc.push(a);
        d.push(e);
    }
    for n in 100..200 {
        acc.push(p1_account(n));
    }
    for n in 200..300 {
        let (a, e) = p2_account(n, NOW - 10 * SEC);
        acc.push(a);
        d.push(e);
    }
    let o = run(&acc, &d, &c);
    // 0.50 / 0.30 / 0.20 of 10 slots; P0 overflow only preempts the P2 share.
    assert_eq!(o.jobs.len(), 10);
    let (p0, p1, p2) = tier_counts(&o.jobs);
    assert!(p1 >= 3, "P1 keeps its reserved share, got {p1}");
    assert_eq!((p0, p1, p2), (7, 3, 0));
    // P0 first in dispatch order, P1 never behind P2
    assert!(o.jobs[..7].iter().all(|j| j.decision.tier == Tier::P0));
}

#[test]
fn p0_does_not_use_unreserved_capacity_while_p2_shares_stay_when_p0_is_small() {
    let mut c = cfg();
    c.concurrency = 10;
    let (mut acc, mut d) = (vec![], vec![]);
    for n in 0..3 {
        let (a, e) = p0_account(n);
        acc.push(a);
        d.push(e);
    }
    for n in 200..300 {
        let (a, e) = p2_account(n, NOW - 10 * SEC);
        acc.push(a);
        d.push(e);
    }
    let o = run(&acc, &d, &c);
    // unused P0/P1 reserve flows to P2: all 10 slots used, none wasted
    assert_eq!(tier_counts(&o.jobs), (3, 0, 7));
    assert_eq!(o.deferred_discretionary, 93);
}

#[test]
fn overload_records_backlog_misses_and_violation_instead_of_dropping() {
    let mut c = cfg();
    c.concurrency = 4;
    let (mut acc, mut d) = (vec![], vec![]);
    for n in 0..30 {
        let (a, _) = p0_account(n);
        acc.push(a);
        // marked 10 s ago: P0 deadline (2 s) is long past
        d.push(dirty(n, &[DirtyReason::PriceMove], 200, NOW - 10 * SEC));
    }
    for n in 100..120 {
        acc.push(p1_account(n));
    }
    let o = run(&acc, &d, &c);
    let all = o.jobs.len() + o.backlog.len() + o.deferred_discretionary as usize;
    assert_eq!(
        all as u64,
        o.candidates.p0 + o.candidates.p1 + o.candidates.p2 + o.candidates.p3
    );
    assert_eq!(o.candidates.p0, 30);
    assert_eq!(o.candidates.p1, 20);
    assert_eq!(o.jobs.len(), 4);
    assert_eq!(o.backlog.len(), 46, "no P0/P1 job silently discarded");
    let p0_miss = o
        .deadline_misses
        .iter()
        .filter(|m| m.tier == Tier::P0)
        .count();
    assert_eq!(p0_miss, 30);
    assert!(o.deadline_misses.iter().any(|m| !m.dispatched));
    assert!(
        o.coverage_violations
            .iter()
            .any(|v| matches!(v, CoverageViolation::MandatoryBacklogOverdue { .. }))
    );
    // worst-overdue P0 first inside the backlog ordering
    assert!(o.backlog.windows(2).all(|w| {
        (w[0].decision.tier, w[0].decision.earliest_due)
            <= (w[1].decision.tier, w[1].decision.earliest_due)
    }));
}

#[test]
fn coverage_violation_when_budget_cannot_cover_tracked_accounts() {
    let mut c = cfg();
    c.concurrency = 1;
    c.recheck_cost_ns = SEC;
    c.max_account_age_ns = 10 * SEC;
    c.expiring_window_ns = 2 * SEC;
    c.background_after_ns = 5 * SEC;
    // window capacity 10 rechecks, P1 share 30% -> 3 sweep rechecks per window
    let three: Vec<_> = (0..3).map(|n| healthy(n, 1)).collect();
    assert!(run(&three, &[], &c).coverage_violations.is_empty());
    let four: Vec<_> = (0..4).map(|n| healthy(n, 1)).collect();
    let o = run(&four, &[], &c);
    assert_eq!(
        o.coverage_violations,
        vec![CoverageViolation::SweepCapacity {
            tracked_accounts: 4,
            sweep_capacity_per_window: 3,
            window_capacity: 10
        }]
    );
}

#[test]
fn invalid_config_is_an_error_not_a_guess() {
    let mut c = cfg();
    c.p2_share_permille = 300;
    let r = schedule(
        &ScheduleInput {
            now_ns: NOW,
            accounts: &[],
            dirty: &[],
            urgent_events: &[],
            pools: &[],
            in_flight: &[],
            optional_decisions: &[],
        },
        &c,
    );
    assert!(matches!(r, Err(ScheduleError::InvalidConfig(_))));
}

#[test]
fn cohorts_share_a_saturated_lane_fairly() {
    let mut c = cfg();
    c.concurrency = 10; // P1 reserve = 3
    let mut acc = vec![];
    // cohort 1 owns the 10 earliest deadlines, cohort 2 two later ones
    for n in 0..10 {
        let mut a = healthy(n, 285 - u64::from(n));
        a.cohort = 1;
        acc.push(a);
    }
    for n in 10..12 {
        let mut a = healthy(n, 270);
        a.cohort = 2;
        acc.push(a);
    }
    let o = run(&acc, &[], &c);
    let picked: Vec<u64> = o
        .jobs
        .iter()
        .filter(|j| j.decision.tier == Tier::P1)
        .map(|j| j.cohort)
        .collect();
    assert!(
        picked.contains(&2),
        "late-deadline cohort still served: {picked:?}"
    );
}

// ---------------------------------------------------------------- aging / starvation

/// Simulated ticks under a permanent P0 flood: the oldest discretionary job still runs, and P1
/// keeps its share every single tick.
#[test]
fn aging_prevents_starvation_under_prolonged_p0_flood() {
    let mut c = cfg();
    c.concurrency = 10;
    // keep max-age out of the picture: this test is about aging alone
    c.max_account_age_ns = 1_000_000 * SEC;
    c.background_after_ns = 1_000_000 * SEC;
    let tick = 10 * SEC;
    let victim = 9_000u32;
    let mut served_at: Option<u64> = None;
    let mut next = 0u32;
    // live queue of accounts, each processed once when dispatched
    let mut queue: Vec<(AccountState, mev_liq_state::DirtyEntry)> = vec![];
    let (va, vd) = p2_account(victim, NOW - SEC);
    queue.push((va, vd));
    for t in 0..60u64 {
        let now = NOW + t * tick;
        for _ in 0..40 {
            // 40 fresh P0 arrivals per tick against 10 slots: permanent overload
            let (a, _) = p0_account(next);
            let d = dirty(next, &[DirtyReason::PriceMove], 200, now - SEC);
            queue.push((a, d));
            next += 1;
        }
        // a fresh P1 candidate every tick
        let (p1a, _) = p2_account(5_000 + t as u32, now);
        let mut p1a = p1a;
        p1a.unreconciled = true;
        queue.push((
            p1a,
            dirty(5_000 + t as u32, &[DirtyReason::Manual], 200, now),
        ));
        let acc: Vec<_> = queue.iter().map(|(a, _)| a.clone()).collect();
        let dr: Vec<_> = queue.iter().map(|(_, d)| d.clone()).collect();
        let o = run_at(now, &acc, &dr, &[], &[], &c);
        let (_, p1, _) = tier_counts(&o.jobs);
        assert!(p1 >= 1, "tick {t}: P1 got its reserved capacity");
        if o.jobs.iter().any(|j| j.key == key(victim)) && served_at.is_none() {
            served_at = Some(t);
        }
        let done: std::collections::BTreeSet<_> = o.jobs.iter().map(|j| j.key).collect();
        queue.retain(|(a, _)| !done.contains(&a.key));
    }
    let t = served_at.expect("discretionary job starved under a P0 flood");
    // due at ~60 s, aged after another 120 s, floor slot every tick afterwards: well under 30 ticks
    assert!(t <= 25, "served only at tick {t}");
}

// ---------------------------------------------------------------- advisory independence (smoke)

#[test]
fn optional_decisions_never_change_anything_here() {
    let accounts = [healthy(1, 290), healthy(2, 10)];
    let base = run(&accounts, &[], &cfg());
    let dec = [OptionalDecision {
        key: key(2),
        revision: 1,
        feature_digest: B256::repeat_byte(1),
        bonus: 20,
        expires_at_ns: NOW + 1_000 * SEC,
    }];
    let with = schedule(
        &ScheduleInput {
            now_ns: NOW,
            accounts: &accounts,
            dirty: &[],
            urgent_events: &[],
            pools: &[],
            in_flight: &[],
            optional_decisions: &dec,
        },
        &cfg(),
    )
    .unwrap();
    assert_eq!(base, with);
    assert!(with.jobs.iter().all(|j| j.decision.advisory_bonus == 0));
}
