//! Event-index state: registry, reverse index, coalescing, fan-out bound, reorg rewind, gaps,
//! flags and the v2 migration with its rollback.

use alloy::primitives::{Address, B256, U256};
use mev_liq_state::*;

const CHAIN: u64 = 42161;

fn pool() -> Address {
    Address::repeat_byte(0xaa)
}
fn who(n: u8) -> Address {
    Address::repeat_byte(n)
}
fn asset(n: u8) -> Address {
    Address::repeat_byte(0xe0 + n)
}
fn pt(n: u64, fork: u8) -> ChainPoint {
    let mut h = [0u8; 32];
    h[0] = fork;
    h[24..].copy_from_slice(&n.to_be_bytes());
    ChainPoint {
        block_number: n,
        block_hash: B256::from(h),
    }
}
fn repo() -> (tempfile::TempDir, SnapshotRepository) {
    let d = tempfile::tempdir().unwrap();
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    (d, r)
}
fn batch(name: &str, effects: Vec<Effect>) -> ScanBatch {
    let mut b = ScanBatch::new(name, CHAIN, pool(), 1_000, 10_000);
    b.effects = effects;
    b
}
fn borrow(n: u8, at: ChainPoint) -> Vec<Effect> {
    vec![
        Effect::Touch {
            borrower: who(n),
            reason: DirtyReason::BorrowerEvent,
            at,
            register: Some(at),
            stale_deps: false,
        },
        Effect::DepAdd {
            borrower: who(n),
            asset: asset(1),
            kind: KIND_DEBT,
        },
    ]
}
fn dirty(r: &SnapshotRepository, n: u8) -> Vec<DirtyReason> {
    r.dirty_entries()
        .unwrap()
        .into_iter()
        .find(|e| e.key.borrower == who(n))
        .map(|e| {
            let mut v = e.reasons;
            v.sort();
            v
        })
        .unwrap_or_default()
}

#[test]
fn migration_two_applies_and_rolls_back_without_touching_version_one_data() {
    let (d, r) = repo();
    assert_eq!(r.schema_version().unwrap(), SCHEMA_VERSION);
    r.mark_dirty(
        DirtyTarget::Account(AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(1),
        }),
        DirtyReason::Manual,
        5,
    )
    .unwrap();
    r.apply_scan(&batch("n", borrow(2, pt(10, 1)))).unwrap();
    r.rollback_to(1).unwrap();
    assert_eq!(r.schema_version().unwrap(), 1);
    // v1 data (the dirty mark) survives; the v2 tables are gone.
    assert_eq!(r.list_dirty().unwrap().len(), 2);
    assert!(r.scan_cursor("n").is_err());
    drop(r);
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    assert_eq!(
        r.schema_version().unwrap(),
        SCHEMA_VERSION,
        "reopening re-applies the migrations"
    );
    assert_eq!(r.list_dirty().unwrap().len(), 2);
    assert!(r.scan_cursor("n").unwrap().is_none());
}

#[test]
fn new_borrower_is_registered_and_marked_discovered_and_event_dirty() {
    let (_d, r) = repo();
    let out = r.apply_scan(&batch("n", borrow(1, pt(10, 1)))).unwrap();
    assert_eq!(out.registered, 1);
    assert_eq!(
        dirty(&r, 1),
        vec![DirtyReason::BorrowerEvent, DirtyReason::Discovered]
    );
    let rec = r.borrower(CHAIN, pool(), who(1)).unwrap().unwrap();
    assert!(rec.active && !rec.deps_reconciled);
    assert_eq!(rec.discovered_block, 10);
    // Same borrower again: no second registration, no duplicate row.
    let out = r.apply_scan(&batch("n", borrow(1, pt(11, 1)))).unwrap();
    assert_eq!(out.registered, 0);
    assert_eq!(r.active_borrowers(CHAIN, pool()).unwrap(), vec![who(1)]);
    // The latest chain position wins on the coalesced mark.
    let e = r.dirty_entries().unwrap();
    assert_eq!(e[0].latest_chain, Some(pt(11, 1)));
}

#[test]
fn events_for_unknown_borrowers_are_ignored_unless_they_register() {
    let (_d, r) = repo();
    let touch = |register: bool| Effect::Touch {
        borrower: who(9),
        reason: DirtyReason::BorrowerEvent,
        at: pt(5, 1),
        register: register.then_some(pt(5, 1)),
        stale_deps: true,
    };
    r.apply_scan(&batch("n", vec![touch(false)])).unwrap();
    assert!(r.borrower(CHAIN, pool(), who(9)).unwrap().is_none());
    assert!(r.list_dirty().unwrap().is_empty());
    r.apply_scan(&batch("n", vec![touch(true)])).unwrap();
    assert!(r.borrower(CHAIN, pool(), who(9)).unwrap().is_some());
}

#[test]
fn reverse_index_follows_collateral_toggles_and_fans_out_to_holders_only() {
    let (_d, r) = repo();
    let mut e = borrow(1, pt(10, 1));
    e.extend(borrow(2, pt(10, 1)));
    r.apply_scan(&batch("n", e)).unwrap();
    // Reconcile both: borrower 1 holds asset 2 as collateral, borrower 2 holds asset 3.
    for (n, a) in [(1u8, 2u8), (2, 3)] {
        let o = r
            .reconcile_borrower(
                CHAIN,
                pool(),
                who(n),
                &[(asset(1), KIND_DEBT), (asset(a), KIND_COLLATERAL)],
                true,
                pt(12, 1),
            )
            .unwrap();
        assert_eq!(o, ReconcileOutcome::Active);
    }
    // Clear marks so the fan-out is observable.
    for n in [1u8, 2] {
        let _ = n;
    }
    assert_eq!(
        r.asset_borrowers(CHAIN, pool(), asset(2)).unwrap(),
        vec![(who(1), KIND_COLLATERAL)]
    );
    assert_eq!(r.asset_borrowers(CHAIN, pool(), asset(1)).unwrap().len(), 2);

    // Disabling collateral on asset 2 removes the dependency (kind bit only).
    r.apply_scan(&batch(
        "n",
        vec![Effect::DepRemove {
            borrower: who(1),
            asset: asset(2),
            kind: KIND_COLLATERAL,
        }],
    ))
    .unwrap();
    assert!(
        r.asset_borrowers(CHAIN, pool(), asset(2))
            .unwrap()
            .is_empty()
    );
    // A dependency that carries both kinds keeps the other one.
    r.apply_scan(&batch(
        "n",
        vec![
            Effect::DepAdd {
                borrower: who(2),
                asset: asset(3),
                kind: KIND_DEBT,
            },
            Effect::DepRemove {
                borrower: who(2),
                asset: asset(3),
                kind: KIND_COLLATERAL,
            },
        ],
    ))
    .unwrap();
    assert_eq!(
        r.asset_borrowers(CHAIN, pool(), asset(3)).unwrap(),
        vec![(who(2), KIND_DEBT)]
    );

    // Asset-level mark reaches only holders (both reconciled): asset 3 -> borrower 2 only.
    let before = dirty(&r, 1);
    let o = r
        .apply_scan(&batch(
            "n",
            vec![Effect::AssetDirty {
                asset: asset(3),
                reason: DirtyReason::ReserveConfig,
                at: Some(pt(13, 1)),
            }],
        ))
        .unwrap();
    assert_eq!(o.marked, 1);
    assert_eq!(dirty(&r, 1), before);
    assert!(dirty(&r, 2).contains(&DirtyReason::ReserveConfig));
}

#[test]
fn unreconciled_borrowers_count_as_holders_of_everything() {
    let (_d, r) = repo();
    r.apply_scan(&batch("n", borrow(1, pt(10, 1)))).unwrap();
    let o = r
        .mark_asset_holders(
            CHAIN,
            pool(),
            asset(7),
            DirtyReason::PriceMove,
            None,
            9,
            100,
        )
        .unwrap();
    assert_eq!(
        o.marked, 1,
        "no dependency set yet => affected by any asset"
    );
}

#[test]
fn zero_debt_borrower_is_deactivated_and_leaves_the_index_until_it_borrows_again() {
    let (_d, r) = repo();
    r.apply_scan(&batch("n", borrow(1, pt(10, 1)))).unwrap();
    r.reconcile_borrower(
        CHAIN,
        pool(),
        who(1),
        &[(asset(1), KIND_DEBT)],
        true,
        pt(11, 1),
    )
    .unwrap();
    let o = r
        .reconcile_borrower(
            CHAIN,
            pool(),
            who(1),
            &[(asset(2), KIND_COLLATERAL)],
            false,
            pt(12, 1),
        )
        .unwrap();
    assert_eq!(o, ReconcileOutcome::Deactivated);
    assert!(r.active_borrowers(CHAIN, pool()).unwrap().is_empty());
    assert!(
        r.asset_borrowers(CHAIN, pool(), asset(1))
            .unwrap()
            .is_empty()
    );
    assert!(
        r.asset_borrowers(CHAIN, pool(), asset(2))
            .unwrap()
            .is_empty()
    );
    let o = r
        .mark_asset_holders(
            CHAIN,
            pool(),
            asset(1),
            DirtyReason::PriceMove,
            None,
            99,
            100,
        )
        .unwrap();
    assert_eq!(o.marked, 0, "inactive borrowers are not rechecked");

    // A new Borrow reactivates it and its dependency set must be rebuilt.
    r.apply_scan(&batch("n", borrow(1, pt(20, 1)))).unwrap();
    let rec = r.borrower(CHAIN, pool(), who(1)).unwrap().unwrap();
    assert!(rec.active && !rec.deps_reconciled);
}

#[test]
fn reconcile_refuses_data_older_than_the_newest_touching_event() {
    let (_d, r) = repo();
    r.apply_scan(&batch("n", borrow(1, pt(30, 1)))).unwrap();
    let o = r
        .reconcile_borrower(CHAIN, pool(), who(1), &[], false, pt(25, 1))
        .unwrap();
    assert_eq!(o, ReconcileOutcome::Superseded);
    assert!(r.borrower(CHAIN, pool(), who(1)).unwrap().unwrap().active);
    assert_eq!(
        r.reconcile_borrower(CHAIN, pool(), who(77), &[], false, pt(25, 1))
            .unwrap(),
        ReconcileOutcome::Unknown
    );
}

#[test]
fn coalescing_keeps_the_most_urgent_reason_and_latest_chain_ref() {
    let mut c = Coalescer::new(10);
    c.push_account(who(1), DirtyReason::Discovered, pt(5, 1), true, false);
    c.push_account(who(1), DirtyReason::PriceMove, pt(9, 1), false, true);
    c.push_account(who(1), DirtyReason::BorrowerEvent, pt(7, 1), false, false);
    c.push_account(who(2), DirtyReason::Reorg, pt(3, 1), false, false);
    let fx = c.into_effects();
    assert_eq!(fx.len(), 2);
    // Most urgent account first.
    assert!(matches!(
        &fx[0],
        Effect::Touch { borrower, reason: DirtyReason::Reorg, .. } if *borrower == who(2)
    ));
    assert!(matches!(
        &fx[1],
        Effect::Touch {
            borrower,
            reason: DirtyReason::PriceMove,
            at,
            register: Some(first),
            stale_deps: true,
        } if *borrower == who(1) && at.block_number == 9 && first.block_number == 5
    ));
}

#[test]
fn overload_escalates_to_all_borrowers_and_never_drops_a_trigger() {
    let (_d, r) = repo();
    let mut regs = vec![];
    for n in 1..=8u8 {
        regs.extend(borrow(n, pt(10, 1)));
    }
    r.apply_scan(&batch("n", regs)).unwrap();
    // Clear every mark by "checking" nothing: simply count after the escalation.
    let mut c = Coalescer::new(3);
    for n in 1..=8u8 {
        c.push_account(
            who(n),
            DirtyReason::PriceMove,
            pt(20 + u64::from(n), 1),
            false,
            false,
        );
    }
    assert_eq!(c.overflowed(), 5);
    let fx = c.into_effects();
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::AllDirty {
            reason: DirtyReason::PriceMove,
            ..
        }
    )));
    r.apply_scan(&batch("n", fx)).unwrap();
    for n in 1..=8u8 {
        assert!(
            dirty(&r, n).contains(&DirtyReason::PriceMove),
            "borrower {n} must be marked even though it overflowed the cap"
        );
    }
    assert!(r.has_flag(CHAIN, pool(), FLAG_BACKLOG_ESCALATED).unwrap());
}

#[test]
fn fanout_is_bounded_per_call_durable_and_drains_riskiest_first() {
    let (d, r) = repo();
    let n_borrowers = 30u8;
    let mut e = vec![];
    for n in 1..=n_borrowers {
        e.extend(borrow(n, pt(10, 1)));
    }
    r.apply_scan(&batch("n", e)).unwrap();
    for n in 1..=n_borrowers {
        r.reconcile_borrower(
            CHAIN,
            pool(),
            who(n),
            &[(asset(1), KIND_DEBT)],
            true,
            pt(11, 1),
        )
        .unwrap();
    }
    // Risk order: higher n = lower health factor. Give them snapshots.
    for n in 1..=n_borrowers {
        let mut s = BorrowerSnapshot {
            key: AccountKey {
                chain_id: CHAIN,
                pool: pool(),
                borrower: who(n),
            },
            revision: 0,
            chain: ChainRef {
                block_number: 12,
                block_hash: pt(12, 1).block_hash,
                source: ChainSource::Provisional,
                observed_at_ns: 10_000,
            },
            health_factor_wad: WAD + U256::from(1000u32 - u32::from(n)),
            total_debt_base: U256::from(1u8),
            total_collateral_base: U256::from(2u8),
            available_borrows_base: U256::ZERO,
            liquidation_threshold: U256::ZERO,
            ltv: U256::ZERO,
            reserves: vec![],
            risk_metadata_version: 1,
            dirty_reasons: vec![],
            last_successful_check_ns: Some(10_000),
            status: SnapshotStatus::Valid,
            provenance: "t".into(),
            feature_digest: B256::ZERO,
        };
        s = s.seal();
        r.upsert_cas(&s, 0).unwrap();
    }
    assert!(
        r.list_dirty().unwrap().is_empty(),
        "rechecks cleared the marks"
    );

    let mut b = batch(
        "n",
        vec![Effect::AssetDirty {
            asset: asset(1),
            reason: DirtyReason::PriceMove,
            at: Some(pt(40, 1)),
        }],
    );
    b.fanout_limit = 10;
    b.now_ns = 5_000;
    let o = r.apply_scan(&b).unwrap();
    assert_eq!((o.marked, o.queued), (10, 20), "bounded immediate fan-out");
    let first: Vec<_> = r.list_dirty().unwrap().iter().map(|k| k.borrower).collect();
    assert_eq!(first.len(), 10);
    assert!(
        first.contains(&who(n_borrowers)),
        "riskiest borrower marked first"
    );
    assert!(
        !first.contains(&who(1)),
        "safest borrower waits in the queue"
    );
    assert_eq!(r.fanout_pending(CHAIN, pool()).unwrap(), 20);

    // The queue survives a restart, and draining is bounded too.
    drop(r);
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    assert_eq!(r.fanout_pending(CHAIN, pool()).unwrap(), 20);
    assert_eq!(r.drain_fanout(CHAIN, pool(), 15).unwrap(), 15);
    assert_eq!(r.list_dirty().unwrap().len(), 25);
    assert_eq!(r.drain_fanout(CHAIN, pool(), 15).unwrap(), 5);
    assert_eq!(r.list_dirty().unwrap().len(), 30, "nothing was dropped");
    assert_eq!(r.fanout_pending(CHAIN, pool()).unwrap(), 0);
    // Marks keep the trigger's time and chain position.
    let e = r.dirty_entries().unwrap();
    assert!(
        e.iter()
            .all(|e| e.marked_at_ns == 5_000 && e.latest_chain == Some(pt(40, 1)))
    );
}

#[test]
fn dirty_entries_are_ordered_by_urgency_with_most_urgent_reason() {
    let (_d, r) = repo();
    let mut e = vec![];
    for n in 1..=3u8 {
        e.extend(borrow(n, pt(10, 1)));
    }
    r.apply_scan(&batch("n", e)).unwrap();
    let k = |n: u8| AccountKey {
        chain_id: CHAIN,
        pool: pool(),
        borrower: who(n),
    };
    r.mark_dirty(DirtyTarget::Account(k(2)), DirtyReason::PriceMove, 50)
        .unwrap();
    r.mark_dirty(DirtyTarget::Account(k(3)), DirtyReason::Reorg, 60)
        .unwrap();
    let e = r.dirty_entries().unwrap();
    assert_eq!(e[0].key, k(3));
    assert_eq!(e[0].most_urgent, DirtyReason::Reorg);
    assert_eq!(e[1].key, k(2));
    assert_eq!(e[1].most_urgent, DirtyReason::PriceMove);
    assert_eq!(e[2].most_urgent, DirtyReason::BorrowerEvent);
}

#[test]
fn cursor_ancestry_and_gaps_commit_atomically_with_the_marks() {
    let (_d, r) = repo();
    r.init_scan_cursor(
        "n",
        ScanCursor {
            next_block: 100,
            tip: Some(pt(99, 1)),
            back_block: 100,
        },
    )
    .unwrap();
    // Re-init never moves an existing cursor.
    let c = r
        .init_scan_cursor(
            "n",
            ScanCursor {
                next_block: 5,
                tip: None,
                back_block: 5,
            },
        )
        .unwrap();
    assert_eq!(c.next_block, 100);

    let mut b = batch("n", borrow(1, pt(105, 1)));
    b.next_block = Some(111);
    b.tip = Some(pt(110, 1));
    b.ancestry = vec![pt(105, 1)];
    b.new_gaps = vec![NewGap {
        dir: ScanDir::Backward,
        from_block: 10,
        to_block: 20,
        reason: "429".into(),
    }];
    r.apply_scan(&b).unwrap();
    let c = r.scan_cursor("n").unwrap().unwrap();
    assert_eq!((c.next_block, c.tip), (111, Some(pt(110, 1))));
    let anc = r.scan_ancestry("n", 10).unwrap();
    assert_eq!(anc, vec![pt(110, 1), pt(105, 1)]);
    let gaps = r.open_gaps("n").unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(
        (gaps[0].from_block, gaps[0].to_block, gaps[0].dir),
        (10, 20, ScanDir::Backward)
    );

    let mut b = batch("n", vec![]);
    b.resolved_gaps = vec![gaps[0].id];
    r.apply_scan(&b).unwrap();
    assert!(r.open_gaps("n").unwrap().is_empty());
}

#[test]
fn reorg_rewind_drops_orphan_discoveries_flags_touched_borrowers_and_invalidates_snapshots() {
    let (_d, r) = repo();
    r.init_scan_cursor(
        "n",
        ScanCursor {
            next_block: 100,
            tip: Some(pt(99, 1)),
            back_block: 100,
        },
    )
    .unwrap();
    // Borrower 1 known from before the fork; borrower 2 discovered on the orphaned block 106;
    // borrower 1 also touched on the orphaned block.
    let mut b = batch("n", borrow(1, pt(101, 1)));
    b.next_block = Some(102);
    b.tip = Some(pt(101, 1));
    r.apply_scan(&b).unwrap();
    r.reconcile_borrower(
        CHAIN,
        pool(),
        who(1),
        &[(asset(1), KIND_DEBT)],
        true,
        pt(101, 1),
    )
    .unwrap();
    let mut e = borrow(2, pt(106, 1));
    e.push(Effect::Touch {
        borrower: who(1),
        reason: DirtyReason::BorrowerEvent,
        at: pt(106, 1),
        register: None,
        stale_deps: true,
    });
    let mut b = batch("n", e);
    b.next_block = Some(111);
    b.tip = Some(pt(110, 1));
    b.ancestry = vec![pt(106, 1)];
    r.apply_scan(&b).unwrap();

    // A snapshot of borrower 1 read on the orphaned fork at block 108.
    let s = BorrowerSnapshot {
        key: AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(1),
        },
        revision: 0,
        chain: ChainRef {
            block_number: 108,
            block_hash: pt(108, 1).block_hash,
            source: ChainSource::Provisional,
            observed_at_ns: 10_000,
        },
        health_factor_wad: WAD + U256::from(5u8),
        total_debt_base: U256::from(1u8),
        total_collateral_base: U256::from(2u8),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::ZERO,
        ltv: U256::ZERO,
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(10_000),
        status: SnapshotStatus::Valid,
        provenance: "t".into(),
        feature_digest: B256::ZERO,
    }
    .seal();
    r.upsert_cas(&s, 0).unwrap();
    assert!(dirty(&r, 1).is_empty());

    // Common ancestor is 103 (fork 1); everything above was on the dropped fork.
    let out = r
        .rewind_scan("n", CHAIN, pool(), pt(103, 1), 20_000)
        .unwrap();
    assert_eq!(out.removed_borrowers, 1);
    assert_eq!(out.touched_marked, 1);
    assert_eq!(out.invalidated_snapshots, 1);
    assert!(
        r.borrower(CHAIN, pool(), who(2)).unwrap().is_none(),
        "orphan discovery removed"
    );
    let rec = r.borrower(CHAIN, pool(), who(1)).unwrap().unwrap();
    assert!(rec.deps_stale && rec.active);
    assert!(dirty(&r, 1).contains(&DirtyReason::Reorg));
    let snap = r
        .fetch_by_key(&AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(1),
        })
        .unwrap()
        .unwrap();
    assert_eq!(snap.status, SnapshotStatus::Invalidated);
    let c = r.scan_cursor("n").unwrap().unwrap();
    assert_eq!((c.next_block, c.tip), (104, Some(pt(103, 1))));
    assert!(
        r.scan_ancestry("n", 10)
            .unwrap()
            .iter()
            .all(|p| p.block_number <= 103)
    );
}

#[test]
fn pool_flags_set_clear_and_survive_reopen() {
    let (d, r) = repo();
    r.set_flag(
        CHAIN,
        pool(),
        FLAG_FULL_REVERIFICATION,
        Some(pt(5, 1)),
        "impl changed",
        7,
    )
    .unwrap();
    drop(r);
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    assert!(r.has_flag(CHAIN, pool(), FLAG_FULL_REVERIFICATION).unwrap());
    let f = &r.flags(CHAIN, pool()).unwrap()[0];
    assert_eq!(
        (f.block_number, f.detail.as_str()),
        (Some(5), "impl changed")
    );
    assert!(
        r.clear_flag(CHAIN, pool(), FLAG_FULL_REVERIFICATION)
            .unwrap()
    );
    assert!(!r.has_flag(CHAIN, pool(), FLAG_FULL_REVERIFICATION).unwrap());
}

#[test]
fn price_baselines_persist_and_moves_mark_holders() {
    let (_d, r) = repo();
    r.apply_scan(&batch("n", borrow(1, pt(10, 1)))).unwrap();
    assert!(r.oracle_price(CHAIN, pool(), asset(1)).unwrap().is_none());
    r.seed_prices(CHAIN, pool(), &[(asset(1), U256::from(100u8))], None)
        .unwrap();
    r.seed_prices(CHAIN, pool(), &[(asset(1), U256::from(999u16))], None)
        .unwrap();
    assert_eq!(
        r.oracle_price(CHAIN, pool(), asset(1)).unwrap(),
        Some(U256::from(100u8))
    );
    let o = r
        .record_price_moves(
            CHAIN,
            pool(),
            &[(asset(1), U256::from(101u8))],
            Some(pt(50, 1)),
            77,
            10,
        )
        .unwrap();
    assert_eq!(o.marked, 1);
    assert_eq!(
        r.oracle_price(CHAIN, pool(), asset(1)).unwrap(),
        Some(U256::from(101u8))
    );
    assert!(dirty(&r, 1).contains(&DirtyReason::PriceMove));
}
