//! Reads scheduler inputs from the durable `mev-liq-state` repository (public API only).

use alloy::primitives::Address;
use mev_liq_state::{BorrowerSnapshot, DirtyEntry, RepoError, SnapshotRepository};

use crate::types::{AccountState, PoolState};

/// Owned inputs for one pool; `ScheduleInput` borrows from it.
#[derive(Debug, Clone)]
pub struct LoadedInputs {
    pub accounts: Vec<AccountState>,
    pub dirty: Vec<DirtyEntry>,
    pub pools: Vec<PoolState>,
}

/// Fairness cohort: the largest-debt reserve when per-reserve balances are known (tag 1),
/// otherwise a coarse debt-size bucket (tag 2), otherwise 0 (no snapshot yet).
pub fn cohort_of(s: &BorrowerSnapshot) -> u64 {
    if let Some(r) = s
        .reserves
        .iter()
        .filter(|r| !r.debt_raw.is_zero())
        .max_by(|a, b| a.debt_raw.cmp(&b.debt_raw).then(b.asset.cmp(&a.asset)))
    {
        let a = r.asset.as_slice();
        return (1u64 << 32) | u64::from(u32::from_be_bytes([a[0], a[1], a[2], a[3]]));
    }
    (2u64 << 32) | (s.total_debt_base.bit_len() / 4) as u64
}

/// Load every active borrower of `(chain_id, pool)` with its newest snapshot, the dirty list,
/// unreconciled flags, pool flags and the open coverage gaps of scan `scan_name`.
pub fn load_inputs(
    repo: &SnapshotRepository,
    chain_id: u64,
    pool: Address,
    scan_name: &str,
) -> Result<LoadedInputs, RepoError> {
    let unreconciled: std::collections::BTreeSet<Address> = repo
        .list_unreconciled(chain_id, pool, i32::MAX as usize)?
        .into_iter()
        .collect();
    let mut accounts = vec![];
    for b in repo.active_borrowers(chain_id, pool)? {
        let key = mev_liq_state::AccountKey {
            chain_id,
            pool,
            borrower: b,
        };
        let snapshot = repo.fetch_by_key(&key)?;
        let mut a = AccountState::new(key, snapshot);
        a.cohort = a.snapshot.as_ref().map_or(0, cohort_of);
        a.unreconciled = unreconciled.contains(&b);
        accounts.push(a);
    }
    let dirty = repo
        .dirty_entries()?
        .into_iter()
        .filter(|e| e.key.chain_id == chain_id && e.key.pool == pool)
        .collect();
    let pools = vec![PoolState {
        chain_id,
        pool,
        flags: repo.flags(chain_id, pool)?,
        gaps: repo.open_gaps(scan_name)?,
    }];
    Ok(LoadedInputs {
        accounts,
        dirty,
        pools,
    })
}
