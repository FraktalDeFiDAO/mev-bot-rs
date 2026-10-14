//! Durable snapshot repository on SQLite (WAL, `synchronous=FULL`, migrations table with a
//! rollback path). One row per account holds the latest snapshot; `revision` is a per-account
//! monotonic counter changed only inside `BEGIN IMMEDIATE` transactions.
//!
//! Storage: `U256` as fixed-width 32-byte big-endian BLOBs (memcmp order == numeric order),
//! addresses/hashes as raw BLOBs, block numbers and nanosecond times as INTEGER (must fit i64).
//!
//! Provisional -> canonical promotion happens only through `checkpoint_canonical`, which records a
//! canonical block checkpoint. `upsert_cas` refuses a `Canonical` snapshot whose block was never
//! checkpointed, so a provisional read can never be relabelled canonical by a writer.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use alloy::primitives::{Address, B256, U256};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::domain::{
    AccountKey, BorrowerSnapshot, ChainPoint, ChainRef, ChainSource, DirtyReason, ReserveBalance,
    SnapshotStatus,
};

pub const SCHEMA_VERSION: i64 = 3;

struct Migration {
    version: i64,
    name: &'static str,
    up: &'static str,
    down: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "snapshots_dirty_checkpoints",
        up: "
CREATE TABLE snapshots(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  revision INTEGER NOT NULL,
  block_number INTEGER NOT NULL, block_hash BLOB NOT NULL, source INTEGER NOT NULL,
  observed_at_ns INTEGER NOT NULL,
  hf BLOB NOT NULL, debt BLOB NOT NULL, coll BLOB NOT NULL, avail BLOB NOT NULL,
  liq_threshold BLOB NOT NULL, ltv BLOB NOT NULL,
  reserves BLOB NOT NULL, risk_version INTEGER NOT NULL,
  last_check_ns INTEGER, status INTEGER NOT NULL,
  provenance TEXT NOT NULL, digest BLOB NOT NULL,
  PRIMARY KEY(chain_id, pool, borrower)
) WITHOUT ROWID;
CREATE INDEX snapshots_by_block ON snapshots(block_number);
CREATE TABLE dirty(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  reason INTEGER NOT NULL, marked_at_ns INTEGER NOT NULL,
  PRIMARY KEY(chain_id, pool, borrower, reason)
) WITHOUT ROWID;
CREATE TABLE checkpoints(block_hash BLOB PRIMARY KEY, block_number INTEGER NOT NULL) WITHOUT ROWID;
",
        down: "DROP TABLE checkpoints; DROP TABLE dirty; DROP TABLE snapshots;",
    },
    Migration {
        version: 2,
        name: "event_index_reverse_deps_gaps",
        up: crate::index::MIGRATION_2_UP,
        down: crate::index::MIGRATION_2_DOWN,
    },
    Migration {
        version: 3,
        name: "snapshot_history",
        up: crate::history::MIGRATION_3_UP,
        down: crate::history::MIGRATION_3_DOWN,
    },
];

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stale revision: expected {expected}, stored {actual}")]
    Stale { expected: u64, actual: u64 },
    #[error("out-of-order snapshot: block {new_block} older than stored block {stored_block}")]
    OutOfOrder { stored_block: u64, new_block: u64 },
    #[error("feature digest does not match snapshot contents")]
    DigestMismatch,
    #[error("canonical snapshot refused: block {0} was never checkpointed canonical")]
    NotCheckpointed(B256),
    #[error("value out of range for storage: {0}")]
    Overflow(&'static str),
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("database schema version {found} is newer than this build ({supported})")]
    SchemaTooNew { found: i64, supported: i64 },
}

/// What `mark_dirty` applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyTarget {
    Account(AccountKey),
    /// Every account in the pool whose reserve list contains `asset`, or whose reserve
    /// composition is unknown (empty list): unknown composition is treated as affected.
    Asset {
        chain_id: u64,
        pool: Address,
        asset: Address,
    },
}

pub struct SnapshotRepository {
    conn: Mutex<Connection>,
}

pub(crate) fn i64_of(v: u64, what: &'static str) -> Result<i64, RepoError> {
    i64::try_from(v).map_err(|_| RepoError::Overflow(what))
}
pub(crate) fn u64_of(v: i64, what: &'static str) -> Result<u64, RepoError> {
    u64::try_from(v).map_err(|_| RepoError::Corrupt(format!("negative {what}")))
}
pub(crate) fn u256_of(b: &[u8]) -> Result<U256, RepoError> {
    U256::try_from_be_slice(b)
        .filter(|_| b.len() == 32)
        .ok_or_else(|| RepoError::Corrupt(format!("u256 blob of {} bytes", b.len())))
}
pub(crate) fn b256_of(b: &[u8]) -> Result<B256, RepoError> {
    (b.len() == 32)
        .then(|| B256::from_slice(b))
        .ok_or_else(|| RepoError::Corrupt(format!("hash blob of {} bytes", b.len())))
}
pub(crate) fn addr_of(b: &[u8]) -> Result<Address, RepoError> {
    (b.len() == 20)
        .then(|| Address::from_slice(b))
        .ok_or_else(|| RepoError::Corrupt(format!("address blob of {} bytes", b.len())))
}

const RESERVE_LEN: usize = 84;

fn encode_reserves(rs: &[ReserveBalance]) -> Vec<u8> {
    let mut v = Vec::with_capacity(rs.len() * RESERVE_LEN);
    for r in rs {
        v.extend_from_slice(r.asset.as_slice());
        v.extend_from_slice(&r.collateral_raw.to_be_bytes::<32>());
        v.extend_from_slice(&r.debt_raw.to_be_bytes::<32>());
    }
    v
}
fn decode_reserves(b: &[u8]) -> Result<Vec<ReserveBalance>, RepoError> {
    if !b.len().is_multiple_of(RESERVE_LEN) {
        return Err(RepoError::Corrupt("reserves blob length".into()));
    }
    b.chunks(RESERVE_LEN)
        .map(|c| {
            Ok(ReserveBalance {
                asset: addr_of(&c[..20])?,
                collateral_raw: u256_of(&c[20..52])?,
                debt_raw: u256_of(&c[52..84])?,
            })
        })
        .collect()
}

const COLS: &str = "revision, block_number, block_hash, source, observed_at_ns, hf, debt, coll, \
    avail, liq_threshold, ltv, reserves, risk_version, last_check_ns, status, provenance, digest";

fn row_snapshot(key: AccountKey, r: &rusqlite::Row<'_>) -> Result<BorrowerSnapshot, RepoError> {
    let blob = |i: usize| r.get::<_, Vec<u8>>(i);
    let source = ChainSource::from_code(r.get(3)?)
        .ok_or_else(|| RepoError::Corrupt("source code".into()))?;
    let status = match r.get::<_, i64>(14)? {
        0 => SnapshotStatus::Valid,
        1 => SnapshotStatus::Invalidated,
        s => return Err(RepoError::Corrupt(format!("status {s}"))),
    };
    let last: Option<i64> = r.get(13)?;
    Ok(BorrowerSnapshot {
        key,
        revision: u64_of(r.get(0)?, "revision")?,
        chain: ChainRef {
            block_number: u64_of(r.get(1)?, "block_number")?,
            block_hash: b256_of(&blob(2)?)?,
            source,
            observed_at_ns: u64_of(r.get(4)?, "observed_at_ns")?,
        },
        health_factor_wad: u256_of(&blob(5)?)?,
        total_debt_base: u256_of(&blob(6)?)?,
        total_collateral_base: u256_of(&blob(7)?)?,
        available_borrows_base: u256_of(&blob(8)?)?,
        liquidation_threshold: u256_of(&blob(9)?)?,
        ltv: u256_of(&blob(10)?)?,
        reserves: decode_reserves(&blob(11)?)?,
        risk_metadata_version: u64_of(r.get(12)?, "risk_version")?,
        dirty_reasons: vec![],
        last_successful_check_ns: last.map(|v| u64_of(v, "last_check_ns")).transpose()?,
        status,
        provenance: r.get(15)?,
        feature_digest: b256_of(&blob(16)?)?,
    })
}

fn load(tx: &Connection, key: &AccountKey) -> Result<Option<BorrowerSnapshot>, RepoError> {
    let sql = format!("SELECT {COLS} FROM snapshots WHERE chain_id=?1 AND pool=?2 AND borrower=?3");
    let found = tx
        .query_row(
            &sql,
            params![
                i64_of(key.chain_id, "chain_id")?,
                key.pool.as_slice(),
                key.borrower.as_slice()
            ],
            |r| Ok(row_snapshot(*key, r)),
        )
        .optional()?;
    let Some(res) = found else { return Ok(None) };
    let mut s = res?;
    let mut st = tx.prepare(
        "SELECT reason FROM dirty WHERE chain_id=?1 AND pool=?2 AND borrower=?3 ORDER BY reason",
    )?;
    let reasons = st.query_map(
        params![
            i64_of(key.chain_id, "chain_id")?,
            key.pool.as_slice(),
            key.borrower.as_slice()
        ],
        |r| r.get::<_, i64>(0),
    )?;
    for code in reasons {
        s.dirty_reasons.push(
            DirtyReason::from_code(code?)
                .ok_or_else(|| RepoError::Corrupt("dirty reason".into()))?,
        );
    }
    Ok(Some(s))
}

pub(crate) fn load_all_keys(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<AccountKey>, RepoError> {
    let mut st = tx.prepare(sql)?;
    let rows = st.query_map(args, |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut out = vec![];
    for row in rows {
        let (c, p, b) = row?;
        out.push(AccountKey {
            chain_id: u64_of(c, "chain_id")?,
            pool: addr_of(&p)?,
            borrower: addr_of(&b)?,
        });
    }
    Ok(out)
}

fn mark(tx: &Connection, key: &AccountKey, reason: DirtyReason, at: u64) -> Result<(), RepoError> {
    mark_at(tx, key, reason, at, None)
}

/// Upsert one dirty mark. Repeated marks coalesce on `(account, reason)`: the mark time only moves
/// forward and the chain position is the highest block seen (a `None` never erases a known one).
pub(crate) fn mark_at(
    tx: &Connection,
    key: &AccountKey,
    reason: DirtyReason,
    at: u64,
    chain: Option<ChainPoint>,
) -> Result<(), RepoError> {
    let (num, hash) = match chain {
        Some(c) => (
            Some(i64_of(c.block_number, "block_number")?),
            Some(c.block_hash.to_vec()),
        ),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO dirty(chain_id,pool,borrower,reason,marked_at_ns,block_number,block_hash)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(chain_id,pool,borrower,reason) DO UPDATE SET
           marked_at_ns=MAX(marked_at_ns, excluded.marked_at_ns),
           block_hash=CASE WHEN excluded.block_number IS NOT NULL
                            AND (block_number IS NULL OR excluded.block_number >= block_number)
                           THEN excluded.block_hash ELSE block_hash END,
           block_number=CASE WHEN excluded.block_number IS NOT NULL
                              AND (block_number IS NULL OR excluded.block_number >= block_number)
                             THEN excluded.block_number ELSE block_number END",
        params![
            i64_of(key.chain_id, "chain_id")?,
            key.pool.as_slice(),
            key.borrower.as_slice(),
            reason.code(),
            i64_of(at, "marked_at_ns")?,
            num,
            hash
        ],
    )?;
    Ok(())
}

impl SnapshotRepository {
    /// Open (creating if needed) a durable file database and apply pending migrations.
    pub fn open(path: &Path) -> Result<Self, RepoError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=10000;",
        )?;
        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.migrate_up()?;
        Ok(repo)
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn migrate_up(&self) -> Result<(), RepoError> {
        let mut c = self.conn();
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
               version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL)",
        )?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cur: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if cur > SCHEMA_VERSION {
            return Err(RepoError::SchemaTooNew {
                found: cur,
                supported: SCHEMA_VERSION,
            });
        }
        for m in MIGRATIONS.iter().filter(|m| m.version > cur) {
            tx.execute_batch(m.up)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,name,applied_at) VALUES(?1,?2,strftime('%s','now'))",
                params![m.version, m.name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Roll the schema back to `target` (0 = empty), running down scripts newest-first.
    /// Destroys the data of the rolled-back tables. Reopening re-applies migrations.
    pub fn rollback_to(&self, target: i64) -> Result<(), RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for m in MIGRATIONS.iter().rev().filter(|m| m.version > target) {
            tx.execute_batch(m.down)?;
            tx.execute(
                "DELETE FROM schema_migrations WHERE version=?1",
                [m.version],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, RepoError> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn fetch_by_key(&self, key: &AccountKey) -> Result<Option<BorrowerSnapshot>, RepoError> {
        load(&self.conn(), key)
    }

    /// Compare-and-set. Succeeds only if the stored revision equals `expected_revision` (0 = no
    /// row yet); stores the snapshot at revision `expected_revision + 1` (the input's `revision`
    /// and `dirty_reasons` are ignored) and returns what was stored. Clears dirty marks made at or
    /// before the snapshot's `observed_at_ns`.
    pub fn upsert_cas(
        &self,
        snapshot: &BorrowerSnapshot,
        expected_revision: u64,
    ) -> Result<BorrowerSnapshot, RepoError> {
        if !snapshot.digest_matches() {
            return Err(RepoError::DigestMismatch);
        }
        let new_rev = expected_revision
            .checked_add(1)
            .filter(|r| i64::try_from(*r).is_ok())
            .ok_or(RepoError::Overflow("revision"))?;
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load(&tx, &snapshot.key)?;
        let actual = existing.as_ref().map_or(0, |e| e.revision);
        if actual != expected_revision {
            return Err(RepoError::Stale {
                expected: expected_revision,
                actual,
            });
        }
        if let Some(e) = &existing {
            let older = snapshot.chain.block_number < e.chain.block_number;
            let downgrade = snapshot.chain.block_number == e.chain.block_number
                && snapshot.chain.block_hash == e.chain.block_hash
                && e.chain.source == ChainSource::Canonical
                && snapshot.chain.source == ChainSource::Provisional;
            if older || downgrade {
                return Err(RepoError::OutOfOrder {
                    stored_block: e.chain.block_number,
                    new_block: snapshot.chain.block_number,
                });
            }
        }
        if snapshot.chain.source == ChainSource::Canonical {
            let ok: Option<i64> = tx
                .query_row(
                    "SELECT block_number FROM checkpoints WHERE block_hash=?1",
                    [snapshot.chain.block_hash.as_slice()],
                    |r| r.get(0),
                )
                .optional()?;
            if ok != Some(i64_of(snapshot.chain.block_number, "block_number")?) {
                return Err(RepoError::NotCheckpointed(snapshot.chain.block_hash));
            }
        }
        let k = &snapshot.key;
        let s = snapshot;
        tx.execute(
            &format!(
                "INSERT OR REPLACE INTO snapshots(chain_id,pool,borrower,{COLS})
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,0,?18,?19)"
            ),
            params![
                i64_of(k.chain_id, "chain_id")?,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                new_rev as i64,
                i64_of(s.chain.block_number, "block_number")?,
                s.chain.block_hash.as_slice(),
                s.chain.source.code(),
                i64_of(s.chain.observed_at_ns, "observed_at_ns")?,
                s.health_factor_wad.to_be_bytes::<32>().as_slice(),
                s.total_debt_base.to_be_bytes::<32>().as_slice(),
                s.total_collateral_base.to_be_bytes::<32>().as_slice(),
                s.available_borrows_base.to_be_bytes::<32>().as_slice(),
                s.liquidation_threshold.to_be_bytes::<32>().as_slice(),
                s.ltv.to_be_bytes::<32>().as_slice(),
                encode_reserves(&s.reserves),
                i64_of(s.risk_metadata_version, "risk_version")?,
                s.last_successful_check_ns
                    .map(|v| i64_of(v, "last_check_ns"))
                    .transpose()?,
                s.provenance,
                s.feature_digest.as_slice(),
            ],
        )?;
        tx.execute(
            "DELETE FROM dirty WHERE chain_id=?1 AND pool=?2 AND borrower=?3 AND marked_at_ns<=?4",
            params![
                i64_of(k.chain_id, "chain_id")?,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                i64_of(s.chain.observed_at_ns, "observed_at_ns")?
            ],
        )?;
        let stored = load(&tx, k)?.ok_or_else(|| RepoError::Corrupt("row vanished".into()))?;
        crate::history::record(&tx, &stored)?;
        tx.commit()?;
        Ok(stored)
    }

    /// Accounts whose last successful check is older than `max_age_ns` (strictly: age == max_age
    /// is still fresh), never checked, or invalidated by a reorg.
    pub fn list_expired(&self, now_ns: u64, max_age_ns: u64) -> Result<Vec<AccountKey>, RepoError> {
        let now = i64_of(now_ns, "now_ns")?;
        let max = i64::try_from(max_age_ns).unwrap_or(i64::MAX); // saturate: "never expires"
        load_all_keys(
            &self.conn(),
            "SELECT chain_id,pool,borrower FROM snapshots
             WHERE status=1 OR last_check_ns IS NULL
                OR (?1 > last_check_ns AND ?1 - last_check_ns > ?2)
             ORDER BY chain_id,pool,borrower",
            &[&now, &max],
        )
    }

    /// Accounts carrying at least one dirty mark (including accounts with no snapshot yet).
    pub fn list_dirty(&self) -> Result<Vec<AccountKey>, RepoError> {
        load_all_keys(
            &self.conn(),
            "SELECT DISTINCT chain_id,pool,borrower FROM dirty ORDER BY chain_id,pool,borrower",
            &[],
        )
    }

    /// Mark accounts for recheck; returns how many accounts were marked. Dirty marks live outside
    /// the revisioned snapshot so marking never makes an in-flight recheck stale.
    pub fn mark_dirty(
        &self,
        target: DirtyTarget,
        reason: DirtyReason,
        marked_at_ns: u64,
    ) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = match target {
            DirtyTarget::Account(k) => {
                mark(&tx, &k, reason, marked_at_ns)?;
                1
            }
            DirtyTarget::Asset {
                chain_id,
                pool,
                asset,
            } => {
                let mut hit = vec![];
                {
                    let mut st = tx.prepare(
                        "SELECT borrower, reserves FROM snapshots WHERE chain_id=?1 AND pool=?2",
                    )?;
                    let rows = st.query_map(
                        params![i64_of(chain_id, "chain_id")?, pool.as_slice()],
                        |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
                    )?;
                    for row in rows {
                        let (b, rs) = row?;
                        let rs = decode_reserves(&rs)?;
                        if rs.is_empty() || rs.iter().any(|r| r.asset == asset) {
                            hit.push(AccountKey {
                                chain_id,
                                pool,
                                borrower: addr_of(&b)?,
                            });
                        }
                    }
                }
                for k in &hit {
                    mark(&tx, k, reason, marked_at_ns)?;
                }
                hit.len()
            }
        };
        tx.commit()?;
        Ok(n)
    }

    /// Record `point` as canonical and promote the provisional, valid snapshots read at exactly
    /// that block (number and hash). Each promotion bumps the revision and re-seals the digest
    /// (the source is part of it). Returns the number promoted.
    pub fn checkpoint_canonical(&self, point: ChainPoint) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let num = i64_of(point.block_number, "block_number")?;
        tx.execute(
            "INSERT OR REPLACE INTO checkpoints(block_hash, block_number) VALUES(?1,?2)",
            params![point.block_hash.as_slice(), num],
        )?;
        let keys = load_all_keys(
            &tx,
            "SELECT chain_id,pool,borrower FROM snapshots
             WHERE block_hash=?1 AND block_number=?2 AND source=0 AND status=0",
            &[&point.block_hash.as_slice(), &num],
        )?;
        for k in &keys {
            let mut s = load(&tx, k)?.ok_or_else(|| RepoError::Corrupt("row vanished".into()))?;
            s.chain.source = ChainSource::Canonical;
            s = s.seal();
            tx.execute(
                "UPDATE snapshots SET source=1, revision=revision+1, digest=?4
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![
                    i64_of(k.chain_id, "chain_id")?,
                    k.pool.as_slice(),
                    k.borrower.as_slice(),
                    s.feature_digest.as_slice()
                ],
            )?;
            crate::history::record(&tx, &s)?;
        }
        tx.commit()?;
        Ok(keys.len())
    }

    /// Roll back to `ancestor`: every valid snapshot read after it, or at its height on a
    /// different hash, is on the dropped fork. Those rows (provisional or canonical) are marked
    /// `Invalidated`, get a bumped revision and a `Reorg` dirty mark. Rows at or below the
    /// ancestor survive, as do checkpoints at or below it. Returns the number invalidated.
    pub fn invalidate_from_reorg(
        &self,
        ancestor: ChainPoint,
        now_ns: u64,
    ) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx: Transaction<'_> = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = invalidate_in_tx(&tx, ancestor, now_ns)?;
        tx.commit()?;
        Ok(n)
    }
}

/// Body of `invalidate_from_reorg`, reusable inside a larger transaction.
pub(crate) fn invalidate_in_tx(
    tx: &Connection,
    ancestor: ChainPoint,
    now_ns: u64,
) -> Result<usize, RepoError> {
    {
        let num = i64_of(ancestor.block_number, "block_number")?;
        crate::history::prune_reorg(tx, num, ancestor.block_hash.as_slice())?;
        tx.execute("DELETE FROM checkpoints WHERE block_number>?1", [num])?;
        tx.execute(
            "DELETE FROM checkpoints WHERE block_number=?1 AND block_hash<>?2",
            params![num, ancestor.block_hash.as_slice()],
        )?;
        let keys = load_all_keys(
            tx,
            "SELECT chain_id,pool,borrower FROM snapshots
             WHERE status=0 AND (block_number>?1 OR (block_number=?1 AND block_hash<>?2))",
            &[&num, &ancestor.block_hash.as_slice()],
        )?;
        for k in &keys {
            tx.execute(
                "UPDATE snapshots SET status=1, revision=revision+1
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![
                    i64_of(k.chain_id, "chain_id")?,
                    k.pool.as_slice(),
                    k.borrower.as_slice()
                ],
            )?;
            mark(tx, k, DirtyReason::Reorg, now_ns)?;
        }
        Ok(keys.len())
    }
}
