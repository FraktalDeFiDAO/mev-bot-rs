//! Debt tiers and bounded reads for a large borrower population (Aave v3 on Arbitrum indexes
//! ~188k borrowers, most of them repaid or dust).
//!
//! An active borrower is *unknown* (no snapshot yet), *hot* (snapshot debt at or above
//! `min_debt_base`, or an invalidated snapshot) or *cold* (snapshot debt below the cutoff: dust
//! that cannot be liquidated at a profit). Hot accounts carry the max-age coverage guarantee;
//! cold accounts are refreshed on a longer age and whenever an event marks them dirty. Every read
//! here is one bounded statement, so memory does not grow with the indexed population.

use std::collections::HashSet;

use alloy::primitives::{Address, U256};
use rusqlite::{TransactionBehavior, params};

use crate::domain::{AccountKey, BorrowerSnapshot, WAD};
use crate::repo::{
    COLS, RepoError, SnapshotRepository, addr_of, i64_of, load, row_snapshot, u64_of, u256_of,
    upsert_cas_in,
};

pub(crate) const MIGRATION_5_UP: &str =
    "CREATE INDEX snapshots_by_check ON snapshots(chain_id, pool, last_check_ns);";
pub(crate) const MIGRATION_5_DOWN: &str = "DROP INDEX snapshots_by_check;";

/// Where the hot/cold line is and how old each tier may get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebtTiers {
    /// Snapshot debt in protocol base units (Aave: USD with 8 decimals) at or above which an
    /// account is hot.
    pub min_debt_base: U256,
    pub hot_max_age_ns: u64,
    pub cold_max_age_ns: u64,
}

/// Counts over the active borrowers of one pool. "Fresh" uses the same rule as `list_expired`:
/// an age equal to the maximum is still fresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolCensus {
    pub active: u64,
    pub inactive: u64,
    pub unknown: u64,
    pub hot: u64,
    pub hot_fresh: u64,
    pub cold: u64,
    pub cold_fresh: u64,
    /// Active accounts carrying at least one dirty mark.
    pub dirty: u64,
    /// Valid hot snapshots with `HF < 1` (liquidatable at their snapshot block).
    pub eligible: u64,
}

/// One of the lowest-health hot accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowHealth {
    pub borrower: Address,
    pub health_factor_wad: U256,
    pub total_debt_base: U256,
    pub last_check_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreOutcome {
    pub stored: usize,
    /// Refused because the stored snapshot is from a newer block (or the same block, canonical).
    pub superseded: usize,
}

const ACTIVE: &str =
    "EXISTS(SELECT 1 FROM borrowers b WHERE b.chain_id=s.chain_id AND b.pool=s.pool
                       AND b.borrower=s.borrower AND b.active=1)";

fn blob(v: U256) -> [u8; 32] {
    v.to_be_bytes::<32>()
}

impl SnapshotRepository {
    /// Census of `(chain_id, pool)` at `now_ns`.
    pub fn pool_census(
        &self,
        chain_id: u64,
        pool: Address,
        tiers: &DebtTiers,
        now_ns: u64,
    ) -> Result<PoolCensus, RepoError> {
        let c = self.conn();
        let cid = i64_of(chain_id, "chain_id")?;
        let now = i64_of(now_ns, "now_ns")?;
        let hot_age = i64::try_from(tiers.hot_max_age_ns).unwrap_or(i64::MAX);
        let cold_age = i64::try_from(tiers.cold_max_age_ns).unwrap_or(i64::MAX);
        let (min, wad, zero) = (blob(tiers.min_debt_base), blob(WAD), blob(U256::ZERO));
        let row = c.query_row(
            "SELECT COUNT(*),
               COALESCE(SUM(s.borrower IS NULL), 0),
               COALESCE(SUM(s.borrower IS NOT NULL AND (s.debt >= ?3 OR s.status=1)), 0),
               COALESCE(SUM(s.borrower IS NOT NULL AND s.debt >= ?3 AND s.status=0
                 AND s.last_check_ns IS NOT NULL AND ?4 - s.last_check_ns <= ?5), 0),
               COALESCE(SUM(s.borrower IS NOT NULL AND s.debt < ?3 AND s.status=0), 0),
               COALESCE(SUM(s.borrower IS NOT NULL AND s.debt < ?3 AND s.status=0
                 AND s.last_check_ns IS NOT NULL AND ?4 - s.last_check_ns <= ?6), 0),
               COALESCE(SUM(s.borrower IS NOT NULL AND s.status=0 AND s.debt >= ?3
                 AND s.debt > ?8 AND s.hf < ?7), 0)
             FROM borrowers b LEFT JOIN snapshots s
               ON s.chain_id=b.chain_id AND s.pool=b.pool AND s.borrower=b.borrower
             WHERE b.chain_id=?1 AND b.pool=?2 AND b.active=1",
            params![
                cid,
                pool.as_slice(),
                min.as_slice(),
                now,
                hot_age,
                cold_age,
                wad.as_slice(),
                zero.as_slice()
            ],
            |r| {
                Ok([
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ])
            },
        )?;
        let inactive: i64 = c.query_row(
            "SELECT COUNT(*) FROM borrowers WHERE chain_id=?1 AND pool=?2 AND active=0",
            params![cid, pool.as_slice()],
            |r| r.get(0),
        )?;
        let dirty: i64 = c.query_row(
            "SELECT COUNT(DISTINCT d.borrower) FROM dirty d JOIN borrowers b
               ON b.chain_id=d.chain_id AND b.pool=d.pool AND b.borrower=d.borrower
             WHERE d.chain_id=?1 AND d.pool=?2 AND b.active=1",
            params![cid, pool.as_slice()],
            |r| r.get(0),
        )?;
        let n = |v: i64, w| u64_of(v, w);
        Ok(PoolCensus {
            active: n(row[0], "active")?,
            inactive: n(inactive, "inactive")?,
            unknown: n(row[1], "unknown")?,
            hot: n(row[2], "hot")?,
            hot_fresh: n(row[3], "hot_fresh")?,
            cold: n(row[4], "cold")?,
            cold_fresh: n(row[5], "cold_fresh")?,
            dirty: n(dirty, "dirty")?,
            eligible: n(row[6], "eligible")?,
        })
    }

    /// Snapshots of the active accounts the scheduler must cover: hot ones, invalidated ones and
    /// cold ones carrying a dirty mark. `dirty_reasons` is left empty (the scheduler reads the
    /// dirty list separately); everything else is the stored row.
    pub fn scheduled_snapshots(
        &self,
        chain_id: u64,
        pool: Address,
        min_debt_base: U256,
    ) -> Result<Vec<BorrowerSnapshot>, RepoError> {
        let c = self.conn();
        let sql = format!(
            "SELECT {COLS}, borrower FROM snapshots s
             WHERE chain_id=?1 AND pool=?2 AND {ACTIVE}
               AND (debt >= ?3 OR status=1 OR EXISTS(SELECT 1 FROM dirty d
                    WHERE d.chain_id=s.chain_id AND d.pool=s.pool AND d.borrower=s.borrower))
             ORDER BY borrower"
        );
        let mut st = c.prepare(&sql)?;
        let mut rows = st.query(params![
            i64_of(chain_id, "chain_id")?,
            pool.as_slice(),
            blob(min_debt_base).as_slice()
        ])?;
        let mut out = vec![];
        while let Some(r) = rows.next()? {
            let key = AccountKey {
                chain_id,
                pool,
                borrower: addr_of(&r.get::<_, Vec<u8>>(17)?)?,
            };
            out.push(row_snapshot(key, r)?);
        }
        Ok(out)
    }

    /// Up to `limit` active borrowers without any snapshot (oldest discovery first), and how many
    /// there are in total.
    pub fn unknown_borrowers(
        &self,
        chain_id: u64,
        pool: Address,
        limit: usize,
    ) -> Result<(Vec<Address>, u64), RepoError> {
        let c = self.conn();
        let cid = i64_of(chain_id, "chain_id")?;
        let lim = i64::try_from(limit).unwrap_or(i64::MAX);
        const NO_SNAPSHOT: &str = "NOT EXISTS(SELECT 1 FROM snapshots s WHERE s.chain_id=b.chain_id
                                   AND s.pool=b.pool AND s.borrower=b.borrower)";
        let mut st = c.prepare(&format!(
            "SELECT borrower FROM borrowers b WHERE chain_id=?1 AND pool=?2 AND active=1
               AND {NO_SNAPSHOT} ORDER BY discovered_block, borrower LIMIT ?3"
        ))?;
        let rows: Vec<Vec<u8>> = st
            .query_map(params![cid, pool.as_slice(), lim], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let total: i64 = c.query_row(
            &format!(
                "SELECT COUNT(*) FROM borrowers b WHERE chain_id=?1 AND pool=?2 AND active=1
                   AND {NO_SNAPSHOT}"
            ),
            params![cid, pool.as_slice()],
            |r| r.get(0),
        )?;
        let list = rows.iter().map(|b| addr_of(b)).collect::<Result<_, _>>()?;
        Ok((list, u64_of(total, "unknown")?))
    }

    /// At most `limit` active borrowers to recheck now, most urgent first: dirty accounts that
    /// have a snapshot (oldest mark first); invalidated, never-checked and expired hot snapshots
    /// (oldest check first); borrowers with no snapshot yet (oldest discovery first); expired cold
    /// snapshots (oldest check first). No account appears twice.
    pub fn refresh_candidates(
        &self,
        chain_id: u64,
        pool: Address,
        tiers: &DebtTiers,
        now_ns: u64,
        limit: usize,
    ) -> Result<Vec<Address>, RepoError> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let c = self.conn();
        let cid = i64_of(chain_id, "chain_id")?;
        let now = i64_of(now_ns, "now_ns")?;
        let hot_age = i64::try_from(tiers.hot_max_age_ns).unwrap_or(i64::MAX);
        let cold_age = i64::try_from(tiers.cold_max_age_ns).unwrap_or(i64::MAX);
        let min = blob(tiers.min_debt_base);
        let (pb, mb): (&[u8], &[u8]) = (pool.as_slice(), &min);
        let lim = i64::try_from(limit).unwrap_or(i64::MAX);
        let stages: [(String, Vec<&dyn rusqlite::ToSql>); 4] = [
            (
                "SELECT d.borrower FROM dirty d WHERE d.chain_id=?1 AND d.pool=?2
                   AND EXISTS(SELECT 1 FROM snapshots s WHERE s.chain_id=d.chain_id
                              AND s.pool=d.pool AND s.borrower=d.borrower AND "
                    .to_owned()
                    + ACTIVE
                    + ") GROUP BY d.borrower ORDER BY MIN(d.marked_at_ns), d.borrower LIMIT ?3",
                vec![&cid, &pb, &lim],
            ),
            (
                format!(
                    "SELECT borrower FROM snapshots s WHERE chain_id=?1 AND pool=?2 AND {ACTIVE}
                       AND (status=1 OR last_check_ns IS NULL
                            OR (debt >= ?4 AND ?5 - last_check_ns > ?6))
                     ORDER BY last_check_ns IS NOT NULL, last_check_ns, borrower LIMIT ?3"
                ),
                vec![&cid, &pb, &lim, &mb, &now, &hot_age],
            ),
            (
                "SELECT borrower FROM borrowers b WHERE chain_id=?1 AND pool=?2 AND active=1
                   AND NOT EXISTS(SELECT 1 FROM snapshots s WHERE s.chain_id=b.chain_id
                                  AND s.pool=b.pool AND s.borrower=b.borrower)
                 ORDER BY discovered_block, borrower LIMIT ?3"
                    .to_owned(),
                vec![&cid, &pb, &lim],
            ),
            (
                format!(
                    "SELECT borrower FROM snapshots s WHERE chain_id=?1 AND pool=?2 AND {ACTIVE}
                       AND status=0 AND debt < ?4 AND last_check_ns IS NOT NULL
                       AND ?5 - last_check_ns > ?6
                     ORDER BY last_check_ns, borrower LIMIT ?3"
                ),
                vec![&cid, &pb, &lim, &mb, &now, &cold_age],
            ),
        ];
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (sql, args) in stages {
            let mut st = c.prepare(&sql)?;
            let rows: Vec<Vec<u8>> = st
                .query_map(args.as_slice(), |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for b in rows {
                let a = addr_of(&b)?;
                if seen.insert(a) {
                    out.push(a);
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Store freshly read snapshots in one transaction. Each goes in at the stored revision
    /// (compare-and-set inside the transaction); one older than the stored block is counted as
    /// superseded and skipped. Clears dirty marks made at or before each snapshot's observation.
    pub fn store_rechecks(&self, snaps: &[BorrowerSnapshot]) -> Result<StoreOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut out = StoreOutcome::default();
        for s in snaps {
            let cur = load(&tx, &s.key)?.map_or(0, |e| e.revision);
            match upsert_cas_in(&tx, s, cur) {
                Ok(_) => out.stored += 1,
                Err(RepoError::OutOfOrder { .. }) => out.superseded += 1,
                Err(e) => return Err(e),
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// The `n` valid hot snapshots with the lowest health factor (ties by address).
    pub fn lowest_health(
        &self,
        chain_id: u64,
        pool: Address,
        min_debt_base: U256,
        n: usize,
    ) -> Result<Vec<LowHealth>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(&format!(
            "SELECT borrower, hf, debt, last_check_ns FROM snapshots s
             WHERE chain_id=?1 AND pool=?2 AND status=0 AND debt >= ?3 AND debt > ?4 AND {ACTIVE}
             ORDER BY hf, borrower LIMIT ?5"
        ))?;
        let rows = st.query_map(
            params![
                i64_of(chain_id, "chain_id")?,
                pool.as_slice(),
                blob(min_debt_base).as_slice(),
                blob(U256::ZERO).as_slice(),
                i64::try_from(n).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )?;
        let mut out = vec![];
        for row in rows {
            let (b, hf, debt, last) = row?;
            out.push(LowHealth {
                borrower: addr_of(&b)?,
                health_factor_wad: u256_of(&hf)?,
                total_debt_base: u256_of(&debt)?,
                last_check_ns: last.map(|v| u64_of(v, "last_check_ns")).transpose()?,
            });
        }
        Ok(out)
    }
}
