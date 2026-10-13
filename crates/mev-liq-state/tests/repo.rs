use std::sync::{Arc, Barrier};

use alloy::primitives::{Address, B256, U256};
use mev_liq_state::*;
use proptest::prelude::*;

type Mutation = Box<dyn Fn(&mut BorrowerSnapshot)>;

fn key(n: u8) -> AccountKey {
    AccountKey {
        chain_id: 42161,
        pool: Address::repeat_byte(0xaa),
        borrower: Address::repeat_byte(n),
    }
}

fn snap(n: u8, block: u64, source: ChainSource) -> BorrowerSnapshot {
    BorrowerSnapshot {
        key: key(n),
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: B256::repeat_byte(block as u8),
            source,
            observed_at_ns: block * 1_000,
        },
        health_factor_wad: WAD + U256::from(5u8),
        total_debt_base: U256::from(100u8),
        total_collateral_base: U256::from(200u8),
        available_borrows_base: U256::from(7u8),
        liquidation_threshold: U256::from(8000u16),
        ltv: U256::from(7500u16),
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(block * 1_000),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

fn repo() -> (tempfile::TempDir, SnapshotRepository) {
    let d = tempfile::tempdir().unwrap();
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    (d, r)
}

#[test]
fn eligibility_is_exact_at_the_wad_boundary() {
    let mut s = snap(1, 10, ChainSource::Provisional);
    s.health_factor_wad = WAD;
    assert!(!s.eligible(), "HF == 1e18 is not liquidatable");
    s.health_factor_wad = WAD - U256::from(1u8);
    assert!(s.eligible(), "HF == 1e18-1 is");
    s.health_factor_wad = WAD + U256::from(1u8);
    assert!(!s.eligible());
}

#[test]
fn zero_debt_max_hf_is_not_eligible() {
    let mut s = snap(1, 10, ChainSource::Provisional);
    s.health_factor_wad = U256::MAX;
    s.total_debt_base = U256::ZERO;
    assert!(!s.eligible());
    // Even a (nonsensical) low HF with zero debt is never eligible.
    s.health_factor_wad = U256::ZERO;
    assert!(!s.eligible());
}

#[test]
fn max_u256_round_trips_through_storage() {
    let (_d, r) = repo();
    let mut s = snap(1, 10, ChainSource::Provisional);
    s.health_factor_wad = U256::MAX;
    s.total_debt_base = U256::MAX;
    s.total_collateral_base = U256::MAX;
    s.available_borrows_base = U256::MAX;
    s.reserves = vec![ReserveBalance {
        asset: Address::repeat_byte(9),
        collateral_raw: U256::MAX,
        debt_raw: U256::from(1u8),
    }];
    let s = s.seal();
    let stored = r.upsert_cas(&s, 0).unwrap();
    assert_eq!(stored.revision, 1);
    let got = r.fetch_by_key(&s.key).unwrap().unwrap();
    assert_eq!(got.health_factor_wad, U256::MAX);
    assert_eq!(got.reserves, s.reserves);
    assert_eq!(got.feature_digest, s.feature_digest);
    assert!(got.digest_matches());
}

#[test]
fn cas_rejects_stale_and_advances_monotonically() {
    let (_d, r) = repo();
    let s = snap(1, 10, ChainSource::Provisional);
    assert_eq!(r.upsert_cas(&s, 0).unwrap().revision, 1);
    assert!(matches!(
        r.upsert_cas(&s, 0),
        Err(RepoError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    assert!(matches!(
        r.upsert_cas(&s, 5),
        Err(RepoError::Stale {
            expected: 5,
            actual: 1
        })
    ));
    let s2 = snap(1, 11, ChainSource::Provisional);
    assert_eq!(r.upsert_cas(&s2, 1).unwrap().revision, 2);
    // Out-of-order (older block) is rejected even with the right revision.
    assert!(matches!(
        r.upsert_cas(&s, 2),
        Err(RepoError::OutOfOrder {
            stored_block: 11,
            new_block: 10
        })
    ));
    assert_eq!(r.fetch_by_key(&s.key).unwrap().unwrap().revision, 2);
}

#[test]
fn tampered_digest_is_rejected() {
    let (_d, r) = repo();
    let mut s = snap(1, 10, ChainSource::Provisional);
    s.total_debt_base = U256::from(999u16);
    assert!(matches!(
        r.upsert_cas(&s, 0),
        Err(RepoError::DigestMismatch)
    ));
}

#[test]
fn concurrent_upserts_exactly_one_wins() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.db");
    SnapshotRepository::open(&path).unwrap();
    for round in 0..5u8 {
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2u8)
            .map(|t| {
                let (path, barrier) = (path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let r = SnapshotRepository::open(&path).unwrap();
                    let mut s = snap(round, 10 + u64::from(t), ChainSource::Provisional);
                    s.total_debt_base = U256::from(t) + U256::from(1u8);
                    let s = s.seal();
                    barrier.wait();
                    r.upsert_cas(&s, 0).map(|x| x.revision)
                })
            })
            .collect();
        let res: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = res.iter().filter(|r| r.is_ok()).count();
        assert_eq!(wins, 1, "round {round}: {res:?}");
        assert!(res.iter().any(|r| matches!(
            r,
            Err(RepoError::Stale {
                expected: 0,
                actual: 1
            })
        )));
    }
}

#[test]
fn state_survives_restart() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.db");
    let s = snap(1, 10, ChainSource::Provisional);
    {
        let r = SnapshotRepository::open(&path).unwrap();
        r.upsert_cas(&s, 0).unwrap();
        r.mark_dirty(DirtyTarget::Account(s.key), DirtyReason::PriceMove, 20_000)
            .unwrap();
        r.checkpoint_canonical(ChainPoint {
            block_number: 10,
            block_hash: s.chain.block_hash,
        })
        .unwrap();
    }
    let r = SnapshotRepository::open(&path).unwrap();
    let got = r.fetch_by_key(&s.key).unwrap().unwrap();
    assert_eq!(got.revision, 2);
    assert_eq!(got.chain.source, ChainSource::Canonical);
    assert_eq!(got.dirty_reasons, vec![DirtyReason::PriceMove]);
    assert!(got.digest_matches());
    assert_eq!(r.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn provisional_is_never_promoted_without_checkpoint() {
    let (_d, r) = repo();
    let canon = snap(1, 10, ChainSource::Canonical);
    assert!(matches!(
        r.upsert_cas(&canon, 0),
        Err(RepoError::NotCheckpointed(_))
    ));
    let prov = snap(1, 10, ChainSource::Provisional);
    r.upsert_cas(&prov, 0).unwrap();
    // Re-labelling the same read canonical is refused until the block is checkpointed.
    assert!(matches!(
        r.upsert_cas(&canon, 1),
        Err(RepoError::NotCheckpointed(_))
    ));
    let n = r
        .checkpoint_canonical(ChainPoint {
            block_number: 10,
            block_hash: prov.chain.block_hash,
        })
        .unwrap();
    assert_eq!(n, 1);
    let got = r.fetch_by_key(&prov.key).unwrap().unwrap();
    assert_eq!(got.chain.source, ChainSource::Canonical);
    assert_eq!(got.revision, 2);
    assert!(got.digest_matches());
    // A canonical row cannot be downgraded back to provisional at the same block.
    assert!(matches!(
        r.upsert_cas(&prov, 2),
        Err(RepoError::OutOfOrder { .. })
    ));
    // A checkpoint at the wrong height for a hash does not authorize canonical writes.
    let wrong = snap(2, 12, ChainSource::Canonical);
    r.checkpoint_canonical(ChainPoint {
        block_number: 99,
        block_hash: wrong.chain.block_hash,
    })
    .unwrap();
    assert!(matches!(
        r.upsert_cas(&wrong, 0),
        Err(RepoError::NotCheckpointed(_))
    ));
}

#[test]
fn reorg_invalidates_provisional_and_orphaned_rows_keeps_ancestor_canon() {
    let (_d, r) = repo();
    // canonical at 10 (survives), provisional at 11 and 12 (dropped), canonical at 12 (dropped fork).
    let a = snap(1, 10, ChainSource::Provisional);
    r.upsert_cas(&a, 0).unwrap();
    r.checkpoint_canonical(ChainPoint {
        block_number: 10,
        block_hash: a.chain.block_hash,
    })
    .unwrap();
    r.upsert_cas(&snap(2, 11, ChainSource::Provisional), 0)
        .unwrap();
    r.upsert_cas(&snap(3, 12, ChainSource::Provisional), 0)
        .unwrap();
    let c12 = snap(4, 12, ChainSource::Provisional);
    r.upsert_cas(&c12, 0).unwrap();
    r.checkpoint_canonical(ChainPoint {
        block_number: 12,
        block_hash: c12.chain.block_hash,
    })
    .unwrap();
    let before = r.fetch_by_key(&key(1)).unwrap().unwrap();

    let n = r
        .invalidate_from_reorg(
            ChainPoint {
                block_number: 10,
                block_hash: a.chain.block_hash,
            },
            50_000,
        )
        .unwrap();
    assert_eq!(n, 3);
    let survivor = r.fetch_by_key(&key(1)).unwrap().unwrap();
    assert_eq!(survivor, before);
    assert_eq!(survivor.status, SnapshotStatus::Valid);
    for b in [2u8, 3, 4] {
        let s = r.fetch_by_key(&key(b)).unwrap().unwrap();
        assert_eq!(s.status, SnapshotStatus::Invalidated);
        assert_eq!(s.revision, if b >= 3 { 3 } else { 2 }); // 3 and 4 share block 12: promoted, then invalidated
        assert!(s.dirty_reasons.contains(&DirtyReason::Reorg));
    }
    // The 12-height checkpoint is gone: canonical writes there are refused again.
    assert!(matches!(
        r.upsert_cas(&snap(5, 12, ChainSource::Canonical), 0),
        Err(RepoError::NotCheckpointed(_))
    ));
    // Invalidated rows are expired; a fresh recheck at the next revision clears the Reorg mark.
    let exp = r.list_expired(60_000, u64::MAX).unwrap();
    assert_eq!(exp, vec![key(2), key(3), key(4)]);
    let mut fresh = snap(2, 11, ChainSource::Provisional);
    fresh.chain.observed_at_ns = 60_000;
    let stored = r.upsert_cas(&fresh.seal(), 2).unwrap();
    assert_eq!(stored.status, SnapshotStatus::Valid);
    assert!(stored.dirty_reasons.is_empty());
}

#[test]
fn list_expired_boundaries() {
    let (_d, r) = repo();
    let mut s = snap(1, 10, ChainSource::Provisional);
    s.last_successful_check_ns = Some(1_000);
    r.upsert_cas(&s.clone().seal(), 0).unwrap();
    let mut never = snap(2, 10, ChainSource::Provisional);
    never.last_successful_check_ns = None;
    r.upsert_cas(&never.seal(), 0).unwrap();
    // age 99 / 100 / 101 with max_age 100
    assert_eq!(r.list_expired(1_099, 100).unwrap(), vec![key(2)]);
    assert_eq!(r.list_expired(1_100, 100).unwrap(), vec![key(2)]);
    assert_eq!(r.list_expired(1_101, 100).unwrap(), vec![key(1), key(2)]);
    // clock behind the check time: not expired.
    assert_eq!(r.list_expired(500, 100).unwrap(), vec![key(2)]);
}

#[test]
fn mark_dirty_by_asset_and_account() {
    let (_d, r) = repo();
    let weth = Address::repeat_byte(0x11);
    let usdc = Address::repeat_byte(0x22);
    let mut a = snap(1, 10, ChainSource::Provisional);
    a.reserves = vec![ReserveBalance {
        asset: weth,
        collateral_raw: U256::from(1u8),
        debt_raw: U256::ZERO,
    }];
    r.upsert_cas(&a.seal(), 0).unwrap();
    let mut b = snap(2, 10, ChainSource::Provisional);
    b.reserves = vec![ReserveBalance {
        asset: usdc,
        collateral_raw: U256::ZERO,
        debt_raw: U256::from(1u8),
    }];
    r.upsert_cas(&b.seal(), 0).unwrap();
    r.upsert_cas(&snap(3, 10, ChainSource::Provisional), 0)
        .unwrap(); // unknown composition
    let pool = Address::repeat_byte(0xaa);
    let n = r
        .mark_dirty(
            DirtyTarget::Asset {
                chain_id: 42161,
                pool,
                asset: weth,
            },
            DirtyReason::PriceMove,
            20_000,
        )
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(r.list_dirty().unwrap(), vec![key(1), key(3)]);
    // A recheck observed after the mark clears it; one observed before does not.
    let mut old = snap(1, 11, ChainSource::Provisional);
    old.chain.observed_at_ns = 19_999;
    r.upsert_cas(&old.seal(), 1).unwrap();
    assert!(r.list_dirty().unwrap().contains(&key(1)));
    let mut newer = snap(1, 12, ChainSource::Provisional);
    newer.chain.observed_at_ns = 20_000;
    r.upsert_cas(&newer.seal(), 2).unwrap();
    assert_eq!(r.list_dirty().unwrap(), vec![key(3)]);
}

#[test]
fn migrations_roll_back_and_reapply() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.db");
    let r = SnapshotRepository::open(&path).unwrap();
    r.upsert_cas(&snap(1, 10, ChainSource::Provisional), 0)
        .unwrap();
    r.rollback_to(0).unwrap();
    assert_eq!(r.schema_version().unwrap(), 0);
    drop(r);
    let r = SnapshotRepository::open(&path).unwrap();
    assert_eq!(r.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(r.fetch_by_key(&key(1)).unwrap().is_none());
}

#[test]
fn digest_is_deterministic_and_sensitive_to_every_digested_field() {
    let base = snap(1, 10, ChainSource::Provisional);
    assert_eq!(
        base.compute_digest(),
        snap(1, 10, ChainSource::Provisional).compute_digest()
    );
    let one = U256::from(1u8);
    let mutations: Vec<(&str, Mutation)> = vec![
        ("chain_id", Box::new(|s| s.key.chain_id += 1)),
        ("pool", Box::new(|s| s.key.pool = Address::repeat_byte(1))),
        (
            "borrower",
            Box::new(|s| s.key.borrower = Address::repeat_byte(0x77)),
        ),
        ("block_number", Box::new(|s| s.chain.block_number += 1)),
        (
            "block_hash",
            Box::new(|s| s.chain.block_hash = B256::repeat_byte(1)),
        ),
        (
            "source",
            Box::new(|s| s.chain.source = ChainSource::Canonical),
        ),
        ("hf", Box::new(move |s| s.health_factor_wad += one)),
        ("debt", Box::new(move |s| s.total_debt_base += one)),
        ("coll", Box::new(move |s| s.total_collateral_base += one)),
        ("avail", Box::new(move |s| s.available_borrows_base += one)),
        ("lt", Box::new(move |s| s.liquidation_threshold += one)),
        ("ltv", Box::new(move |s| s.ltv += one)),
        ("risk", Box::new(|s| s.risk_metadata_version += 1)),
        (
            "reserves",
            Box::new(|s| {
                s.reserves.push(ReserveBalance {
                    asset: Address::ZERO,
                    collateral_raw: U256::ZERO,
                    debt_raw: U256::ZERO,
                })
            }),
        ),
    ];
    for (name, m) in mutations {
        let mut s = base.clone();
        m(&mut s);
        assert_ne!(
            s.compute_digest(),
            base.feature_digest,
            "{name} must change the digest"
        );
    }
    // Reserve fields individually.
    let mut with = base.clone();
    with.reserves = vec![ReserveBalance {
        asset: Address::repeat_byte(1),
        collateral_raw: one,
        debt_raw: one,
    }];
    let d0 = with.compute_digest();
    for f in 0..3 {
        let mut s = with.clone();
        match f {
            0 => s.reserves[0].asset = Address::repeat_byte(2),
            1 => s.reserves[0].collateral_raw += one,
            _ => s.reserves[0].debt_raw += one,
        }
        assert_ne!(s.compute_digest(), d0);
    }
    // Bookkeeping fields are intentionally outside the digest.
    let mut s = base.clone();
    s.revision = 9;
    s.chain.observed_at_ns += 1;
    s.dirty_reasons = vec![DirtyReason::Manual];
    s.last_successful_check_ns = None;
    s.provenance = "x".into();
    s.status = SnapshotStatus::Invalidated;
    assert_eq!(s.compute_digest(), base.feature_digest);
}

fn any_u256() -> impl Strategy<Value = U256> {
    prop::array::uniform4(any::<u64>()).prop_map(U256::from_limbs)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn u256_fields_round_trip(hf in any_u256(), debt in any_u256(), coll in any_u256(), raw in any_u256()) {
        let (_d, r) = repo();
        let mut s = snap(1, 10, ChainSource::Provisional);
        s.health_factor_wad = hf;
        s.total_debt_base = debt;
        s.total_collateral_base = coll;
        s.reserves = vec![ReserveBalance { asset: Address::repeat_byte(3), collateral_raw: raw, debt_raw: raw }];
        let s = s.seal();
        r.upsert_cas(&s, 0).unwrap();
        let got = r.fetch_by_key(&s.key).unwrap().unwrap();
        prop_assert_eq!(got.health_factor_wad, hf);
        prop_assert_eq!(got.total_debt_base, debt);
        prop_assert_eq!(got.total_collateral_base, coll);
        prop_assert_eq!(&got.reserves, &s.reserves);
        prop_assert_eq!(got.eligible(), hf < WAD && !debt.is_zero());
    }

    #[test]
    fn revisions_are_strictly_monotonic(blocks in prop::collection::vec(0u64..40, 1..12)) {
        let (_d, r) = repo();
        let mut rev = 0u64;
        let mut max_block = 0u64;
        for b in blocks {
            let s = snap(1, b, ChainSource::Provisional);
            match r.upsert_cas(&s, rev) {
                Ok(st) => {
                    prop_assert!(b >= max_block);
                    prop_assert_eq!(st.revision, rev + 1);
                    rev = st.revision;
                    max_block = b;
                }
                Err(RepoError::OutOfOrder { .. }) => prop_assert!(b < max_block),
                Err(e) => prop_assert!(false, "unexpected {e}"),
            }
            prop_assert_eq!(r.fetch_by_key(&key(1)).unwrap().unwrap().revision, rev);
        }
    }
}
