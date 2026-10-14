//! Canonical snapshot history (migration 3, JEV-LIQ-007): the last few canonical
//! `(hf, debt, block, time)` points per account, used to derive `hf_trend` without any lookahead.
//!
//! Only canonical, valid snapshots are recorded: either written canonical through `upsert_cas`
//! (which already requires a checkpointed block) or promoted by `checkpoint_canonical`. A reorg
//! deletes every history row on the dropped fork, so a row here is always on the canonical chain.
//! Retention is bounded per account.

use alloy::primitives::{B256, U256};
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{AccountKey, BorrowerSnapshot, ChainSource, SnapshotStatus};
use crate::repo::{RepoError, SnapshotRepository, b256_of, i64_of, u64_of, u256_of};

/// History rows kept per account; older rows are pruned on every insert.
pub const HISTORY_RETENTION: usize = 32;

pub(crate) const MIGRATION_3_UP: &str = "
CREATE TABLE snapshot_history(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  block_number INTEGER NOT NULL, block_hash BLOB NOT NULL, observed_at_ns INTEGER NOT NULL,
  hf BLOB NOT NULL, debt BLOB NOT NULL,
  PRIMARY KEY(chain_id, pool, borrower, block_number)
) WITHOUT ROWID;
CREATE INDEX snapshot_history_by_block ON snapshot_history(block_number);
";

pub(crate) const MIGRATION_3_DOWN: &str = "DROP TABLE snapshot_history;";

/// One canonical observation of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPoint {
    pub block_number: u64,
    pub block_hash: B256,
    pub observed_at_ns: u64,
    pub health_factor_wad: U256,
    pub total_debt_base: U256,
}

/// Record a canonical, valid snapshot (no-op for anything else) and prune to the retention bound.
pub(crate) fn record(tx: &Connection, s: &BorrowerSnapshot) -> Result<(), RepoError> {
    if s.chain.source != ChainSource::Canonical || s.status != SnapshotStatus::Valid {
        return Ok(());
    }
    let k = &s.key;
    let chain = i64_of(k.chain_id, "chain_id")?;
    tx.execute(
        "INSERT OR REPLACE INTO snapshot_history
           (chain_id,pool,borrower,block_number,block_hash,observed_at_ns,hf,debt)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            chain,
            k.pool.as_slice(),
            k.borrower.as_slice(),
            i64_of(s.chain.block_number, "block_number")?,
            s.chain.block_hash.as_slice(),
            i64_of(s.chain.observed_at_ns, "observed_at_ns")?,
            s.health_factor_wad.to_be_bytes::<32>().as_slice(),
            s.total_debt_base.to_be_bytes::<32>().as_slice(),
        ],
    )?;
    // Keep the newest HISTORY_RETENTION rows: delete everything below the Nth-newest block.
    tx.execute(
        "DELETE FROM snapshot_history WHERE chain_id=?1 AND pool=?2 AND borrower=?3
           AND block_number < COALESCE((SELECT block_number FROM snapshot_history
             WHERE chain_id=?1 AND pool=?2 AND borrower=?3
             ORDER BY block_number DESC LIMIT 1 OFFSET ?4), 0)",
        params![
            chain,
            k.pool.as_slice(),
            k.borrower.as_slice(),
            (HISTORY_RETENTION - 1) as i64
        ],
    )?;
    Ok(())
}

/// Drop history on the dropped fork: above the ancestor, or at its height on another hash.
pub(crate) fn prune_reorg(
    tx: &Connection,
    ancestor_number: i64,
    ancestor_hash: &[u8],
) -> Result<(), RepoError> {
    tx.execute(
        "DELETE FROM snapshot_history
         WHERE block_number>?1 OR (block_number=?1 AND block_hash<>?2)",
        params![ancestor_number, ancestor_hash],
    )?;
    Ok(())
}

impl SnapshotRepository {
    /// The newest canonical history point of `key` with `block_number` strictly below
    /// `before_block`. The cutoff is enforced in SQL: a row at or after it can never be returned
    /// (leakage guard for as-of feature building).
    pub fn previous_canonical(
        &self,
        key: &AccountKey,
        before_block: u64,
    ) -> Result<Option<HistoryPoint>, RepoError> {
        let c = self.conn();
        let row = c
            .query_row(
                "SELECT block_number,block_hash,observed_at_ns,hf,debt FROM snapshot_history
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3 AND block_number<?4
                 ORDER BY block_number DESC LIMIT 1",
                params![
                    i64_of(key.chain_id, "chain_id")?,
                    key.pool.as_slice(),
                    key.borrower.as_slice(),
                    i64_of(before_block, "before_block")?
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Vec<u8>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(b, h, t, hf, debt)| {
            Ok(HistoryPoint {
                block_number: u64_of(b, "block_number")?,
                block_hash: b256_of(&h)?,
                observed_at_ns: u64_of(t, "observed_at_ns")?,
                health_factor_wad: u256_of(&hf)?,
                total_debt_base: u256_of(&debt)?,
            })
        })
        .transpose()
    }

    /// Number of history rows for `key` (retention checks).
    pub fn history_len(&self, key: &AccountKey) -> Result<usize, RepoError> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM snapshot_history WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            params![
                i64_of(key.chain_id, "chain_id")?,
                key.pool.as_slice(),
                key.borrower.as_slice()
            ],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}
