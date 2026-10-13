#![allow(dead_code)]

use alloy::primitives::{Address, B256, U256};
use mev_liq_sched::*;
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainPoint, ChainRef, ChainSource, DirtyEntry, DirtyReason,
    SnapshotStatus, WAD,
};

pub const SEC: u64 = 1_000_000_000;
pub const NOW: u64 = 1_000_000 * SEC;
pub const POOL_BYTE: u8 = 0xaa;

pub fn pool() -> Address {
    Address::repeat_byte(POOL_BYTE)
}

pub fn key(n: u32) -> AccountKey {
    let mut b = [0u8; 20];
    b[16..].copy_from_slice(&n.to_be_bytes());
    AccountKey {
        chain_id: 42161,
        pool: pool(),
        borrower: Address::from(b),
    }
}

/// Valid canonical snapshot at `block`, last checked `age_s` seconds before `NOW`.
pub fn snap(n: u32, hf: U256, debt: u64, block: u64, age_s: u64) -> BorrowerSnapshot {
    let t = NOW - age_s * SEC;
    BorrowerSnapshot {
        key: key(n),
        revision: 1,
        chain: ChainRef {
            block_number: block,
            block_hash: B256::repeat_byte((block % 250) as u8 + 1),
            source: ChainSource::Canonical,
            observed_at_ns: t,
        },
        health_factor_wad: hf,
        total_debt_base: U256::from(debt),
        total_collateral_base: U256::from(1_000u64),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::from(8000u16),
        ltv: U256::from(7500u16),
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(t),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

/// HF given in thousandths of 1.0 (1010 = 1.010).
pub fn hf(milli: u64) -> U256 {
    WAD / U256::from(1000u64) * U256::from(milli)
}

pub fn healthy(n: u32, age_s: u64) -> AccountState {
    AccountState::new(key(n), Some(snap(n, hf(3000), 100, 100, age_s)))
}

pub fn point(block: u64) -> ChainPoint {
    ChainPoint {
        block_number: block,
        block_hash: B256::repeat_byte(9),
    }
}

pub fn dirty(n: u32, reasons: &[DirtyReason], block: u64, at_ns: u64) -> DirtyEntry {
    let most = *reasons.iter().max_by_key(|r| r.urgency()).unwrap();
    DirtyEntry {
        key: key(n),
        reasons: reasons.to_vec(),
        most_urgent: most,
        latest_chain: Some(point(block)),
        marked_at_ns: at_ns,
    }
}

pub fn run(accounts: &[AccountState], dirty: &[DirtyEntry], cfg: &PolicyConfig) -> ScheduleOutput {
    run_at(NOW, accounts, dirty, &[], &[], cfg)
}

pub fn run_at(
    now: u64,
    accounts: &[AccountState],
    dirty: &[DirtyEntry],
    pools: &[PoolState],
    in_flight: &[InFlight],
    cfg: &PolicyConfig,
) -> ScheduleOutput {
    schedule(
        &ScheduleInput {
            now_ns: now,
            accounts,
            dirty,
            urgent_events: &[],
            pools,
            in_flight,
            optional_decisions: &[],
        },
        cfg,
    )
    .unwrap()
}

pub fn find(o: &ScheduleOutput, n: u32) -> Option<&RecheckJob> {
    o.jobs
        .iter()
        .chain(o.backlog.iter())
        .find(|j| j.key == key(n))
}
