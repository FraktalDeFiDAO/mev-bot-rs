//! Real SQLite repository + task 004 apply path -> scheduled order, across a restart.

mod common;

use common::*;
use mev_liq_sched::*;
use mev_liq_state::{
    DirtyReason, DirtyTarget, Effect, NewGap, ScanBatch, ScanDir, SnapshotRepository, WAD,
};

const SCAN: &str = "live";

fn ids(o: &ScheduleOutput) -> Vec<(u32, Tier)> {
    let n = |k: &mev_liq_state::AccountKey| {
        u32::from_be_bytes(k.borrower.as_slice()[16..].try_into().unwrap())
    };
    o.jobs
        .iter()
        .map(|j| (n(&j.key), j.decision.tier))
        .collect()
}

fn params() -> ShadowParams {
    ShadowParams {
        chain_id: 42161,
        pool: pool(),
        scan_name: SCAN.into(),
        policy: PolicyConfig::default(),
        max_records: 100,
    }
}

fn schedule_from(repo: &SnapshotRepository, now: u64) -> ScheduleOutput {
    let l = load_inputs(repo, 42161, pool(), SCAN).unwrap();
    schedule(
        &ScheduleInput {
            now_ns: now,
            accounts: &l.accounts,
            dirty: &l.dirty,
            urgent_events: &[],
            pools: &l.pools,
            in_flight: &[],
            optional_decisions: &[],
        },
        &PolicyConfig::default(),
    )
    .unwrap()
}

/// Canonical snapshots must sit on a checkpointed block.
fn store(repo: &SnapshotRepository, s: mev_liq_state::BorrowerSnapshot, expected: u64) {
    repo.checkpoint_canonical(mev_liq_state::ChainPoint {
        block_number: s.chain.block_number,
        block_hash: s.chain.block_hash,
    })
    .unwrap();
    repo.upsert_cas(&s, expected).unwrap();
}

/// Registers `n` through the task-004 apply path and stores a snapshot for it.
fn seed(repo: &SnapshotRepository, n: u32, hf_milli: u64, debt: u64, age_s: u64) {
    let b = key(n).borrower;
    let at = point(100);
    let mut batch = ScanBatch::new(SCAN, 42161, pool(), NOW - 1_000 * SEC, 100);
    batch.effects.push(Effect::Touch {
        borrower: b,
        reason: DirtyReason::Discovered,
        at,
        register: Some(at),
        stale_deps: false,
    });
    repo.apply_scan(&batch).unwrap();
    repo.reconcile_borrower(42161, pool(), b, &[], true, at)
        .unwrap();
    store(repo, snap(n, hf(hf_milli), debt, 100, age_s), 0);
}

fn live_event(repo: &SnapshotRepository, n: u32, reason: DirtyReason, block: u64, now: u64) {
    let mut batch = ScanBatch::new(SCAN, 42161, pool(), now, 100);
    batch.effects.push(Effect::Touch {
        borrower: key(n).borrower,
        reason,
        at: point(block),
        register: None,
        stale_deps: false,
    });
    repo.apply_scan(&batch).unwrap();
}

#[test]
fn dirty_marks_from_the_apply_path_are_scheduled_in_tier_order_and_survive_restart() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("liq.sqlite");
    let repo = SnapshotRepository::open(&path).unwrap();

    seed(&repo, 1, 1010, 10, 20); // near threshold, then a price move   -> P0
    seed(&repo, 2, 3000, 10, 20); // healthy, then a borrower event      -> P2
    seed(&repo, 4, 2000, 10, 280); // healthy but 280 s since last check -> P1 (expiring)
    seed(&repo, 5, 900, 10, 20); // HF 0.9 with debt: verified eligible  -> P0
    seed(&repo, 6, 3000, 10, 20); // healthy, no events                  -> nothing
    // account 3 is only seen by the index (Borrow) and has no snapshot yet -> P1 unknown
    let mut batch = ScanBatch::new(SCAN, 42161, pool(), NOW - 10 * SEC, 100);
    let at = point(150);
    batch.effects.push(Effect::Touch {
        borrower: key(3).borrower,
        reason: DirtyReason::BorrowerEvent,
        at,
        register: Some(at),
        stale_deps: false,
    });
    repo.apply_scan(&batch).unwrap();
    live_event(&repo, 1, DirtyReason::PriceMove, 200, NOW - 3 * SEC);
    live_event(&repo, 2, DirtyReason::BorrowerEvent, 200, NOW - 3 * SEC);
    // a snapshot-less mark through the legacy mark_dirty path is still scheduled
    repo.mark_dirty(DirtyTarget::Account(key(4)), DirtyReason::Manual, NOW - SEC)
        .unwrap();

    let o = schedule_from(&repo, NOW);
    let got = ids(&o);
    // P0 first (account 1 due earlier than 5? 1: NOW-3s+2s; 5: refresh at NOW-20s+5s, +2s), then P1, then P2.
    let tiers: Vec<Tier> = got.iter().map(|g| g.1).collect();
    assert!(tiers.windows(2).all(|t| t[0] <= t[1]), "{got:?}");
    let by = |n: u32| got.iter().find(|g| g.0 == n).copied();
    assert_eq!(by(1), Some((1, Tier::P0)));
    assert_eq!(by(5), Some((5, Tier::P0)));
    assert_eq!(by(3), Some((3, Tier::P1)));
    assert_eq!(by(4), Some((4, Tier::P1)));
    assert_eq!(by(2), Some((2, Tier::P2)));
    assert_eq!(by(6), None);
    // earliest deadline first inside P0: verified-eligible 5 (long due) before 1
    assert_eq!(got[0].0, 5);
    assert_eq!(got[1].0, 1);
    assert!(o.coverage_violations.is_empty());

    // Restart: reopen the database; unacked work is restored identically.
    let first = serde_json::to_string(&o).unwrap();
    drop(repo);
    let repo2 = SnapshotRepository::open(&path).unwrap();
    assert_eq!(
        serde_json::to_string(&schedule_from(&repo2, NOW)).unwrap(),
        first
    );

    // Acknowledge account 1 the real way: a fresh snapshot (CAS) clears its marks.
    let fresh = snap(1, hf(1010), 10, 300, 0);
    store(&repo2, fresh, 1);
    let again = ids(&schedule_from(&repo2, NOW));
    assert!(!again.iter().any(|g| g.0 == 1), "{again:?}");
}

#[test]
fn open_gap_and_full_reverification_flag_drive_p1_through_the_repository() {
    let d = tempfile::tempdir().unwrap();
    let repo = SnapshotRepository::open(&d.path().join("liq.sqlite")).unwrap();
    seed(&repo, 1, 3000, 10, 100);
    seed(&repo, 2, 3000, 10, 5);
    assert!(schedule_from(&repo, NOW).jobs.is_empty());

    let mut batch = ScanBatch::new(SCAN, 42161, pool(), NOW - 30 * SEC, 100);
    batch.new_gaps.push(NewGap {
        dir: ScanDir::Forward,
        from_block: 10,
        to_block: 20,
        reason: "rpc timeout".into(),
    });
    repo.apply_scan(&batch).unwrap();
    let o = schedule_from(&repo, NOW);
    // account 1 was last checked before the gap (100 s ago), account 2 after (5 s ago)
    assert_eq!(ids(&o), vec![(1, Tier::P1)]);
    assert!(
        o.jobs[0]
            .decision
            .reason_codes
            .contains(&ReasonCode::CoverageGap)
    );

    repo.set_flag(
        42161,
        pool(),
        mev_liq_state::FLAG_FULL_REVERIFICATION,
        None,
        "implementation changed",
        NOW - SEC,
    )
    .unwrap();
    let o = schedule_from(&repo, NOW);
    assert_eq!(
        o.candidates.p1, 2,
        "the flag reaches every account checked before it"
    );
    let _ = WAD;
}

#[test]
fn shadow_pass_reports_counters_and_job_list() {
    let d = tempfile::tempdir().unwrap();
    let repo = SnapshotRepository::open(&d.path().join("liq.sqlite")).unwrap();
    seed(&repo, 1, 900, 10, 6);
    seed(&repo, 2, 3000, 10, 290);
    let r = run_shadow_pass(&repo, &params(), NOW).unwrap();
    assert_eq!(r.counters.candidates.p0, 1);
    assert_eq!(r.counters.candidates.p1, 1);
    assert_eq!(r.counters.dispatched, 2);
    assert_eq!(r.jobs.len(), 2);
    assert!(!r.needs_alert());
    // overdue P1 -> alert
    let r = run_shadow_pass(&repo, &params(), NOW + 100 * SEC).unwrap();
    assert!(r.counters.deadline_misses_p1 >= 1);
    assert!(r.needs_alert());
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["jobs"][0]["decision"]["tier"], "P0");
}
