//! Snapshot history (migration 3): canonical-only, bounded, reorg-safe, leakage-proof.

use alloy::primitives::{Address, B256, U256};
use mev_liq_state::*;

fn key(n: u8) -> AccountKey {
    AccountKey {
        chain_id: 42161,
        pool: Address::repeat_byte(0xaa),
        borrower: Address::repeat_byte(n),
    }
}

fn hash(block: u64) -> B256 {
    B256::left_padding_from(&block.to_be_bytes())
}

fn snap(n: u8, block: u64, source: ChainSource, hf_extra: u64) -> BorrowerSnapshot {
    BorrowerSnapshot {
        key: key(n),
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: hash(block),
            source,
            observed_at_ns: block * 1_000,
        },
        health_factor_wad: WAD + U256::from(hf_extra),
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

fn canon(r: &SnapshotRepository, n: u8, block: u64, hf_extra: u64) {
    r.checkpoint_canonical(ChainPoint {
        block_number: block,
        block_hash: hash(block),
    })
    .unwrap();
    let rev = r.fetch_by_key(&key(n)).unwrap().map_or(0, |s| s.revision);
    r.upsert_cas(&snap(n, block, ChainSource::Canonical, hf_extra), rev)
        .unwrap();
}

#[test]
fn only_canonical_snapshots_enter_history() {
    let (_d, r) = repo();
    r.upsert_cas(&snap(1, 10, ChainSource::Provisional, 1), 0)
        .unwrap();
    assert_eq!(r.history_len(&key(1)).unwrap(), 0);
    assert!(r.previous_canonical(&key(1), 100).unwrap().is_none());
    // Promotion records it.
    r.checkpoint_canonical(ChainPoint {
        block_number: 10,
        block_hash: hash(10),
    })
    .unwrap();
    let p = r.previous_canonical(&key(1), 11).unwrap().unwrap();
    assert_eq!(p.block_number, 10);
    assert_eq!(p.health_factor_wad, WAD + U256::from(1u8));
    assert_eq!(p.observed_at_ns, 10_000);
}

#[test]
fn previous_canonical_never_returns_a_row_at_or_after_the_cutoff() {
    let (_d, r) = repo();
    for b in [10u64, 20, 30, 40] {
        canon(&r, 1, b, b);
    }
    for cutoff in 0..=45u64 {
        match r.previous_canonical(&key(1), cutoff).unwrap() {
            Some(p) => assert!(p.block_number < cutoff, "cutoff {cutoff} leaked {p:?}"),
            None => assert!(cutoff <= 10),
        }
    }
    assert_eq!(
        r.previous_canonical(&key(1), 30)
            .unwrap()
            .unwrap()
            .block_number,
        20
    );
    assert_eq!(
        r.previous_canonical(&key(1), 31)
            .unwrap()
            .unwrap()
            .block_number,
        30
    );
    // A different account's history is invisible.
    assert!(r.previous_canonical(&key(2), 100).unwrap().is_none());
}

#[test]
fn retention_is_bounded_and_keeps_the_newest() {
    let (_d, r) = repo();
    let n = HISTORY_RETENTION as u64 + 8;
    for b in 1..=n {
        canon(&r, 1, b, b);
    }
    assert_eq!(r.history_len(&key(1)).unwrap(), HISTORY_RETENTION);
    assert_eq!(
        r.previous_canonical(&key(1), n + 1)
            .unwrap()
            .unwrap()
            .block_number,
        n
    );
    // The oldest retained is n - RETENTION + 1; anything older was pruned.
    let oldest = n - HISTORY_RETENTION as u64 + 1;
    assert!(r.previous_canonical(&key(1), oldest).unwrap().is_none());
}

#[test]
fn reorg_removes_history_on_the_dropped_fork() {
    let (_d, r) = repo();
    for b in [10u64, 20, 30] {
        canon(&r, 1, b, b);
    }
    r.invalidate_from_reorg(
        ChainPoint {
            block_number: 20,
            block_hash: hash(20),
        },
        99,
    )
    .unwrap();
    assert_eq!(r.history_len(&key(1)).unwrap(), 2);
    assert_eq!(
        r.previous_canonical(&key(1), 100)
            .unwrap()
            .unwrap()
            .block_number,
        20
    );
    // Same height, different hash: the ancestor at 20 is replaced, so the old 20 row goes too.
    r.invalidate_from_reorg(
        ChainPoint {
            block_number: 20,
            block_hash: B256::repeat_byte(0xee),
        },
        100,
    )
    .unwrap();
    assert_eq!(
        r.previous_canonical(&key(1), 100)
            .unwrap()
            .unwrap()
            .block_number,
        10
    );
}

#[test]
fn migration_three_rolls_back_and_reapplies_without_touching_snapshots() {
    let (d, r) = repo();
    canon(&r, 1, 10, 1);
    assert_eq!(r.schema_version().unwrap(), 3);
    r.rollback_to(2).unwrap();
    assert_eq!(r.schema_version().unwrap(), 2);
    assert!(r.previous_canonical(&key(1), 100).is_err(), "table dropped");
    assert!(
        r.fetch_by_key(&key(1)).unwrap().is_some(),
        "snapshot rows survive"
    );
    drop(r);
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    assert_eq!(r.schema_version().unwrap(), 3);
    assert!(r.previous_canonical(&key(1), 100).unwrap().is_none());
}
