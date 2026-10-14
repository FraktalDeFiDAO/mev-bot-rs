//! Debt tiers: census, bounded scheduler reads, refresh order and the batched store.

use alloy::primitives::{Address, B256, U256};
use mev_liq_state::*;

const CHAIN: u64 = 42161;
const SEC: u64 = 1_000_000_000;
const NOW: u64 = 10_000 * SEC;
const USD: u64 = 100_000_000; // Aave base currency: 8 decimals

fn pool() -> Address {
    Address::repeat_byte(0xaa)
}
fn who(n: u8) -> Address {
    Address::with_last_byte(n)
}
fn repo() -> (tempfile::TempDir, SnapshotRepository) {
    let d = tempfile::tempdir().unwrap();
    let r = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    (d, r)
}
fn tiers() -> DebtTiers {
    DebtTiers {
        min_debt_base: U256::from(10 * USD),
        hot_max_age_ns: 300 * SEC,
        cold_max_age_ns: 3600 * SEC,
    }
}
fn wad(milli: u64) -> U256 {
    U256::from(milli) * U256::from(1_000_000_000_000_000u64)
}

/// A provisional snapshot of `who(n)` at `block`, last checked `age_s` seconds before `NOW`.
fn snap(n: u8, block: u64, debt_usd: u64, hf_milli: u64, age_s: u64) -> BorrowerSnapshot {
    let at = NOW - age_s * SEC;
    BorrowerSnapshot {
        key: AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(n),
        },
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: B256::with_last_byte(block as u8),
            source: ChainSource::Provisional,
            observed_at_ns: at,
        },
        health_factor_wad: wad(hf_milli),
        total_debt_base: U256::from(debt_usd * USD),
        total_collateral_base: U256::from(2 * debt_usd * USD),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::from(8_000u64),
        ltv: U256::from(7_000u64),
        reserves: vec![],
        risk_metadata_version: 0,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(at),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

/// 1 hot fresh, 2 hot expired, 3 cold fresh, 4 cold expired, 5 hot eligible, 6 unknown,
/// 7 hot fresh but dirty after its check, 8 inactive (repaid).
fn population(r: &SnapshotRepository) {
    let all: Vec<Address> = (1..=8).map(who).collect();
    r.register_borrowers(CHAIN, pool(), &all, None, NOW - 7200 * SEC)
        .unwrap();
    let out = r
        .store_rechecks(&[
            snap(1, 100, 500, 1_500, 10),
            snap(2, 100, 50, 1_200, 400),
            snap(3, 100, 2, 3_000, 100),
            snap(4, 100, 1, 3_000, 4_000),
            snap(5, 100, 900, 950, 20),
            snap(7, 100, 80, 1_100, 30),
            snap(8, 100, 0, u64::MAX / 1_000_000_000_000_000, 30),
        ])
        .unwrap();
    assert_eq!(out.stored, 7);
    r.mark_dirty(
        DirtyTarget::Account(AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(7),
        }),
        DirtyReason::PriceMove,
        NOW - SEC,
    )
    .unwrap();
    // Repaid: reconciliation deactivates it and drops its marks.
    r.mark_dirty(
        DirtyTarget::Account(AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(8),
        }),
        DirtyReason::PriceMove,
        NOW - SEC,
    )
    .unwrap();
    let at = ChainPoint {
        block_number: 101,
        block_hash: B256::with_last_byte(101),
    };
    assert_eq!(
        r.reconcile_borrower(CHAIN, pool(), who(8), &[], false, at)
            .unwrap(),
        ReconcileOutcome::Deactivated
    );
}

#[test]
fn census_splits_active_borrowers_into_tiers() {
    let (_d, r) = repo();
    population(&r);
    let c = r.pool_census(CHAIN, pool(), &tiers(), NOW).unwrap();
    assert_eq!(
        c,
        PoolCensus {
            active: 7,
            inactive: 1,
            unknown: 1,
            hot: 4,
            hot_fresh: 3,
            cold: 2,
            cold_fresh: 1,
            dirty: 2, // 6 (discovered, no snapshot yet) and 7 (price move)
            eligible: 1,
        }
    );
}

#[test]
fn deactivation_clears_dirty_marks() {
    let (_d, r) = repo();
    population(&r);
    assert!(
        r.dirty_entries()
            .unwrap()
            .iter()
            .all(|e| e.key.borrower != who(8))
    );
}

#[test]
fn scheduler_reads_hot_and_dirty_accounts_only() {
    let (_d, r) = repo();
    population(&r);
    let got: Vec<Address> = r
        .scheduled_snapshots(CHAIN, pool(), tiers().min_debt_base)
        .unwrap()
        .iter()
        .map(|s| s.key.borrower)
        .collect();
    assert_eq!(got, vec![who(1), who(2), who(5), who(7)]);
    let stored = r
        .fetch_by_key(&AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(5),
        })
        .unwrap()
        .unwrap();
    let read = r
        .scheduled_snapshots(CHAIN, pool(), tiers().min_debt_base)
        .unwrap()
        .into_iter()
        .find(|s| s.key.borrower == who(5))
        .unwrap();
    assert_eq!(
        read, stored,
        "same row as fetch_by_key (no dirty reasons here)"
    );
    assert_eq!(
        r.unknown_borrowers(CHAIN, pool(), 10).unwrap(),
        (vec![who(6)], 1)
    );
    assert_eq!(
        r.unknown_borrowers(CHAIN, pool(), 0).unwrap(),
        (vec![], 1),
        "the total is counted even when nothing is returned"
    );
}

#[test]
fn refresh_order_is_dirty_then_hot_expired_then_unknown_then_cold_expired() {
    let (_d, r) = repo();
    population(&r);
    let all = r
        .refresh_candidates(CHAIN, pool(), &tiers(), NOW, 100)
        .unwrap();
    assert_eq!(all, vec![who(7), who(2), who(6), who(4)]);
    assert_eq!(
        r.refresh_candidates(CHAIN, pool(), &tiers(), NOW, 2)
            .unwrap(),
        vec![who(7), who(2)],
        "the limit cuts the least urgent stages"
    );
    assert!(
        r.refresh_candidates(CHAIN, pool(), &tiers(), NOW, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn store_rechecks_refreshes_and_skips_older_blocks() {
    let (_d, r) = repo();
    population(&r);
    let out = r
        .store_rechecks(&[snap(2, 120, 50, 1_200, 0), snap(1, 90, 500, 1_500, 0)])
        .unwrap();
    assert_eq!(
        out,
        StoreOutcome {
            stored: 1,
            superseded: 1
        }
    );
    let two = r
        .fetch_by_key(&AccountKey {
            chain_id: CHAIN,
            pool: pool(),
            borrower: who(2),
        })
        .unwrap()
        .unwrap();
    assert_eq!((two.revision, two.chain.block_number), (2, 120));
    // 2 is fresh again, 7's mark is cleared by a newer check.
    let next = r
        .refresh_candidates(CHAIN, pool(), &tiers(), NOW, 100)
        .unwrap();
    assert!(!next.contains(&who(2)));
    r.store_rechecks(&[snap(7, 121, 80, 1_100, 0)]).unwrap();
    assert_eq!(
        r.refresh_candidates(CHAIN, pool(), &tiers(), NOW, 100)
            .unwrap(),
        vec![who(6), who(4)]
    );
}

#[test]
fn lowest_health_lists_hot_accounts_by_health_factor() {
    let (_d, r) = repo();
    population(&r);
    let low = r
        .lowest_health(CHAIN, pool(), tiers().min_debt_base, 3)
        .unwrap();
    let order: Vec<Address> = low.iter().map(|l| l.borrower).collect();
    assert_eq!(order, vec![who(5), who(7), who(2)]);
    assert_eq!(low[0].health_factor_wad, wad(950));
    assert_eq!(low[0].total_debt_base, U256::from(900 * USD));
}

#[test]
fn migration_5_rolls_back_and_reapplies() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.db");
    let r = SnapshotRepository::open(&path).unwrap();
    assert_eq!(r.schema_version().unwrap(), 5);
    r.rollback_to(4).unwrap();
    assert_eq!(r.schema_version().unwrap(), 4);
    drop(r);
    let r = SnapshotRepository::open(&path).unwrap();
    assert_eq!(r.schema_version().unwrap(), SCHEMA_VERSION);
}
