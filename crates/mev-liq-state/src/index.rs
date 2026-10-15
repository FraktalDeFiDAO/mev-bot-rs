//! Durable event-index state (JEV-LIQ-004): borrower registry, the `asset -> borrower` reverse
//! index, scan cursors with block-hash ancestry, explicit coverage gaps, pool flags (upgrade
//! re-verification), a bounded fan-out queue and oracle price baselines.
//!
//! Everything that must stay consistent is written by [`SnapshotRepository::apply_scan`] in ONE
//! SQLite transaction: the dirty marks and registry/dependency changes caused by a block range and
//! the cursor that says "this range is done". A crash can therefore never leave a cursor ahead of
//! the marks it implies (the old JSON cache could), nor marks without the cursor (harmless replay).
//!
//! Reorgs: [`SnapshotRepository::rewind_scan`] moves the cursor back to a common ancestor, drops
//! borrowers discovered only on the orphaned blocks, flags the borrowers those blocks touched
//! (dirty `Reorg`, dependency set stale) and invalidates snapshots read on the dropped fork.

use alloy::primitives::{Address, U256};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::domain::{AccountKey, ChainPoint, DirtyReason};
use crate::repo::{
    RepoError, SnapshotRepository, addr_of, b256_of, i64_of, invalidate_in_tx, mark_at, u64_of,
    u256_of,
};

/// Reverse-index dependency kinds (bit flags).
pub const KIND_COLLATERAL: u8 = 1;
pub const KIND_DEBT: u8 = 2;

/// Pool flag: the Pool implementation changed (or could not be proven unchanged); every cached
/// reserve/risk assumption must be re-verified before use.
pub const FLAG_FULL_REVERIFICATION: &str = "full_reverification";
/// Pool flag: the coalescing queue overflowed and every active borrower was marked instead.
pub const FLAG_BACKLOG_ESCALATED: &str = "backlog_escalated";

pub(crate) const MIGRATION_2_UP: &str = "
ALTER TABLE dirty ADD COLUMN block_number INTEGER;
ALTER TABLE dirty ADD COLUMN block_hash BLOB;
CREATE TABLE borrowers(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  active INTEGER NOT NULL,
  deps_reconciled INTEGER NOT NULL DEFAULT 0,
  deps_stale INTEGER NOT NULL DEFAULT 0,
  last_touch_block INTEGER NOT NULL DEFAULT 0,
  discovered_block INTEGER NOT NULL,
  discovered_hash BLOB,
  discovered_ns INTEGER NOT NULL,
  PRIMARY KEY(chain_id, pool, borrower)
) WITHOUT ROWID;
CREATE INDEX borrowers_by_discovery ON borrowers(chain_id, pool, discovered_block);
CREATE TABLE borrower_assets(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, asset BLOB NOT NULL, borrower BLOB NOT NULL,
  kind INTEGER NOT NULL,
  PRIMARY KEY(chain_id, pool, asset, borrower)
) WITHOUT ROWID;
CREATE INDEX borrower_assets_by_borrower ON borrower_assets(chain_id, pool, borrower);
CREATE TABLE scan_cursor(
  name TEXT PRIMARY KEY, next_block INTEGER NOT NULL, tip_number INTEGER, tip_hash BLOB,
  back_block INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE scan_blocks(
  name TEXT NOT NULL, block_number INTEGER NOT NULL, block_hash BLOB NOT NULL,
  PRIMARY KEY(name, block_number)
) WITHOUT ROWID;
CREATE TABLE scan_touched(
  name TEXT NOT NULL, block_number INTEGER NOT NULL, block_hash BLOB NOT NULL, borrower BLOB NOT NULL,
  PRIMARY KEY(name, block_number, block_hash, borrower)
) WITHOUT ROWID;
CREATE TABLE coverage_gaps(
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, direction INTEGER NOT NULL,
  from_block INTEGER NOT NULL, to_block INTEGER NOT NULL, reason TEXT NOT NULL,
  recorded_ns INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, resolved_ns INTEGER
);
CREATE INDEX coverage_gaps_open ON coverage_gaps(name, resolved_ns);
CREATE TABLE pool_flags(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, flag TEXT NOT NULL,
  block_number INTEGER, detail TEXT NOT NULL, set_ns INTEGER NOT NULL,
  PRIMARY KEY(chain_id, pool, flag)
) WITHOUT ROWID;
CREATE TABLE fanout_queue(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL, reason INTEGER NOT NULL,
  marked_ns INTEGER NOT NULL, block_number INTEGER, block_hash BLOB,
  UNIQUE(chain_id, pool, borrower, reason)
);
CREATE TABLE oracle_prices(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, asset BLOB NOT NULL,
  price BLOB NOT NULL, block_number INTEGER,
  PRIMARY KEY(chain_id, pool, asset)
) WITHOUT ROWID;
";

pub(crate) const MIGRATION_2_DOWN: &str = "
DROP TABLE oracle_prices; DROP TABLE fanout_queue; DROP TABLE pool_flags;
DROP TABLE coverage_gaps; DROP TABLE scan_touched; DROP TABLE scan_blocks;
DROP TABLE scan_cursor; DROP TABLE borrower_assets; DROP TABLE borrowers;
ALTER TABLE dirty DROP COLUMN block_hash;
ALTER TABLE dirty DROP COLUMN block_number;
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDir {
    Forward,
    Backward,
}

impl ScanDir {
    fn code(self) -> i64 {
        match self {
            Self::Forward => 0,
            Self::Backward => 1,
        }
    }
    fn from_code(c: i64) -> Result<Self, RepoError> {
        match c {
            0 => Ok(Self::Forward),
            1 => Ok(Self::Backward),
            _ => Err(RepoError::Corrupt(format!("scan direction {c}"))),
        }
    }
}

/// Scan progress. `next_block` is the first block not yet scanned going forward; history below
/// `back_block` is not scanned yet; `tip` is the last fully scanned block (for reorg detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCursor {
    pub next_block: u64,
    pub tip: Option<ChainPoint>,
    pub back_block: u64,
}

/// A block range whose logs were not obtained (or not fully processed). Open until resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageGap {
    pub id: i64,
    pub name: String,
    pub dir: ScanDir,
    pub from_block: u64,
    pub to_block: u64,
    pub reason: String,
    pub recorded_ns: u64,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGap {
    pub dir: ScanDir,
    pub from_block: u64,
    pub to_block: u64,
    pub reason: String,
}

/// One state change derived from a log (or from a coalesced set of logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Register the borrower if unknown (reactivate if inactive) and mark it `Discovered`.
    Discover { borrower: Address, at: ChainPoint },
    /// A live event touched the borrower. Registered borrowers get `reason` marked; unknown ones
    /// are ignored unless `register` (e.g. a `Borrow`), in which case they are registered and also
    /// marked `Discovered`. `stale_deps` flags the dependency set for reconciliation.
    Touch {
        borrower: Address,
        reason: DirtyReason,
        at: ChainPoint,
        register: Option<ChainPoint>,
        stale_deps: bool,
    },
    DepAdd {
        borrower: Address,
        asset: Address,
        kind: u8,
    },
    DepRemove {
        borrower: Address,
        asset: Address,
        kind: u8,
    },
    /// Every active holder of `asset` (reverse index; borrowers with unreconciled dependencies
    /// count as holders of everything) is marked, at most `fanout_limit` immediately, the rest
    /// queued durably.
    AssetDirty {
        asset: Address,
        reason: DirtyReason,
        at: Option<ChainPoint>,
    },
    /// Every active borrower (upgrade, overflow escalation).
    AllDirty {
        reason: DirtyReason,
        at: Option<ChainPoint>,
    },
    Flag {
        flag: String,
        at: Option<ChainPoint>,
        detail: String,
    },
}

/// Everything one scanned range (or gap retry) changes, applied atomically.
#[derive(Debug, Clone)]
pub struct ScanBatch {
    pub name: String,
    pub chain_id: u64,
    pub pool: Address,
    pub now_ns: u64,
    /// Immediate marks per asset/all-dirty effect; the remainder is queued.
    pub fanout_limit: usize,
    pub effects: Vec<Effect>,
    /// Forward: first unscanned block after this batch and the block that closes it.
    pub next_block: Option<u64>,
    pub tip: Option<ChainPoint>,
    /// Blocks seen carrying logs (ancestry for reorg rewinds).
    pub ancestry: Vec<ChainPoint>,
    /// Backward: new lower bound of unscanned history.
    pub back_block: Option<u64>,
    pub new_gaps: Vec<NewGap>,
    pub resolved_gaps: Vec<i64>,
    pub failed_gap_attempts: Vec<i64>,
}

impl ScanBatch {
    pub fn new(name: &str, chain_id: u64, pool: Address, now_ns: u64, fanout_limit: usize) -> Self {
        Self {
            name: name.to_string(),
            chain_id,
            pool,
            now_ns,
            fanout_limit,
            effects: vec![],
            next_block: None,
            tip: None,
            ancestry: vec![],
            back_block: None,
            new_gaps: vec![],
            resolved_gaps: vec![],
            failed_gap_attempts: vec![],
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub registered: usize,
    pub marked: usize,
    pub queued: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RewindOutcome {
    pub invalidated_snapshots: usize,
    pub removed_borrowers: usize,
    pub touched_marked: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorrowerRecord {
    pub active: bool,
    pub deps_reconciled: bool,
    pub deps_stale: bool,
    pub last_touch_block: u64,
    pub discovered_block: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolFlag {
    pub flag: String,
    pub block_number: Option<u64>,
    pub detail: String,
    pub set_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Unknown borrower: nothing to reconcile.
    Unknown,
    /// Dependencies replaced; borrower is active.
    Active,
    /// No debt on chain: borrower deactivated and its dependencies removed.
    Deactivated,
    /// A newer event touched the borrower after the block the read was made at: left stale.
    Superseded,
}

/// One entry of the prioritized dirty list task 005 consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyEntry {
    pub key: AccountKey,
    pub reasons: Vec<DirtyReason>,
    pub most_urgent: DirtyReason,
    pub latest_chain: Option<ChainPoint>,
    pub marked_at_ns: u64,
}

fn point_cols(p: Option<ChainPoint>) -> Result<(Option<i64>, Option<Vec<u8>>), RepoError> {
    Ok(match p {
        Some(c) => (
            Some(i64_of(c.block_number, "block_number")?),
            Some(c.block_hash.to_vec()),
        ),
        None => (None, None),
    })
}

fn key(chain_id: u64, pool: Address, borrower: Address) -> AccountKey {
    AccountKey {
        chain_id,
        pool,
        borrower,
    }
}

fn registered(
    c: &Connection,
    chain_id: u64,
    pool: Address,
    borrower: Address,
) -> Result<Option<BorrowerRecord>, RepoError> {
    c.query_row(
        "SELECT active,deps_reconciled,deps_stale,last_touch_block,discovered_block FROM borrowers
         WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
        params![
            i64_of(chain_id, "chain_id")?,
            pool.as_slice(),
            borrower.as_slice()
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        },
    )
    .optional()?
    .map(|(a, rc, st, lt, db)| {
        Ok(BorrowerRecord {
            active: a != 0,
            deps_reconciled: rc != 0,
            deps_stale: st != 0,
            last_touch_block: u64_of(lt, "last_touch_block")?,
            discovered_block: u64_of(db, "discovered_block")?,
        })
    })
    .transpose()
}

/// Register or reactivate. Returns true when the borrower was new.
fn register(
    c: &Connection,
    k: &AccountKey,
    at: Option<ChainPoint>,
    now_ns: u64,
) -> Result<bool, RepoError> {
    let (num, hash) = point_cols(at)?;
    let n = c.execute(
        "INSERT OR IGNORE INTO borrowers(chain_id,pool,borrower,active,discovered_block,discovered_hash,discovered_ns)
         VALUES(?1,?2,?3,1,?4,?5,?6)",
        params![
            i64_of(k.chain_id, "chain_id")?,
            k.pool.as_slice(),
            k.borrower.as_slice(),
            num.unwrap_or(0),
            hash,
            i64_of(now_ns, "now_ns")?
        ],
    )?;
    if n == 0 {
        // A returning borrower must have its dependency set re-established.
        c.execute(
            "UPDATE borrowers SET deps_reconciled=CASE WHEN active=0 THEN 0 ELSE deps_reconciled END,
               active=1 WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            params![
                i64_of(k.chain_id, "chain_id")?,
                k.pool.as_slice(),
                k.borrower.as_slice()
            ],
        )?;
    }
    Ok(n == 1)
}

/// Active holders of `asset`, riskiest first (lowest stored health factor, unknown last).
fn holders(
    c: &Connection,
    chain_id: u64,
    pool: Address,
    asset: Option<Address>,
) -> Result<Vec<Address>, RepoError> {
    let mut sql = String::from(
        "SELECT b.borrower FROM borrowers b
         LEFT JOIN snapshots s ON s.chain_id=b.chain_id AND s.pool=b.pool AND s.borrower=b.borrower
         WHERE b.chain_id=?1 AND b.pool=?2 AND b.active=1",
    );
    if asset.is_some() {
        sql.push_str(
            " AND (b.deps_reconciled=0 OR EXISTS(SELECT 1 FROM borrower_assets a
                WHERE a.chain_id=b.chain_id AND a.pool=b.pool AND a.asset=?3
                  AND a.borrower=b.borrower))",
        );
    }
    sql.push_str(" ORDER BY (s.hf IS NULL), s.hf, b.borrower");
    let mut st = c.prepare(&sql)?;
    let cid = i64_of(chain_id, "chain_id")?;
    let rows: Vec<Vec<u8>> = match asset {
        Some(a) => st
            .query_map(params![cid, pool.as_slice(), a.as_slice()], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
        None => st
            .query_map(params![cid, pool.as_slice()], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
    };
    rows.iter().map(|b| addr_of(b)).collect()
}

/// Mark the first `limit` immediately, queue the rest (coalescing on borrower+reason).
#[allow(clippy::too_many_arguments)]
fn fan_out(
    c: &Connection,
    chain_id: u64,
    pool: Address,
    list: &[Address],
    reason: DirtyReason,
    now_ns: u64,
    at: Option<ChainPoint>,
    limit: usize,
    out: &mut ApplyOutcome,
) -> Result<(), RepoError> {
    let (num, hash) = point_cols(at)?;
    for (i, b) in list.iter().enumerate() {
        if i < limit {
            mark_at(c, &key(chain_id, pool, *b), reason, now_ns, at)?;
            out.marked += 1;
        } else {
            c.execute(
                "INSERT INTO fanout_queue(chain_id,pool,borrower,reason,marked_ns,block_number,block_hash)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(chain_id,pool,borrower,reason) DO UPDATE SET
                   marked_ns=MAX(marked_ns, excluded.marked_ns),
                   block_hash=CASE WHEN excluded.block_number IS NOT NULL
                                    AND (block_number IS NULL OR excluded.block_number >= block_number)
                                   THEN excluded.block_hash ELSE block_hash END,
                   block_number=CASE WHEN excluded.block_number IS NOT NULL
                                      AND (block_number IS NULL OR excluded.block_number >= block_number)
                                     THEN excluded.block_number ELSE block_number END",
                params![
                    i64_of(chain_id, "chain_id")?,
                    pool.as_slice(),
                    b.as_slice(),
                    reason.code(),
                    i64_of(now_ns, "now_ns")?,
                    num,
                    hash
                ],
            )?;
            out.queued += 1;
        }
    }
    Ok(())
}

fn set_flag_in(
    c: &Connection,
    chain_id: u64,
    pool: Address,
    flag: &str,
    at: Option<ChainPoint>,
    detail: &str,
    now_ns: u64,
) -> Result<(), RepoError> {
    c.execute(
        "INSERT INTO pool_flags(chain_id,pool,flag,block_number,detail,set_ns) VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(chain_id,pool,flag) DO UPDATE SET
           block_number=excluded.block_number, detail=excluded.detail, set_ns=excluded.set_ns",
        params![
            i64_of(chain_id, "chain_id")?,
            pool.as_slice(),
            flag,
            at.map(|p| i64_of(p.block_number, "block_number")).transpose()?,
            detail,
            i64_of(now_ns, "now_ns")?
        ],
    )?;
    Ok(())
}

fn apply_effect(
    c: &Connection,
    b: &ScanBatch,
    e: &Effect,
    out: &mut ApplyOutcome,
) -> Result<(), RepoError> {
    let (cid, pool) = (b.chain_id, b.pool);
    match e {
        Effect::Discover { borrower, at } => {
            let k = key(cid, pool, *borrower);
            if register(c, &k, Some(*at), b.now_ns)? {
                out.registered += 1;
                mark_at(c, &k, DirtyReason::Discovered, b.now_ns, Some(*at))?;
                out.marked += 1;
            }
        }
        Effect::Touch {
            borrower,
            reason,
            at,
            register: reg,
            stale_deps,
        } => {
            let k = key(cid, pool, *borrower);
            let known = registered(c, cid, pool, *borrower)?;
            if known.is_none() && reg.is_none() {
                return Ok(());
            }
            if let Some(first) = reg {
                // New borrower: discovered at the FIRST registering event, so a reorg that drops
                // only a later touch keeps it. A known one is (re)activated.
                if register(c, &k, Some(*first), b.now_ns)? {
                    out.registered += 1;
                    mark_at(c, &k, DirtyReason::Discovered, b.now_ns, Some(*first))?;
                    out.marked += 1;
                }
            }
            mark_at(c, &k, *reason, b.now_ns, Some(*at))?;
            out.marked += 1;
            c.execute(
                "UPDATE borrowers SET last_touch_block=MAX(last_touch_block, ?4),
                   deps_stale=CASE WHEN ?5 THEN 1 ELSE deps_stale END
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![
                    i64_of(cid, "chain_id")?,
                    pool.as_slice(),
                    borrower.as_slice(),
                    i64_of(at.block_number, "block_number")?,
                    *stale_deps
                ],
            )?;
            c.execute(
                "INSERT OR IGNORE INTO scan_touched(name,block_number,block_hash,borrower) VALUES(?1,?2,?3,?4)",
                params![
                    b.name,
                    i64_of(at.block_number, "block_number")?,
                    at.block_hash.as_slice(),
                    borrower.as_slice()
                ],
            )?;
        }
        Effect::DepAdd {
            borrower,
            asset,
            kind,
        } => {
            if registered(c, cid, pool, *borrower)?.is_some() {
                c.execute(
                    "INSERT INTO borrower_assets(chain_id,pool,asset,borrower,kind) VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(chain_id,pool,asset,borrower) DO UPDATE SET kind=kind | excluded.kind",
                    params![
                        i64_of(cid, "chain_id")?,
                        pool.as_slice(),
                        asset.as_slice(),
                        borrower.as_slice(),
                        i64::from(*kind)
                    ],
                )?;
            }
        }
        Effect::DepRemove {
            borrower,
            asset,
            kind,
        } => {
            let args = params![
                i64_of(cid, "chain_id")?,
                pool.as_slice(),
                asset.as_slice(),
                borrower.as_slice(),
                i64::from(*kind)
            ];
            c.execute(
                "UPDATE borrower_assets SET kind = kind & ~?5
                 WHERE chain_id=?1 AND pool=?2 AND asset=?3 AND borrower=?4",
                args,
            )?;
            c.execute(
                "DELETE FROM borrower_assets WHERE chain_id=?1 AND pool=?2 AND asset=?3 AND borrower=?4 AND kind=0",
                params![
                    i64_of(cid, "chain_id")?,
                    pool.as_slice(),
                    asset.as_slice(),
                    borrower.as_slice()
                ],
            )?;
        }
        Effect::AssetDirty { asset, reason, at } => {
            let list = holders(c, cid, pool, Some(*asset))?;
            fan_out(
                c,
                cid,
                pool,
                &list,
                *reason,
                b.now_ns,
                *at,
                b.fanout_limit,
                out,
            )?;
        }
        Effect::AllDirty { reason, at } => {
            let list = holders(c, cid, pool, None)?;
            fan_out(
                c,
                cid,
                pool,
                &list,
                *reason,
                b.now_ns,
                *at,
                b.fanout_limit,
                out,
            )?;
        }
        Effect::Flag { flag, at, detail } => {
            set_flag_in(c, cid, pool, flag, *at, detail, b.now_ns)?;
        }
    }
    Ok(())
}

const ANCESTRY_KEEP: i64 = 512;
const TOUCHED_KEEP: i64 = 2048;

impl SnapshotRepository {
    pub fn scan_cursor(&self, name: &str) -> Result<Option<ScanCursor>, RepoError> {
        let c = self.conn();
        let row = c
            .query_row(
                "SELECT next_block,tip_number,tip_hash,back_block FROM scan_cursor WHERE name=?1",
                [name],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<Vec<u8>>>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(n, tn, th, bb)| {
            Ok(ScanCursor {
                next_block: u64_of(n, "next_block")?,
                tip: match (tn, th) {
                    (Some(n), Some(h)) => Some(ChainPoint {
                        block_number: u64_of(n, "tip_number")?,
                        block_hash: b256_of(&h)?,
                    }),
                    _ => None,
                },
                back_block: u64_of(bb, "back_block")?,
            })
        })
        .transpose()
    }

    /// Create the cursor if absent (never moves an existing one). Returns the stored cursor.
    pub fn init_scan_cursor(&self, name: &str, start: ScanCursor) -> Result<ScanCursor, RepoError> {
        {
            let c = self.conn();
            let (tn, th) = point_cols(start.tip)?;
            c.execute(
                "INSERT OR IGNORE INTO scan_cursor(name,next_block,tip_number,tip_hash,back_block)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    name,
                    i64_of(start.next_block, "next_block")?,
                    tn,
                    th,
                    i64_of(start.back_block, "back_block")?
                ],
            )?;
        }
        self.scan_cursor(name)?
            .ok_or_else(|| RepoError::Corrupt("cursor vanished".into()))
    }

    /// Recorded block ancestry, newest first.
    pub fn scan_ancestry(&self, name: &str, limit: usize) -> Result<Vec<ChainPoint>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT block_number,block_hash FROM scan_blocks WHERE name=?1
             ORDER BY block_number DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![name, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (n, h) = row?;
            out.push(ChainPoint {
                block_number: u64_of(n, "block_number")?,
                block_hash: b256_of(&h)?,
            });
        }
        Ok(out)
    }

    /// Apply a scanned range: effects, cursor, ancestry and gap bookkeeping in one transaction.
    pub fn apply_scan(&self, b: &ScanBatch) -> Result<ApplyOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut out = ApplyOutcome::default();
        for e in &b.effects {
            apply_effect(&tx, b, e, &mut out)?;
        }
        for p in b.ancestry.iter().chain(b.tip.iter()) {
            tx.execute(
                "INSERT OR REPLACE INTO scan_blocks(name,block_number,block_hash) VALUES(?1,?2,?3)",
                params![
                    b.name,
                    i64_of(p.block_number, "block_number")?,
                    p.block_hash.as_slice()
                ],
            )?;
        }
        if let Some(next) = b.next_block {
            let (tn, th) = point_cols(b.tip)?;
            tx.execute(
                "UPDATE scan_cursor SET next_block=?2, tip_number=COALESCE(?3,tip_number),
                   tip_hash=COALESCE(?4,tip_hash) WHERE name=?1",
                params![b.name, i64_of(next, "next_block")?, tn, th],
            )?;
        }
        if let Some(back) = b.back_block {
            tx.execute(
                "UPDATE scan_cursor SET back_block=?2 WHERE name=?1",
                params![b.name, i64_of(back, "back_block")?],
            )?;
        }
        for g in &b.new_gaps {
            tx.execute(
                "INSERT INTO coverage_gaps(name,direction,from_block,to_block,reason,recorded_ns)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    b.name,
                    g.dir.code(),
                    i64_of(g.from_block, "from_block")?,
                    i64_of(g.to_block, "to_block")?,
                    g.reason,
                    i64_of(b.now_ns, "now_ns")?
                ],
            )?;
        }
        for id in &b.resolved_gaps {
            tx.execute(
                "UPDATE coverage_gaps SET resolved_ns=?2 WHERE id=?1 AND resolved_ns IS NULL",
                params![id, i64_of(b.now_ns, "now_ns")?],
            )?;
        }
        for id in &b.failed_gap_attempts {
            tx.execute(
                "UPDATE coverage_gaps SET attempts=attempts+1 WHERE id=?1",
                [id],
            )?;
        }
        if let Some(tip) = b.tip {
            let floor = i64_of(tip.block_number, "block_number")?;
            tx.execute(
                "DELETE FROM scan_blocks WHERE name=?1 AND block_number < ?2",
                params![b.name, floor - ANCESTRY_KEEP],
            )?;
            tx.execute(
                "DELETE FROM scan_touched WHERE name=?1 AND block_number < ?2",
                params![b.name, floor - TOUCHED_KEEP],
            )?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// Roll the forward cursor back to `ancestor` after a reorg (see module docs).
    pub fn rewind_scan(
        &self,
        name: &str,
        chain_id: u64,
        pool: Address,
        ancestor: ChainPoint,
        now_ns: u64,
    ) -> Result<RewindOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let num = i64_of(ancestor.block_number, "block_number")?;
        let cid = i64_of(chain_id, "chain_id")?;
        let mut out = RewindOutcome::default();
        // Borrowers touched on the dropped blocks (also: a different hash at the ancestor height).
        let touched: Vec<Vec<u8>> = {
            let mut st = tx.prepare(
                "SELECT DISTINCT borrower FROM scan_touched WHERE name=?1
                 AND (block_number>?2 OR (block_number=?2 AND block_hash<>?3))",
            )?;
            st.query_map(params![name, num, ancestor.block_hash.as_slice()], |r| {
                r.get(0)
            })?
            .collect::<Result<_, _>>()?
        };
        tx.execute(
            "DELETE FROM scan_touched WHERE name=?1
             AND (block_number>?2 OR (block_number=?2 AND block_hash<>?3))",
            params![name, num, ancestor.block_hash.as_slice()],
        )?;
        tx.execute(
            "DELETE FROM scan_blocks WHERE name=?1
             AND (block_number>?2 OR (block_number=?2 AND block_hash<>?3))",
            params![name, num, ancestor.block_hash.as_slice()],
        )?;
        // Borrowers first seen on the dropped fork are not known to exist on the canonical chain.
        let orphan: Vec<Vec<u8>> = {
            let mut st = tx.prepare(
                "SELECT borrower FROM borrowers WHERE chain_id=?1 AND pool=?2 AND discovered_block>0
                 AND (discovered_block>?3 OR (discovered_block=?3 AND discovered_hash<>?4))",
            )?;
            st.query_map(
                params![cid, pool.as_slice(), num, ancestor.block_hash.as_slice()],
                |r| r.get(0),
            )?
            .collect::<Result<_, _>>()?
        };
        for b in &orphan {
            for t in [
                "DELETE FROM borrower_assets WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                "DELETE FROM dirty WHERE chain_id=?1 AND pool=?2 AND borrower=?3
                   AND borrower NOT IN (SELECT borrower FROM snapshots WHERE chain_id=?1 AND pool=?2)",
                "DELETE FROM fanout_queue WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                "DELETE FROM borrowers WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            ] {
                tx.execute(t, params![cid, pool.as_slice(), b])?;
            }
            out.removed_borrowers += 1;
        }
        for b in &touched {
            let a = addr_of(b)?;
            let k = key(chain_id, pool, a);
            if registered(&tx, chain_id, pool, a)?.is_some() {
                mark_at(&tx, &k, DirtyReason::Reorg, now_ns, Some(ancestor))?;
                tx.execute(
                    "UPDATE borrowers SET deps_stale=1, last_touch_block=MIN(last_touch_block, ?4)
                     WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                    params![cid, pool.as_slice(), b, num],
                )?;
                out.touched_marked += 1;
            }
        }
        // `tip` is the last scanned block; `next_block` follows it.
        tx.execute(
            "UPDATE scan_cursor SET next_block=?2, tip_number=?3, tip_hash=?4 WHERE name=?1",
            params![name, num + 1, num, ancestor.block_hash.as_slice()],
        )?;
        // Forward gaps entirely above the ancestor are rescanned anyway.
        tx.execute(
            "DELETE FROM coverage_gaps WHERE name=?1 AND direction=0 AND resolved_ns IS NULL AND from_block>?2",
            params![name, num],
        )?;
        tx.execute(
            "UPDATE coverage_gaps SET to_block=?2 WHERE name=?1 AND direction=0 AND resolved_ns IS NULL AND to_block>?2",
            params![name, num],
        )?;
        out.invalidated_snapshots = invalidate_in_tx(&tx, ancestor, now_ns)?;
        tx.commit()?;
        Ok(out)
    }

    /// Open (unresolved) gaps, oldest first.
    pub fn open_gaps(&self, name: &str) -> Result<Vec<CoverageGap>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT id,direction,from_block,to_block,reason,recorded_ns,attempts FROM coverage_gaps
             WHERE name=?1 AND resolved_ns IS NULL ORDER BY id",
        )?;
        let rows = st.query_map([name], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, d, f, t, reason, rec, att) = row?;
            out.push(CoverageGap {
                id,
                name: name.to_string(),
                dir: ScanDir::from_code(d)?,
                from_block: u64_of(f, "from_block")?,
                to_block: u64_of(t, "to_block")?,
                reason,
                recorded_ns: u64_of(rec, "recorded_ns")?,
                attempts: u32::try_from(att).unwrap_or(u32::MAX),
            });
        }
        Ok(out)
    }

    pub fn set_flag(
        &self,
        chain_id: u64,
        pool: Address,
        flag: &str,
        at: Option<ChainPoint>,
        detail: &str,
        now_ns: u64,
    ) -> Result<(), RepoError> {
        set_flag_in(&self.conn(), chain_id, pool, flag, at, detail, now_ns)
    }

    pub fn clear_flag(&self, chain_id: u64, pool: Address, flag: &str) -> Result<bool, RepoError> {
        let n = self.conn().execute(
            "DELETE FROM pool_flags WHERE chain_id=?1 AND pool=?2 AND flag=?3",
            params![i64_of(chain_id, "chain_id")?, pool.as_slice(), flag],
        )?;
        Ok(n > 0)
    }

    pub fn flags(&self, chain_id: u64, pool: Address) -> Result<Vec<PoolFlag>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT flag,block_number,detail,set_ns FROM pool_flags WHERE chain_id=?1 AND pool=?2 ORDER BY flag",
        )?;
        let rows = st.query_map(
            params![i64_of(chain_id, "chain_id")?, pool.as_slice()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;
        let mut out = vec![];
        for row in rows {
            let (flag, bn, detail, ns) = row?;
            out.push(PoolFlag {
                flag,
                block_number: bn.map(|v| u64_of(v, "block_number")).transpose()?,
                detail,
                set_ns: u64_of(ns, "set_ns")?,
            });
        }
        Ok(out)
    }

    pub fn has_flag(&self, chain_id: u64, pool: Address, flag: &str) -> Result<bool, RepoError> {
        Ok(self.flags(chain_id, pool)?.iter().any(|f| f.flag == flag))
    }

    /// Register borrowers outright (legacy-cache import, manual add): each new one is marked
    /// `Discovered`. Returns how many were new.
    pub fn register_borrowers(
        &self,
        chain_id: u64,
        pool: Address,
        borrowers: &[Address],
        at: Option<ChainPoint>,
        now_ns: u64,
    ) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut n = 0;
        for b in borrowers {
            let k = key(chain_id, pool, *b);
            if register(&tx, &k, at, now_ns)? {
                mark_at(&tx, &k, DirtyReason::Discovered, now_ns, at)?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Register borrowers this repository has never seen (marked `Discovered`); a borrower it
    /// already knows is left exactly as it is, including one reconciliation deactivated. For
    /// bulk imports from another index at startup; a live event uses `register_borrowers`,
    /// which also reactivates a returning borrower. Returns how many were new.
    pub fn import_borrowers(
        &self,
        chain_id: u64,
        pool: Address,
        borrowers: &[Address],
        now_ns: u64,
    ) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut n = 0;
        for b in borrowers {
            let k = key(chain_id, pool, *b);
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO borrowers(chain_id,pool,borrower,active,discovered_block,discovered_hash,discovered_ns)
                 VALUES(?1,?2,?3,1,0,NULL,?4)",
                params![
                    i64_of(chain_id, "chain_id")?,
                    pool.as_slice(),
                    b.as_slice(),
                    i64_of(now_ns, "now_ns")?
                ],
            )?;
            if inserted == 1 {
                mark_at(&tx, &k, DirtyReason::Discovered, now_ns, None)?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn borrower(
        &self,
        chain_id: u64,
        pool: Address,
        borrower: Address,
    ) -> Result<Option<BorrowerRecord>, RepoError> {
        registered(&self.conn(), chain_id, pool, borrower)
    }

    pub fn active_borrowers(
        &self,
        chain_id: u64,
        pool: Address,
    ) -> Result<Vec<Address>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT borrower FROM borrowers WHERE chain_id=?1 AND pool=?2 AND active=1 ORDER BY borrower",
        )?;
        let rows: Vec<Vec<u8>> = st
            .query_map(
                params![i64_of(chain_id, "chain_id")?, pool.as_slice()],
                |r| r.get(0),
            )?
            .collect::<Result<_, _>>()?;
        rows.iter().map(|b| addr_of(b)).collect()
    }

    /// Exact reverse-index lookup: active borrowers with a recorded dependency on `asset`
    /// (does NOT include unreconciled borrowers; `AssetDirty` does).
    pub fn asset_borrowers(
        &self,
        chain_id: u64,
        pool: Address,
        asset: Address,
    ) -> Result<Vec<(Address, u8)>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT a.borrower,a.kind FROM borrower_assets a
             JOIN borrowers b ON b.chain_id=a.chain_id AND b.pool=a.pool AND b.borrower=a.borrower
             WHERE a.chain_id=?1 AND a.pool=?2 AND a.asset=?3 AND b.active=1 ORDER BY a.borrower",
        )?;
        let rows = st.query_map(
            params![
                i64_of(chain_id, "chain_id")?,
                pool.as_slice(),
                asset.as_slice()
            ],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut out = vec![];
        for row in rows {
            let (b, k) = row?;
            out.push((addr_of(&b)?, u8::try_from(k).unwrap_or(0)));
        }
        Ok(out)
    }

    pub fn borrower_dependencies(
        &self,
        chain_id: u64,
        pool: Address,
        borrower: Address,
    ) -> Result<Vec<(Address, u8)>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT asset,kind FROM borrower_assets WHERE chain_id=?1 AND pool=?2 AND borrower=?3 ORDER BY asset",
        )?;
        let rows = st.query_map(
            params![
                i64_of(chain_id, "chain_id")?,
                pool.as_slice(),
                borrower.as_slice()
            ],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut out = vec![];
        for row in rows {
            let (a, k) = row?;
            out.push((addr_of(&a)?, u8::try_from(k).unwrap_or(0)));
        }
        Ok(out)
    }

    /// Active borrowers whose dependency set was never reconciled or went stale.
    pub fn list_unreconciled(
        &self,
        chain_id: u64,
        pool: Address,
        limit: usize,
    ) -> Result<Vec<Address>, RepoError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT borrower FROM borrowers WHERE chain_id=?1 AND pool=?2 AND active=1
               AND (deps_reconciled=0 OR deps_stale=1) ORDER BY deps_stale DESC, borrower LIMIT ?3",
        )?;
        let rows: Vec<Vec<u8>> = st
            .query_map(
                params![i64_of(chain_id, "chain_id")?, pool.as_slice(), limit as i64],
                |r| r.get(0),
            )?
            .collect::<Result<_, _>>()?;
        rows.iter().map(|b| addr_of(b)).collect()
    }

    /// Replace a borrower's dependency set with what the chain says at `at` (user configuration
    /// bitmap + reserve list) and reconcile activity: a borrower with no debt is deactivated and
    /// its dependencies removed (a later `Borrow` re-registers it). A read older than the newest
    /// event touching the borrower is refused (`Superseded`) so stale data never wins.
    pub fn reconcile_borrower(
        &self,
        chain_id: u64,
        pool: Address,
        borrower: Address,
        deps: &[(Address, u8)],
        has_debt: bool,
        at: ChainPoint,
    ) -> Result<ReconcileOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(rec) = registered(&tx, chain_id, pool, borrower)? else {
            return Ok(ReconcileOutcome::Unknown);
        };
        if at.block_number < rec.last_touch_block {
            return Ok(ReconcileOutcome::Superseded);
        }
        let cid = i64_of(chain_id, "chain_id")?;
        tx.execute(
            "DELETE FROM borrower_assets WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            params![cid, pool.as_slice(), borrower.as_slice()],
        )?;
        if has_debt {
            for (asset, kind) in deps.iter().filter(|(_, k)| *k != 0) {
                tx.execute(
                    "INSERT INTO borrower_assets(chain_id,pool,asset,borrower,kind) VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(chain_id,pool,asset,borrower) DO UPDATE SET kind=kind | excluded.kind",
                    params![
                        cid,
                        pool.as_slice(),
                        asset.as_slice(),
                        borrower.as_slice(),
                        i64::from(*kind)
                    ],
                )?;
            }
        }
        if !has_debt {
            // Nothing left to recheck: a later `Borrow` re-registers and re-marks the account.
            tx.execute(
                "DELETE FROM dirty WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![cid, pool.as_slice(), borrower.as_slice()],
            )?;
        }
        tx.execute(
            "UPDATE borrowers SET active=?4, deps_reconciled=1, deps_stale=0
             WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            params![
                cid,
                pool.as_slice(),
                borrower.as_slice(),
                i64::from(has_debt)
            ],
        )?;
        tx.commit()?;
        Ok(if has_debt {
            ReconcileOutcome::Active
        } else {
            ReconcileOutcome::Deactivated
        })
    }

    /// Immediate marks drained from the fan-out queue (at most `budget`), oldest first.
    pub fn drain_fanout(
        &self,
        chain_id: u64,
        pool: Address,
        budget: usize,
    ) -> Result<usize, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cid = i64_of(chain_id, "chain_id")?;
        type QueueRow = (i64, Vec<u8>, i64, i64, Option<i64>, Option<Vec<u8>>);
        let rows: Vec<QueueRow> = {
            let mut st = tx.prepare(
                "SELECT seq,borrower,reason,marked_ns,block_number,block_hash FROM fanout_queue
                 WHERE chain_id=?1 AND pool=?2 ORDER BY seq LIMIT ?3",
            )?;
            st.query_map(params![cid, pool.as_slice(), budget as i64], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<Result<_, _>>()?
        };
        for (seq, b, reason, ns, bn, bh) in &rows {
            let reason = DirtyReason::from_code(*reason)
                .ok_or_else(|| RepoError::Corrupt("fanout reason".into()))?;
            let at = match (bn, bh) {
                (Some(n), Some(h)) => Some(ChainPoint {
                    block_number: u64_of(*n, "block_number")?,
                    block_hash: b256_of(h)?,
                }),
                _ => None,
            };
            // A borrower removed or deactivated while queued needs no recheck.
            if registered(&tx, chain_id, pool, addr_of(b)?)?.is_some_and(|r| r.active) {
                mark_at(
                    &tx,
                    &key(chain_id, pool, addr_of(b)?),
                    reason,
                    u64_of(*ns, "marked_ns")?,
                    at,
                )?;
            }
            tx.execute("DELETE FROM fanout_queue WHERE seq=?1", [seq])?;
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub fn fanout_pending(&self, chain_id: u64, pool: Address) -> Result<usize, RepoError> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM fanout_queue WHERE chain_id=?1 AND pool=?2",
            params![i64_of(chain_id, "chain_id")?, pool.as_slice()],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    pub fn oracle_price(
        &self,
        chain_id: u64,
        pool: Address,
        asset: Address,
    ) -> Result<Option<U256>, RepoError> {
        let b: Option<Vec<u8>> = self
            .conn()
            .query_row(
                "SELECT price FROM oracle_prices WHERE chain_id=?1 AND pool=?2 AND asset=?3",
                params![
                    i64_of(chain_id, "chain_id")?,
                    pool.as_slice(),
                    asset.as_slice()
                ],
                |r| r.get(0),
            )
            .optional()?;
        b.map(|b| u256_of(&b)).transpose()
    }

    /// Store new price baselines and apply the matching `PriceMove` asset marks atomically.
    pub fn record_price_moves(
        &self,
        chain_id: u64,
        pool: Address,
        moves: &[(Address, U256)],
        at: Option<ChainPoint>,
        now_ns: u64,
        fanout_limit: usize,
    ) -> Result<ApplyOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let batch = ScanBatch::new("oracle", chain_id, pool, now_ns, fanout_limit);
        let mut out = ApplyOutcome::default();
        for (asset, price) in moves {
            tx.execute(
                "INSERT OR REPLACE INTO oracle_prices(chain_id,pool,asset,price,block_number) VALUES(?1,?2,?3,?4,?5)",
                params![
                    i64_of(chain_id, "chain_id")?,
                    pool.as_slice(),
                    asset.as_slice(),
                    price.to_be_bytes::<32>().as_slice(),
                    at.map(|p| i64_of(p.block_number, "block_number")).transpose()?
                ],
            )?;
            apply_effect(
                &tx,
                &batch,
                &Effect::AssetDirty {
                    asset: *asset,
                    reason: DirtyReason::PriceMove,
                    at,
                },
                &mut out,
            )?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// Mark every active holder of `asset` (reverse index; unreconciled borrowers count as
    /// holders), at most `fanout_limit` now and the rest queued durably.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_asset_holders(
        &self,
        chain_id: u64,
        pool: Address,
        asset: Address,
        reason: DirtyReason,
        at: Option<ChainPoint>,
        now_ns: u64,
        fanout_limit: usize,
    ) -> Result<ApplyOutcome, RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let batch = ScanBatch::new("oracle", chain_id, pool, now_ns, fanout_limit);
        let mut out = ApplyOutcome::default();
        apply_effect(
            &tx,
            &batch,
            &Effect::AssetDirty { asset, reason, at },
            &mut out,
        )?;
        tx.commit()?;
        Ok(out)
    }

    /// Store baselines without marking anything (first sighting).
    pub fn seed_prices(
        &self,
        chain_id: u64,
        pool: Address,
        prices: &[(Address, U256)],
        at: Option<ChainPoint>,
    ) -> Result<(), RepoError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (asset, price) in prices {
            tx.execute(
                "INSERT OR IGNORE INTO oracle_prices(chain_id,pool,asset,price,block_number) VALUES(?1,?2,?3,?4,?5)",
                params![
                    i64_of(chain_id, "chain_id")?,
                    pool.as_slice(),
                    asset.as_slice(),
                    price.to_be_bytes::<32>().as_slice(),
                    at.map(|p| i64_of(p.block_number, "block_number")).transpose()?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Dirty accounts with their most urgent reason and latest chain position, most urgent first
    /// (then oldest mark first). What the scheduler (task 005) consumes.
    pub fn dirty_entries(&self) -> Result<Vec<DirtyEntry>, RepoError> {
        self.dirty_entries_where(None, &|_| true)
    }

    /// `dirty_entries` restricted to one `(chain_id, pool)` (when given) and to the accounts
    /// `keep` accepts. Rows are streamed, so only the kept entries are held in memory.
    pub fn dirty_entries_where(
        &self,
        pool: Option<(u64, Address)>,
        keep: &dyn Fn(&AccountKey) -> bool,
    ) -> Result<Vec<DirtyEntry>, RepoError> {
        let c = self.conn();
        let (cid, pool_bytes) = match pool {
            Some((cid, p)) => (Some(i64_of(cid, "chain_id")?), Some(p.as_slice().to_vec())),
            None => (None, None),
        };
        let mut st = c.prepare(
            "SELECT chain_id,pool,borrower,reason,marked_at_ns,block_number,block_hash FROM dirty
             WHERE (?1 IS NULL OR (chain_id=?1 AND pool=?2))
             ORDER BY chain_id,pool,borrower",
        )?;
        let rows = st.query_map(params![cid, pool_bytes], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<Vec<u8>>>(6)?,
            ))
        })?;
        let mut out: Vec<DirtyEntry> = vec![];
        for row in rows {
            let (cid, p, b, reason, ns, bn, bh) = row?;
            let k = AccountKey {
                chain_id: u64_of(cid, "chain_id")?,
                pool: addr_of(&p)?,
                borrower: addr_of(&b)?,
            };
            if !keep(&k) {
                continue;
            }
            let reason = DirtyReason::from_code(reason)
                .ok_or_else(|| RepoError::Corrupt("dirty reason".into()))?;
            let ns = u64_of(ns, "marked_at_ns")?;
            let chain = match (bn, bh) {
                (Some(n), Some(h)) => Some(ChainPoint {
                    block_number: u64_of(n, "block_number")?,
                    block_hash: b256_of(&h)?,
                }),
                _ => None,
            };
            match out.last_mut().filter(|e| e.key == k) {
                Some(e) => {
                    e.reasons.push(reason);
                    if reason.urgency() > e.most_urgent.urgency() {
                        e.most_urgent = reason;
                    }
                    e.marked_at_ns = e.marked_at_ns.max(ns);
                    if chain.is_some_and(|c| {
                        e.latest_chain
                            .is_none_or(|o| c.block_number >= o.block_number)
                    }) {
                        e.latest_chain = chain;
                    }
                }
                None => out.push(DirtyEntry {
                    key: k,
                    reasons: vec![reason],
                    most_urgent: reason,
                    latest_chain: chain,
                    marked_at_ns: ns,
                }),
            }
        }
        out.sort_by(|a, b| {
            b.most_urgent
                .urgency()
                .cmp(&a.most_urgent.urgency())
                .then(a.marked_at_ns.cmp(&b.marked_at_ns))
                .then(a.key.cmp(&b.key))
        });
        Ok(out)
    }
}
