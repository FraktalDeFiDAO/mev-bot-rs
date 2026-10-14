//! Reads scheduler inputs from the durable `mev-liq-state` repository (public API only).

use std::collections::BTreeSet;

use alloy::primitives::{Address, U256};
use mev_liq_state::{AccountKey, BorrowerSnapshot, DirtyEntry, RepoError, SnapshotRepository};

use crate::types::{AccountState, PoolState};

/// Owned inputs for one pool; `ScheduleInput` borrows from it.
#[derive(Debug, Clone)]
pub struct LoadedInputs {
    pub accounts: Vec<AccountState>,
    pub dirty: Vec<DirtyEntry>,
    pub pools: Vec<PoolState>,
    /// Active borrowers with no snapshot yet that `load_inputs_bounded` left out of this pass.
    pub deferred_unknown: u64,
}

/// Bounds of `load_inputs_bounded`: what keeps one pass small when the indexed population is
/// large (hundreds of thousands of borrowers, most of them dust or repaid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadBounds {
    /// Snapshot debt (protocol base units) below which a valid account is dust: it is left out
    /// unless it carries a dirty mark (the refresh task keeps it on a longer age).
    pub min_debt_base: U256,
    /// Accounts without any snapshot passed per pass (oldest discovery first); the rest are
    /// counted in `deferred_unknown`.
    pub max_unknown: usize,
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
        deferred_unknown: 0,
    })
}

/// `load_inputs` for a large population: the hot and dirty accounts with their snapshots (one
/// statement), at most `max_unknown` never-read borrowers, and only the dirty entries and
/// unreconciled flags of the accounts loaded. Dust accounts without a dirty mark are left out.
pub fn load_inputs_bounded(
    repo: &SnapshotRepository,
    chain_id: u64,
    pool: Address,
    scan_name: &str,
    b: &LoadBounds,
) -> Result<LoadedInputs, RepoError> {
    let snaps = repo.scheduled_snapshots(chain_id, pool, b.min_debt_base)?;
    let (unknown, unknown_total) = repo.unknown_borrowers(chain_id, pool, b.max_unknown)?;
    let mut accounts: Vec<AccountState> = Vec::with_capacity(snaps.len() + unknown.len());
    for s in snaps {
        let mut a = AccountState::new(s.key, None);
        a.cohort = cohort_of(&s);
        a.snapshot = Some(s);
        accounts.push(a);
    }
    for borrower in &unknown {
        accounts.push(AccountState::new(
            AccountKey {
                chain_id,
                pool,
                borrower: *borrower,
            },
            None,
        ));
    }
    accounts.sort_by_key(|a| a.key.borrower);
    let keys: BTreeSet<Address> = accounts.iter().map(|a| a.key.borrower).collect();
    for u in repo.list_unreconciled(chain_id, pool, i32::MAX as usize)? {
        if let Ok(i) = accounts.binary_search_by(|a| a.key.borrower.cmp(&u)) {
            accounts[i].unreconciled = true;
        }
    }
    let dirty =
        repo.dirty_entries_where(Some((chain_id, pool)), &|k| keys.contains(&k.borrower))?;
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
        deferred_unknown: unknown_total.saturating_sub(unknown.len() as u64),
    })
}
