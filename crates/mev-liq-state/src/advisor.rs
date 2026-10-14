//! Transactional advisor outbox and advisory storage (JEV-LIQ-008).
//!
//! Two SQLite files (see `docs/design/jev-liq-008-outbox.md`):
//!
//! * the **snapshot DB** (`SnapshotRepository`, migration 4) owns an append-only *export log*
//!   (`advisor_export`) plus the per-account current export (`advisor_current`) and shed records.
//!   `enqueue_with_snapshot` writes the account CAS and the export row in ONE transaction, so an
//!   advisory request can never exist for state that was not committed, and committed state is
//!   never left without its export;
//! * the **advisory DB** (`AdvisorStore`, its own migrations) owns the claimable queue
//!   (`advisor_outbox`: leases, attempts, retry, dead letter), decisions, the per-account revision
//!   high-water mark and policy outcomes. The advisor reaches the snapshot DB only through
//!   `SnapshotReader`, opened `SQLITE_OPEN_READ_ONLY` with `query_only`, so Process B cannot write
//!   account state or the export log (spec invariant 7).
//!
//! All times are caller-supplied nanoseconds (no hidden clock). A lease is held while
//! `lease_until_ns > now_ns`. Advisory data is optional: nothing here can delete or demote a
//! mandatory recheck, and a shed export row only loses an optional advisory request.
//!
//! Decision probabilities/scores are model outputs and are `f64` by spec (section 8); every other
//! quantity stays exact.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use alloy::primitives::{B256, keccak256};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::domain::{AccountKey, BorrowerSnapshot};
use crate::repo::{
    RepoError, SnapshotRepository, addr_of, b256_of, i64_of, load, u64_of, upsert_cas_in,
};

pub(crate) const MIGRATION_4_UP: &str = "
CREATE TABLE advisor_export(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_key BLOB NOT NULL UNIQUE CHECK(length(event_key)=32),
  chain_id INTEGER NOT NULL CHECK(chain_id>0),
  pool BLOB NOT NULL CHECK(length(pool)=20),
  borrower BLOB NOT NULL CHECK(length(borrower)=20),
  revision INTEGER NOT NULL CHECK(revision>=0),
  feature_digest BLOB NOT NULL CHECK(length(feature_digest)=32),
  policy_version TEXT NOT NULL,
  feature_json TEXT NOT NULL CHECK(length(feature_json)>0),
  enqueued_at_ns INTEGER NOT NULL
);
CREATE TABLE advisor_current(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  revision INTEGER NOT NULL, feature_digest BLOB NOT NULL CHECK(length(feature_digest)=32),
  PRIMARY KEY(chain_id, pool, borrower)
) WITHOUT ROWID;
CREATE TABLE advisor_shed(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  first_id INTEGER NOT NULL, last_id INTEGER NOT NULL, count INTEGER NOT NULL CHECK(count>0),
  shed_at_ns INTEGER NOT NULL
);
";

pub(crate) const MIGRATION_4_DOWN: &str =
    "DROP TABLE advisor_shed; DROP TABLE advisor_current; DROP TABLE advisor_export;";

/// Largest `feature_json` accepted into the export log (the redacted contract is ~400 bytes).
pub const MAX_FEATURE_JSON_BYTES: usize = 16 * 1024;

// ---------------------------------------------------------------------------------------------
// Producer side (snapshot DB)
// ---------------------------------------------------------------------------------------------

/// One advisory request exported with a snapshot. `feature_json`/`feature_digest` come from
/// `RedactedFeatures::json()` / `digest()`; this crate cannot depend on the features crate.
#[derive(Debug, Clone, Copy)]
pub struct ExportItem<'a> {
    pub policy_version: &'a str,
    pub feature_digest: B256,
    pub feature_json: &'a str,
    pub enqueued_at_ns: u64,
}

/// Export-log bound. Over the bound the oldest rows are shed with a counted record.
#[derive(Debug, Clone, Copy)]
pub struct OutboxConfig {
    pub max_export_rows: u64,
}

#[derive(Debug, Clone)]
pub struct EnqueueOutcome {
    /// The snapshot as stored (revision assigned by CAS).
    pub snapshot: BorrowerSnapshot,
    pub event_key: B256,
    /// False when the same event key was already exported (dedup).
    pub enqueued: bool,
    /// Export rows shed by this call to honour the bound.
    pub shed: u64,
}

/// `keccak256(domain || chain || pool || borrower || revision || feature_digest || policy_version)`.
pub fn event_key(
    key: &AccountKey,
    revision: u64,
    feature_digest: B256,
    policy_version: &str,
) -> B256 {
    let mut b = Vec::with_capacity(128 + policy_version.len());
    b.extend_from_slice(b"mev-liq-state/advisor-event/v1\0");
    b.extend_from_slice(&key.chain_id.to_be_bytes());
    b.extend_from_slice(key.pool.as_slice());
    b.extend_from_slice(key.borrower.as_slice());
    b.extend_from_slice(&revision.to_be_bytes());
    b.extend_from_slice(feature_digest.as_slice());
    b.extend_from_slice(&(policy_version.len() as u64).to_be_bytes());
    b.extend_from_slice(policy_version.as_bytes());
    keccak256(&b)
}

/// Defence in depth behind the features crate's redaction: the export must be bounded JSON and
/// must not contain this account's own pool or borrower address in any hex spelling.
fn validate_export(
    key: &AccountKey,
    item: &ExportItem<'_>,
    cfg: &OutboxConfig,
) -> Result<(), RepoError> {
    if cfg.max_export_rows == 0 {
        return Err(RepoError::BadExport("max_export_rows must be positive"));
    }
    if item.policy_version.is_empty() || item.policy_version.len() > 64 {
        return Err(RepoError::BadExport("policy_version must be 1..=64 bytes"));
    }
    let j = item.feature_json;
    if j.is_empty() || j.len() > MAX_FEATURE_JSON_BYTES || !j.starts_with('{') || !j.ends_with('}')
    {
        return Err(RepoError::BadExport(
            "feature_json must be a bounded JSON object",
        ));
    }
    let lower = j.to_ascii_lowercase();
    for a in [key.pool, key.borrower] {
        let hex: String = a.as_slice().iter().map(|b| format!("{b:02x}")).collect();
        if lower.contains(&hex) {
            return Err(RepoError::BadExport(
                "feature_json contains an account address",
            ));
        }
    }
    Ok(())
}

impl SnapshotRepository {
    /// Update account state through the CAS path AND export advisor work in the SAME SQLite
    /// transaction. Any failure (stale revision, constraint, I/O) rolls back both writes.
    ///
    /// The export row is deduplicated on `event_key`; `advisor_current` always moves to the new
    /// revision and digest (that is what `apply_decision` compares against). If the log would
    /// exceed `cfg.max_export_rows`, the oldest rows are deleted and a `advisor_shed` record is
    /// written in the same transaction (the CAS itself is never shed or refused for queue size).
    pub fn enqueue_with_snapshot(
        &self,
        snapshot: &BorrowerSnapshot,
        expected_revision: u64,
        item: &ExportItem<'_>,
        cfg: &OutboxConfig,
    ) -> Result<EnqueueOutcome, RepoError> {
        validate_export(&snapshot.key, item, cfg)?;
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = upsert_cas_in(&tx, snapshot, expected_revision)?;
        let ev = event_key(
            &stored.key,
            stored.revision,
            item.feature_digest,
            item.policy_version,
        );
        let k = &stored.key;
        let chain = i64_of(k.chain_id, "chain_id")?;
        let rev = i64_of(stored.revision, "revision")?;
        let at = i64_of(item.enqueued_at_ns, "enqueued_at_ns")?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO advisor_export(event_key,chain_id,pool,borrower,revision,
               feature_digest,policy_version,feature_json,enqueued_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                ev.as_slice(),
                chain,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                rev,
                item.feature_digest.as_slice(),
                item.policy_version,
                item.feature_json,
                at
            ],
        )?;
        tx.execute(
            "INSERT INTO advisor_current(chain_id,pool,borrower,revision,feature_digest)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(chain_id,pool,borrower) DO UPDATE SET
               revision=excluded.revision, feature_digest=excluded.feature_digest",
            params![
                chain,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                rev,
                item.feature_digest.as_slice()
            ],
        )?;
        let shed = shed_over_bound(&tx, cfg.max_export_rows, at)?;
        tx.commit()?;
        Ok(EnqueueOutcome {
            snapshot: stored,
            event_key: ev,
            enqueued: inserted == 1,
            shed,
        })
    }

    /// Rows currently in the export log.
    pub fn export_len(&self) -> Result<u64, RepoError> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM advisor_export", [], |r| r.get(0))?;
        u64_of(n, "export_len")
    }

    /// Total export rows shed so far (sum of `advisor_shed.count`).
    pub fn shed_total(&self) -> Result<u64, RepoError> {
        shed_total_on(&self.conn())
    }
}

fn shed_total_on(c: &Connection) -> Result<u64, RepoError> {
    let n: i64 = c.query_row("SELECT COALESCE(SUM(count),0) FROM advisor_shed", [], |r| {
        r.get(0)
    })?;
    u64_of(n, "shed_total")
}

fn shed_over_bound(tx: &Transaction<'_>, max: u64, at_ns: i64) -> Result<u64, RepoError> {
    let n: i64 = tx.query_row("SELECT COUNT(*) FROM advisor_export", [], |r| r.get(0))?;
    let over = u64_of(n, "export_len")?.saturating_sub(max);
    if over == 0 {
        return Ok(0);
    }
    let over_i = i64_of(over, "shed")?;
    let (first, last): (i64, i64) = tx.query_row(
        "SELECT MIN(id), MAX(id) FROM (SELECT id FROM advisor_export ORDER BY id LIMIT ?1)",
        [over_i],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.execute(
        "DELETE FROM advisor_export WHERE id IN
           (SELECT id FROM advisor_export ORDER BY id LIMIT ?1)",
        [over_i],
    )?;
    tx.execute(
        "INSERT INTO advisor_shed(first_id,last_id,count,shed_at_ns) VALUES(?1,?2,?3,?4)",
        params![first, last, over_i, at_ns],
    )?;
    Ok(over)
}

// ---------------------------------------------------------------------------------------------
// Advisor-side read-only view of the snapshot DB
// ---------------------------------------------------------------------------------------------

/// One export-log row, as seen by the advisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRow {
    pub id: i64,
    pub event_key: B256,
    pub key: AccountKey,
    pub revision: u64,
    pub feature_digest: B256,
    pub policy_version: String,
    pub feature_json: String,
    pub enqueued_at_ns: u64,
}

/// The advisor's only handle on snapshot data: read-only at the SQLite level
/// (`SQLITE_OPEN_READ_ONLY` plus `PRAGMA query_only`).
pub struct SnapshotReader {
    conn: Mutex<Connection>,
}

impl SnapshotReader {
    /// Open an existing snapshot DB read-only. Fails if the DB is missing or below schema 4.
    pub fn open(path: &Path) -> Result<Self, AdvisorError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=10000;")?;
        let v: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 4 {
            return Err(AdvisorError::SnapshotSchemaTooOld(v));
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn fetch_by_key(&self, key: &AccountKey) -> Result<Option<BorrowerSnapshot>, AdvisorError> {
        Ok(load(&self.conn(), key)?)
    }

    /// `(revision, feature_digest)` of the account's latest export.
    pub fn current_export(&self, key: &AccountKey) -> Result<Option<(u64, B256)>, AdvisorError> {
        let row: Option<(i64, Vec<u8>)> = self
            .conn()
            .query_row(
                "SELECT revision, feature_digest FROM advisor_current
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![
                    i64_of(key.chain_id, "chain_id")?,
                    key.pool.as_slice(),
                    key.borrower.as_slice()
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(r, d)| Ok((u64_of(r, "revision")?, b256_of(&d)?)))
            .transpose()
    }

    /// Export rows with `id > after_id`, oldest first.
    pub fn export_after(&self, after_id: i64, limit: u32) -> Result<Vec<ExportRow>, AdvisorError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT id,event_key,chain_id,pool,borrower,revision,feature_digest,policy_version,
                    feature_json,enqueued_at_ns
             FROM advisor_export WHERE id>?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = st.query_map(params![after_id, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Vec<u8>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, i64>(9)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, ek, ch, p, b, rev, fd, pv, fj, at) = row?;
            out.push(ExportRow {
                id,
                event_key: b256_of(&ek)?,
                key: AccountKey {
                    chain_id: u64_of(ch, "chain_id")?,
                    pool: addr_of(&p)?,
                    borrower: addr_of(&b)?,
                },
                revision: u64_of(rev, "revision")?,
                feature_digest: b256_of(&fd)?,
                policy_version: pv,
                feature_json: fj,
                enqueued_at_ns: u64_of(at, "enqueued_at_ns")?,
            });
        }
        Ok(out)
    }

    pub fn shed_total(&self) -> Result<u64, AdvisorError> {
        Ok(shed_total_on(&self.conn())?)
    }
}

// ---------------------------------------------------------------------------------------------
// Advisory store (advisory DB)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AdvisorError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("snapshot repository: {0}")]
    Repo(#[from] RepoError),
    #[error("snapshot database schema {0} is older than 4 (advisor export missing)")]
    SnapshotSchemaTooOld(i64),
    #[error("advisor database schema {found} is newer than this build ({supported})")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("malformed decision: {0}")]
    Malformed(&'static str),
    #[error("decision expired: expires_at {expires_at_ns} <= now {now_ns}")]
    Expired { expires_at_ns: u64, now_ns: u64 },
    #[error("decision issued in the future")]
    IssuedInFuture,
    #[error("policy version mismatch")]
    PolicyVersionMismatch,
    #[error("feature schema mismatch")]
    FeatureSchemaMismatch,
    #[error("chain mismatch")]
    ChainMismatch,
    #[error("model not in the allowed list")]
    ModelNotAllowed,
    #[error("no snapshot for this account")]
    UnknownAccount,
    #[error("stale revision: decision {decision}, current {current}")]
    StaleRevision { decision: u64, current: u64 },
    #[error("decision revision {decision} is ahead of stored revision {current}")]
    FutureRevision { decision: u64, current: u64 },
    #[error("no current export for the stored revision")]
    NoCurrentExport,
    #[error("feature digest does not match the current export")]
    DigestMismatch,
    #[error("an older response cannot replace a newer one for the same key")]
    SupersededResponse,
    #[error("lease on row {0} is not held by this owner (lost, expired, or already settled)")]
    LeaseLost(i64),
}

pub const ADVISOR_SCHEMA_VERSION: i64 = 1;

const ADVISOR_MIGRATION_1_UP: &str = "
CREATE TABLE advisor_meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE advisor_outbox(
  id INTEGER PRIMARY KEY,
  event_key BLOB NOT NULL UNIQUE CHECK(length(event_key)=32),
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  revision INTEGER NOT NULL CHECK(revision>=0),
  feature_digest BLOB NOT NULL CHECK(length(feature_digest)=32),
  feature_json TEXT NOT NULL,
  enqueued_at_ns INTEGER NOT NULL,
  available_at_ns INTEGER NOT NULL,
  lease_owner TEXT, lease_until_ns INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
  consumed_at_ns INTEGER,
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','done','dead')),
  last_error TEXT
);
CREATE INDEX advisor_outbox_ready ON advisor_outbox(available_at_ns,id) WHERE state='pending';
CREATE TABLE advisory_decision(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  chain_id INTEGER NOT NULL CHECK(chain_id>0),
  pool BLOB NOT NULL CHECK(length(pool)=20),
  borrower BLOB NOT NULL CHECK(length(borrower)=20),
  revision INTEGER NOT NULL CHECK(revision>=0),
  feature_digest BLOB NOT NULL CHECK(length(feature_digest)=32),
  feature_schema TEXT NOT NULL,
  policy_version TEXT NOT NULL,
  prompt_digest BLOB NOT NULL CHECK(length(prompt_digest)=32),
  model TEXT NOT NULL,
  watch_tier TEXT NOT NULL CHECK(watch_tier IN ('high','normal','low')),
  confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1),
  prep_probability REAL NOT NULL CHECK(prep_probability BETWEEN 0 AND 1),
  value_score REAL NOT NULL CHECK(value_score>=0 AND value_score<=4),
  issued_at_ns INTEGER NOT NULL,
  expires_at_ns INTEGER NOT NULL CHECK(expires_at_ns>issued_at_ns),
  input_tokens INTEGER CHECK(input_tokens>=0),
  output_tokens INTEGER CHECK(output_tokens>=0),
  response_digest BLOB NOT NULL CHECK(length(response_digest)=32),
  decision_mode TEXT NOT NULL CHECK(decision_mode IN ('shadow','active')),
  UNIQUE(chain_id,pool,borrower,revision,feature_digest,policy_version,prompt_digest,model)
);
CREATE INDEX advisory_decision_lookup ON advisory_decision(chain_id,pool,borrower,revision,expires_at_ns DESC);
CREATE TABLE advisor_account_high(
  chain_id INTEGER NOT NULL, pool BLOB NOT NULL, borrower BLOB NOT NULL,
  revision INTEGER NOT NULL,
  PRIMARY KEY(chain_id,pool,borrower)
) WITHOUT ROWID;
CREATE TABLE policy_outcome(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL, policy_name TEXT NOT NULL,
  chain_id INTEGER NOT NULL,
  pool BLOB NOT NULL CHECK(length(pool)=20), borrower BLOB NOT NULL CHECK(length(borrower)=20),
  state_block INTEGER NOT NULL, event_kind TEXT NOT NULL,
  scheduled_at_ns INTEGER, observed_at_ns INTEGER, check_completed_at_ns INTEGER,
  eligible INTEGER, simulatable INTEGER,
  estimated_profit_base BLOB CHECK(estimated_profit_base IS NULL OR length(estimated_profit_base)=32),
  rpc_calls INTEGER NOT NULL DEFAULT 0, simulation_count INTEGER NOT NULL DEFAULT 0,
  reason_codes TEXT NOT NULL DEFAULT ''
);
CREATE INDEX policy_outcome_run ON policy_outcome(run_id,policy_name,state_block);
";

const ADVISOR_MIGRATION_1_DOWN: &str = "DROP TABLE policy_outcome; DROP TABLE advisor_account_high;
DROP TABLE advisory_decision; DROP TABLE advisor_outbox; DROP TABLE advisor_meta;";

/// Queue behaviour. `max_pending` bounds how much the advisor pulls from the export log;
/// `max_in_flight` bounds simultaneously leased rows.
#[derive(Debug, Clone, Copy)]
pub struct QueueConfig {
    pub max_pending: u64,
    pub max_in_flight: u64,
    pub max_attempts: u32,
    pub base_backoff_ns: u64,
    pub max_backoff_ns: u64,
}

impl QueueConfig {
    /// Backoff after the `attempts`-th failed attempt (1-based): `base * 2^(attempts-1)`, capped.
    pub fn backoff_ns(&self, attempts: u32) -> u64 {
        let shift = attempts.saturating_sub(1).min(63);
        self.base_backoff_ns
            .saturating_mul(1u64 << shift)
            .min(self.max_backoff_ns)
    }
}

/// A leased unit of advisory work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub id: i64,
    pub event_key: B256,
    pub key: AccountKey,
    pub revision: u64,
    pub feature_digest: B256,
    pub feature_json: String,
    /// Attempts including this one.
    pub attempts: u32,
    pub lease_until_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOutcome {
    Retry { available_at_ns: u64 },
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PullOutcome {
    pub copied: u64,
    /// Export ids that were shed before the advisor could copy them.
    pub missed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueStats {
    pub pending: u64,
    pub in_flight: u64,
    pub done: u64,
    pub dead: u64,
    pub missed_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTier {
    High,
    Normal,
    Low,
}

impl WatchTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
    fn parse(s: &str) -> Result<Self, AdvisorError> {
        match s {
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            o => Err(AdvisorError::Corrupt(format!("watch_tier {o}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionMode {
    Shadow,
    Active,
}

impl DecisionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Active => "active",
        }
    }
    fn parse(s: &str) -> Result<Self, AdvisorError> {
        match s {
            "shadow" => Ok(Self::Shadow),
            "active" => Ok(Self::Active),
            o => Err(AdvisorError::Corrupt(format!("decision_mode {o}"))),
        }
    }
}

/// A validated-on-apply advisory decision. Never proof of eligibility or profit.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvisoryDecision {
    pub key: AccountKey,
    pub revision: u64,
    pub feature_digest: B256,
    pub feature_schema: String,
    pub policy_version: String,
    pub prompt_digest: B256,
    pub model: String,
    pub watch_tier: WatchTier,
    pub confidence: f64,
    pub prep_probability: f64,
    pub value_score: f64,
    pub issued_at_ns: u64,
    pub expires_at_ns: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub response_digest: B256,
    pub mode: DecisionMode,
}

/// The configured side of the spec section 7 predicate.
#[derive(Debug, Clone)]
pub struct DecisionConfig {
    pub chain_id: u64,
    pub policy_version: String,
    pub feature_schema: String,
    pub allowed_models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Inserted,
    /// Same key, new successful call: issuance/expiry/response refreshed.
    Refreshed,
}

/// A `policy_outcome` row (evaluation record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyOutcome {
    pub run_id: String,
    pub policy_name: String,
    pub key: AccountKey,
    pub state_block: u64,
    pub event_kind: String,
    pub scheduled_at_ns: Option<u64>,
    pub observed_at_ns: Option<u64>,
    pub check_completed_at_ns: Option<u64>,
    pub eligible: Option<bool>,
    pub simulatable: Option<bool>,
    pub estimated_profit_base: Option<alloy::primitives::U256>,
    pub rpc_calls: u32,
    pub simulation_count: u32,
    pub reason_codes: Vec<String>,
}

pub struct AdvisorStore {
    conn: Mutex<Connection>,
    reader: SnapshotReader,
    cfg: QueueConfig,
}

fn opt_i64(v: Option<u64>, what: &'static str) -> Result<Option<i64>, RepoError> {
    v.map(|x| i64_of(x, what)).transpose()
}

impl AdvisorStore {
    /// Open (creating) the advisory DB at `advisor_path` and attach a read-only reader on the
    /// existing snapshot DB at `snapshot_path`.
    pub fn open(
        advisor_path: &Path,
        snapshot_path: &Path,
        cfg: QueueConfig,
    ) -> Result<Self, AdvisorError> {
        let reader = SnapshotReader::open(snapshot_path)?;
        let conn = Connection::open(advisor_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=10000;",
        )?;
        let s = Self {
            conn: Mutex::new(conn),
            reader,
            cfg,
        };
        s.migrate_up()?;
        Ok(s)
    }

    pub fn reader(&self) -> &SnapshotReader {
        &self.reader
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn migrate_up(&self) -> Result<(), AdvisorError> {
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
        if cur > ADVISOR_SCHEMA_VERSION {
            return Err(AdvisorError::SchemaTooNew {
                found: cur,
                supported: ADVISOR_SCHEMA_VERSION,
            });
        }
        if cur < 1 {
            tx.execute_batch(ADVISOR_MIGRATION_1_UP)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,name,applied_at)
                 VALUES(1,'advisor_outbox_decisions',strftime('%s','now'))",
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Roll the advisory schema back to `target` (0 = empty); destroys advisory data.
    pub fn rollback_to(&self, target: i64) -> Result<(), AdvisorError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if target < 1 {
            tx.execute_batch(ADVISOR_MIGRATION_1_DOWN)?;
            tx.execute("DELETE FROM schema_migrations WHERE version=1", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, AdvisorError> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?)
    }

    // ---- queue ----

    /// Copy new export rows (by id watermark) into the claimable queue, stopping at
    /// `max_pending` pending rows. Idempotent on `event_key`. Ids missing from the sequence were
    /// shed before this advisor saw them and are counted in `missed_total`.
    pub fn pull(&self, now_ns: u64, batch: u32) -> Result<PullOutcome, AdvisorError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last = meta_get(&tx, "last_synced_id")?;
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM advisor_outbox WHERE state='pending'",
            [],
            |r| r.get(0),
        )?;
        let room = self
            .cfg
            .max_pending
            .saturating_sub(u64_of(pending, "pending")?)
            .min(u64::from(batch));
        if room == 0 {
            return Ok(PullOutcome::default());
        }
        let rows = self
            .reader
            .export_after(last, u32::try_from(room).unwrap_or(u32::MAX))?;
        let now = i64_of(now_ns, "now_ns")?;
        let (mut prev, mut copied, mut missed) = (last, 0u64, 0u64);
        for r in &rows {
            missed += u64_of(r.id - prev - 1, "gap")?;
            prev = r.id;
            let n = tx.execute(
                "INSERT OR IGNORE INTO advisor_outbox(id,event_key,chain_id,pool,borrower,revision,
                   feature_digest,feature_json,enqueued_at_ns,available_at_ns)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    r.id,
                    r.event_key.as_slice(),
                    i64_of(r.key.chain_id, "chain_id")?,
                    r.key.pool.as_slice(),
                    r.key.borrower.as_slice(),
                    i64_of(r.revision, "revision")?,
                    r.feature_digest.as_slice(),
                    r.feature_json,
                    i64_of(r.enqueued_at_ns, "enqueued_at_ns")?,
                    now
                ],
            )?;
            copied += n as u64;
        }
        if prev != last {
            meta_set(&tx, "last_synced_id", prev)?;
            let m = meta_get(&tx, "missed_total")?;
            meta_set(&tx, "missed_total", m + i64_of(missed, "missed")?)?;
        }
        tx.commit()?;
        Ok(PullOutcome { copied, missed })
    }

    /// Lease up to `max` ready rows to `owner` for `lease_ns`. Exclusive: the whole claim runs in
    /// one `BEGIN IMMEDIATE` transaction and a row is claimable only while unleased or expired,
    /// so two claimers never receive the same row. Each claim counts as an attempt; an expired
    /// lease that already used `max_attempts` is dead-lettered instead of re-claimed. At most
    /// `max_in_flight` rows are leased at once.
    pub fn claim(
        &self,
        owner: &str,
        now_ns: u64,
        lease_ns: u64,
        max: usize,
    ) -> Result<Vec<Claimed>, AdvisorError> {
        let now = i64_of(now_ns, "now_ns")?;
        let until = i64_of(now_ns.saturating_add(lease_ns), "lease_until")?;
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE advisor_outbox SET state='dead', lease_owner=NULL, lease_until_ns=NULL,
               last_error='lease expired at max attempts'
             WHERE state='pending' AND lease_until_ns IS NOT NULL AND lease_until_ns<=?1
               AND attempts>=?2",
            params![now, self.cfg.max_attempts],
        )?;
        let in_flight: i64 = tx.query_row(
            "SELECT COUNT(*) FROM advisor_outbox WHERE state='pending' AND lease_until_ns>?1",
            [now],
            |r| r.get(0),
        )?;
        let room = self
            .cfg
            .max_in_flight
            .saturating_sub(u64_of(in_flight, "in_flight")?);
        let n = i64::try_from(u64::try_from(max).unwrap_or(u64::MAX).min(room)).unwrap_or(i64::MAX);
        if n == 0 {
            return Ok(vec![]);
        }
        let mut st = tx.prepare(
            "UPDATE advisor_outbox SET lease_owner=?1, lease_until_ns=?2, attempts=attempts+1
             WHERE id IN (SELECT id FROM advisor_outbox
                          WHERE state='pending' AND available_at_ns<=?3
                            AND (lease_until_ns IS NULL OR lease_until_ns<=?3)
                          ORDER BY available_at_ns, id LIMIT ?4)
             RETURNING id,event_key,chain_id,pool,borrower,revision,feature_digest,feature_json,
                       attempts,lease_until_ns",
        )?;
        let rows = st.query_map(params![owner, until, now, n], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Vec<u8>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, ek, ch, p, b, rev, fd, fj, att, lu) = row?;
            out.push(Claimed {
                id,
                event_key: b256_of(&ek)?,
                key: AccountKey {
                    chain_id: u64_of(ch, "chain_id")?,
                    pool: addr_of(&p)?,
                    borrower: addr_of(&b)?,
                },
                revision: u64_of(rev, "revision")?,
                feature_digest: b256_of(&fd)?,
                feature_json: fj,
                attempts: u32::try_from(att)
                    .map_err(|_| AdvisorError::Corrupt("attempts".into()))?,
                lease_until_ns: u64_of(lu, "lease_until")?,
            });
        }
        drop(st);
        tx.commit()?;
        out.sort_by_key(|c| c.id);
        Ok(out)
    }

    /// Settle a leased row as done. Fails with `LeaseLost` unless `owner` holds an unexpired lease
    /// on a still-pending row, so a row is acked at most once.
    pub fn ack(&self, owner: &str, id: i64, now_ns: u64) -> Result<(), AdvisorError> {
        let now = i64_of(now_ns, "now_ns")?;
        let n = self.conn().execute(
            "UPDATE advisor_outbox SET state='done', consumed_at_ns=?3,
               lease_owner=NULL, lease_until_ns=NULL
             WHERE id=?1 AND state='pending' AND lease_owner=?2 AND lease_until_ns>?3",
            params![id, owner, now],
        )?;
        if n == 1 {
            Ok(())
        } else {
            Err(AdvisorError::LeaseLost(id))
        }
    }

    /// Record a failed attempt: retry after capped exponential backoff, or dead-letter once
    /// `attempts >= max_attempts`.
    pub fn fail(
        &self,
        owner: &str,
        id: i64,
        now_ns: u64,
        error: &str,
    ) -> Result<FailOutcome, AdvisorError> {
        let now = i64_of(now_ns, "now_ns")?;
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempts = held_attempts(&tx, owner, id, now)?;
        let err: String = error.chars().take(256).collect();
        let out = if attempts >= self.cfg.max_attempts {
            tx.execute(
                "UPDATE advisor_outbox SET state='dead', lease_owner=NULL, lease_until_ns=NULL,
                   last_error=?2 WHERE id=?1",
                params![id, err],
            )?;
            FailOutcome::Dead
        } else {
            let at = now_ns.saturating_add(self.cfg.backoff_ns(attempts));
            tx.execute(
                "UPDATE advisor_outbox SET available_at_ns=?2, lease_owner=NULL,
                   lease_until_ns=NULL, last_error=?3 WHERE id=?1",
                params![id, i64_of(at, "available_at")?, err],
            )?;
            FailOutcome::Retry {
                available_at_ns: at,
            }
        };
        tx.commit()?;
        Ok(out)
    }

    /// Give a leased row back untouched (graceful shutdown): the attempt is refunded and the row
    /// is immediately claimable.
    pub fn release(&self, owner: &str, id: i64, now_ns: u64) -> Result<(), AdvisorError> {
        let now = i64_of(now_ns, "now_ns")?;
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        held_attempts(&tx, owner, id, now)?;
        tx.execute(
            "UPDATE advisor_outbox SET attempts=MAX(attempts-1,0), lease_owner=NULL,
               lease_until_ns=NULL WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn stats(&self, now_ns: u64) -> Result<QueueStats, AdvisorError> {
        let now = i64_of(now_ns, "now_ns")?;
        let c = self.conn();
        let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<u64, AdvisorError> {
            let n: i64 = c.query_row(sql, p, |r| r.get(0))?;
            Ok(u64_of(n, "count")?)
        };
        let in_flight = count(
            "SELECT COUNT(*) FROM advisor_outbox WHERE state='pending' AND lease_until_ns>?1",
            &[&now],
        )?;
        let pending = count(
            "SELECT COUNT(*) FROM advisor_outbox WHERE state='pending'",
            &[],
        )?;
        Ok(QueueStats {
            pending,
            in_flight,
            done: count(
                "SELECT COUNT(*) FROM advisor_outbox WHERE state='done'",
                &[],
            )?,
            dead: count(
                "SELECT COUNT(*) FROM advisor_outbox WHERE state='dead'",
                &[],
            )?,
            missed_total: u64_of(meta_get(&c, "missed_total")?, "missed_total")?,
        })
    }

    // ---- decisions ----

    /// Validate and store a decision (spec section 7 predicate pieces storable here).
    ///
    /// Checks, in order: shape (finite, in-range, `expires > issued`), not expired, configured
    /// policy/schema/chain/model, then against the snapshot DB (read-only): the decision's
    /// revision equals the CURRENT stored revision and its feature digest equals the current
    /// export. The write is a compare-and-swap on a per-account revision high-water mark inside
    /// the advisory transaction: a response for revision `r` cannot land after `r+1` was
    /// applied even if the snapshot read raced. A repeated key refreshes issuance/expiry/response.
    pub fn apply_decision(
        &self,
        d: &AdvisoryDecision,
        now_ns: u64,
        cfg: &DecisionConfig,
    ) -> Result<ApplyOutcome, AdvisorError> {
        validate_shape(d)?;
        if d.expires_at_ns <= now_ns {
            return Err(AdvisorError::Expired {
                expires_at_ns: d.expires_at_ns,
                now_ns,
            });
        }
        if d.issued_at_ns > now_ns {
            return Err(AdvisorError::IssuedInFuture);
        }
        if d.policy_version != cfg.policy_version {
            return Err(AdvisorError::PolicyVersionMismatch);
        }
        if d.feature_schema != cfg.feature_schema {
            return Err(AdvisorError::FeatureSchemaMismatch);
        }
        if d.key.chain_id != cfg.chain_id {
            return Err(AdvisorError::ChainMismatch);
        }
        if !cfg.allowed_models.iter().any(|m| m == &d.model) {
            return Err(AdvisorError::ModelNotAllowed);
        }
        let snap = self
            .reader
            .fetch_by_key(&d.key)?
            .ok_or(AdvisorError::UnknownAccount)?;
        if d.revision < snap.revision {
            return Err(AdvisorError::StaleRevision {
                decision: d.revision,
                current: snap.revision,
            });
        }
        if d.revision > snap.revision {
            return Err(AdvisorError::FutureRevision {
                decision: d.revision,
                current: snap.revision,
            });
        }
        match self.reader.current_export(&d.key)? {
            Some((rev, digest)) if rev == snap.revision => {
                if digest != d.feature_digest {
                    return Err(AdvisorError::DigestMismatch);
                }
            }
            _ => return Err(AdvisorError::NoCurrentExport),
        }
        self.commit_decision(d)
    }

    /// The advisory-side CAS and upsert (no snapshot read; callers validate first).
    fn commit_decision(&self, d: &AdvisoryDecision) -> Result<ApplyOutcome, AdvisorError> {
        let mut c = self.conn();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let k = &d.key;
        let (chain, rev) = (
            i64_of(k.chain_id, "chain_id")?,
            i64_of(d.revision, "revision")?,
        );
        let high: Option<i64> = tx
            .query_row(
                "SELECT revision FROM advisor_account_high WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
                params![chain, k.pool.as_slice(), k.borrower.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(h) = high
            && rev < h
        {
            return Err(AdvisorError::StaleRevision {
                decision: d.revision,
                current: u64_of(h, "revision")?,
            });
        }
        tx.execute(
            "INSERT INTO advisor_account_high(chain_id,pool,borrower,revision) VALUES(?1,?2,?3,?4)
             ON CONFLICT(chain_id,pool,borrower) DO UPDATE SET revision=MAX(revision,excluded.revision)",
            params![chain, k.pool.as_slice(), k.borrower.as_slice(), rev],
        )?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT issued_at_ns FROM advisory_decision
                 WHERE chain_id=?1 AND pool=?2 AND borrower=?3 AND revision=?4 AND feature_digest=?5
                   AND policy_version=?6 AND prompt_digest=?7 AND model=?8",
                params![
                    chain,
                    k.pool.as_slice(),
                    k.borrower.as_slice(),
                    rev,
                    d.feature_digest.as_slice(),
                    d.policy_version,
                    d.prompt_digest.as_slice(),
                    d.model
                ],
                |r| r.get(0),
            )
            .optional()?;
        let issued = i64_of(d.issued_at_ns, "issued_at_ns")?;
        if let Some(prev) = existing
            && issued < prev
        {
            return Err(AdvisorError::SupersededResponse);
        }
        tx.execute(
            "INSERT INTO advisory_decision(chain_id,pool,borrower,revision,feature_digest,
               feature_schema,policy_version,prompt_digest,model,watch_tier,confidence,
               prep_probability,value_score,issued_at_ns,expires_at_ns,input_tokens,output_tokens,
               response_digest,decision_mode)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
             ON CONFLICT(chain_id,pool,borrower,revision,feature_digest,policy_version,prompt_digest,model)
             DO UPDATE SET feature_schema=excluded.feature_schema, watch_tier=excluded.watch_tier,
               confidence=excluded.confidence, prep_probability=excluded.prep_probability,
               value_score=excluded.value_score, issued_at_ns=excluded.issued_at_ns,
               expires_at_ns=excluded.expires_at_ns, input_tokens=excluded.input_tokens,
               output_tokens=excluded.output_tokens, response_digest=excluded.response_digest,
               decision_mode=excluded.decision_mode",
            params![
                chain,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                rev,
                d.feature_digest.as_slice(),
                d.feature_schema,
                d.policy_version,
                d.prompt_digest.as_slice(),
                d.model,
                d.watch_tier.as_str(),
                d.confidence,
                d.prep_probability,
                d.value_score,
                issued,
                i64_of(d.expires_at_ns, "expires_at_ns")?,
                opt_i64(d.input_tokens, "input_tokens")?,
                opt_i64(d.output_tokens, "output_tokens")?,
                d.response_digest.as_slice(),
                d.mode.as_str(),
            ],
        )?;
        tx.commit()?;
        Ok(if existing.is_some() {
            ApplyOutcome::Refreshed
        } else {
            ApplyOutcome::Inserted
        })
    }

    /// The freshest stored decision usable for `snapshot` right now: same account, revision ==
    /// `snapshot.revision`, digest == the current export, configured policy/schema, unexpired,
    /// allowed model. Confidence thresholds, mode and scheduler tier stay with the policy (011).
    pub fn valid_decision_for(
        &self,
        snapshot: &BorrowerSnapshot,
        now_ns: u64,
        cfg: &DecisionConfig,
    ) -> Result<Option<AdvisoryDecision>, AdvisorError> {
        let k = &snapshot.key;
        if k.chain_id != cfg.chain_id {
            return Ok(None);
        }
        let digest = match self.reader.current_export(k)? {
            Some((rev, d)) if rev == snapshot.revision => d,
            _ => return Ok(None),
        };
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT feature_schema,policy_version,prompt_digest,model,watch_tier,confidence,
                    prep_probability,value_score,issued_at_ns,expires_at_ns,input_tokens,
                    output_tokens,response_digest,decision_mode
             FROM advisory_decision
             WHERE chain_id=?1 AND pool=?2 AND borrower=?3 AND revision=?4 AND feature_digest=?5
               AND policy_version=?6 AND feature_schema=?7 AND expires_at_ns>?8
             ORDER BY issued_at_ns DESC, id DESC",
        )?;
        let rows = st.query_map(
            params![
                i64_of(k.chain_id, "chain_id")?,
                k.pool.as_slice(),
                k.borrower.as_slice(),
                i64_of(snapshot.revision, "revision")?,
                digest.as_slice(),
                cfg.policy_version,
                cfg.feature_schema,
                i64_of(now_ns, "now_ns")?
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, f64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Vec<u8>>(12)?,
                    r.get::<_, String>(13)?,
                ))
            },
        )?;
        for row in rows {
            let (fs, pv, pd, model, tier, conf, prep, val, iss, exp, it, ot, rd, mode) = row?;
            if !cfg.allowed_models.iter().any(|m| m == &model) {
                continue;
            }
            return Ok(Some(AdvisoryDecision {
                key: *k,
                revision: snapshot.revision,
                feature_digest: digest,
                feature_schema: fs,
                policy_version: pv,
                prompt_digest: b256_of(&pd)?,
                model,
                watch_tier: WatchTier::parse(&tier)?,
                confidence: conf,
                prep_probability: prep,
                value_score: val,
                issued_at_ns: u64_of(iss, "issued_at")?,
                expires_at_ns: u64_of(exp, "expires_at")?,
                input_tokens: it.map(|v| u64_of(v, "input_tokens")).transpose()?,
                output_tokens: ot.map(|v| u64_of(v, "output_tokens")).transpose()?,
                response_digest: b256_of(&rd)?,
                mode: DecisionMode::parse(&mode)?,
            }));
        }
        Ok(None)
    }

    /// Number of stored decisions for an account (all revisions).
    pub fn decision_count(&self, key: &AccountKey) -> Result<u64, AdvisorError> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM advisory_decision WHERE chain_id=?1 AND pool=?2 AND borrower=?3",
            params![
                i64_of(key.chain_id, "chain_id")?,
                key.pool.as_slice(),
                key.borrower.as_slice()
            ],
            |r| r.get(0),
        )?;
        Ok(u64_of(n, "count")?)
    }

    // ---- policy outcomes ----

    pub fn record_policy_outcome(&self, o: &PolicyOutcome) -> Result<i64, AdvisorError> {
        let c = self.conn();
        c.execute(
            "INSERT INTO policy_outcome(run_id,policy_name,chain_id,pool,borrower,state_block,
               event_kind,scheduled_at_ns,observed_at_ns,check_completed_at_ns,eligible,simulatable,
               estimated_profit_base,rpc_calls,simulation_count,reason_codes)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                o.run_id,
                o.policy_name,
                i64_of(o.key.chain_id, "chain_id")?,
                o.key.pool.as_slice(),
                o.key.borrower.as_slice(),
                i64_of(o.state_block, "state_block")?,
                o.event_kind,
                opt_i64(o.scheduled_at_ns, "scheduled_at_ns")?,
                opt_i64(o.observed_at_ns, "observed_at_ns")?,
                opt_i64(o.check_completed_at_ns, "check_completed_at_ns")?,
                o.eligible,
                o.simulatable,
                o.estimated_profit_base
                    .map(|v| v.to_be_bytes::<32>().to_vec()),
                o.rpc_calls,
                o.simulation_count,
                o.reason_codes.join(","),
            ],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn policy_outcomes(&self, run_id: &str) -> Result<Vec<PolicyOutcome>, AdvisorError> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT run_id,policy_name,chain_id,pool,borrower,state_block,event_kind,
                    scheduled_at_ns,observed_at_ns,check_completed_at_ns,eligible,simulatable,
                    estimated_profit_base,rpc_calls,simulation_count,reason_codes
             FROM policy_outcome WHERE run_id=?1 ORDER BY id",
        )?;
        let rows = st.query_map([run_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<i64>>(9)?,
                r.get::<_, Option<bool>>(10)?,
                r.get::<_, Option<bool>>(11)?,
                r.get::<_, Option<Vec<u8>>>(12)?,
                r.get::<_, u32>(13)?,
                r.get::<_, u32>(14)?,
                r.get::<_, String>(15)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (run, pol, ch, p, b, blk, ev, s, ob, cc, el, si, profit, rpc, sims, reasons) = row?;
            let o = |v: Option<i64>| v.map(|x| u64_of(x, "ns")).transpose();
            out.push(PolicyOutcome {
                run_id: run,
                policy_name: pol,
                key: AccountKey {
                    chain_id: u64_of(ch, "chain_id")?,
                    pool: addr_of(&p)?,
                    borrower: addr_of(&b)?,
                },
                state_block: u64_of(blk, "state_block")?,
                event_kind: ev,
                scheduled_at_ns: o(s)?,
                observed_at_ns: o(ob)?,
                check_completed_at_ns: o(cc)?,
                eligible: el,
                simulatable: si,
                estimated_profit_base: profit.map(|v| crate::repo::u256_of(&v)).transpose()?,
                rpc_calls: rpc,
                simulation_count: sims,
                reason_codes: if reasons.is_empty() {
                    vec![]
                } else {
                    reasons.split(',').map(str::to_owned).collect()
                },
            });
        }
        Ok(out)
    }
}

fn validate_shape(d: &AdvisoryDecision) -> Result<(), AdvisorError> {
    let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
    if !unit(d.confidence) {
        return Err(AdvisorError::Malformed(
            "confidence must be finite in [0,1]",
        ));
    }
    if !unit(d.prep_probability) {
        return Err(AdvisorError::Malformed(
            "prep_probability must be finite in [0,1]",
        ));
    }
    if !(d.value_score.is_finite() && (0.0..=4.0).contains(&d.value_score)) {
        return Err(AdvisorError::Malformed(
            "value_score must be finite in [0,4]",
        ));
    }
    if d.expires_at_ns <= d.issued_at_ns {
        return Err(AdvisorError::Malformed(
            "expires_at must be after issued_at",
        ));
    }
    let name_ok = |s: &str| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !name_ok(&d.model) {
        return Err(AdvisorError::Malformed(
            "model must match [A-Za-z0-9._-]{1,64}",
        ));
    }
    if !name_ok(&d.policy_version) || !name_ok(&d.feature_schema) {
        return Err(AdvisorError::Malformed(
            "policy_version/feature_schema must be a short token",
        ));
    }
    for t in [d.input_tokens, d.output_tokens].into_iter().flatten() {
        i64_of(t, "tokens").map_err(|_| AdvisorError::Malformed("token count out of range"))?;
    }
    Ok(())
}

/// Attempts of a still-pending row currently leased (unexpired) by `owner`.
fn held_attempts(
    tx: &Transaction<'_>,
    owner: &str,
    id: i64,
    now: i64,
) -> Result<u32, AdvisorError> {
    let row: Option<i64> = tx
        .query_row(
            "SELECT attempts FROM advisor_outbox
             WHERE id=?1 AND state='pending' AND lease_owner=?2 AND lease_until_ns>?3",
            params![id, owner, now],
            |r| r.get(0),
        )
        .optional()?;
    let a = row.ok_or(AdvisorError::LeaseLost(id))?;
    u32::try_from(a).map_err(|_| AdvisorError::Corrupt("attempts".into()))
}

fn meta_get(c: &Connection, key: &str) -> Result<i64, rusqlite::Error> {
    Ok(
        c.query_row("SELECT value FROM advisor_meta WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or(0),
    )
}

fn meta_set(c: &Connection, key: &str, v: i64) -> Result<(), rusqlite::Error> {
    c.execute(
        "INSERT INTO advisor_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, v],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;

    /// Spec invariant 7: the advisor's snapshot handle is read-only at the SQLite level.
    #[test]
    fn snapshot_reader_cannot_write() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.db");
        let _repo = SnapshotRepository::open(&p).unwrap();
        let r = SnapshotReader::open(&p).unwrap();
        let c = r.conn();
        for sql in [
            "INSERT INTO advisor_shed(first_id,last_id,count,shed_at_ns) VALUES(1,1,1,1)",
            "DELETE FROM advisor_export",
            "UPDATE snapshots SET revision=revision+1",
            "CREATE TABLE evil(x)",
            "DROP TABLE advisor_current",
        ] {
            assert!(c.execute_batch(sql).is_err(), "write must be denied: {sql}");
        }
        assert!(
            c.execute_batch("PRAGMA query_only=OFF; DELETE FROM advisor_export")
                .is_err()
        );
    }

    /// The advisory-side CAS refuses revision r after r+1 was committed, independent of any
    /// snapshot read (the TOCTOU window between the read-only check and the advisory write).
    #[test]
    fn high_water_refuses_older_revision_after_newer_commit() {
        let d = tempfile::tempdir().unwrap();
        let snap_path = d.path().join("s.db");
        let _repo = SnapshotRepository::open(&snap_path).unwrap();
        let store = AdvisorStore::open(
            &d.path().join("a.db"),
            &snap_path,
            QueueConfig {
                max_pending: 10,
                max_in_flight: 10,
                max_attempts: 3,
                base_backoff_ns: 1,
                max_backoff_ns: 8,
            },
        )
        .unwrap();
        let mk = |rev| AdvisoryDecision {
            key: AccountKey {
                chain_id: 1,
                pool: Address::repeat_byte(1),
                borrower: Address::repeat_byte(2),
            },
            revision: rev,
            feature_digest: keccak256(b"f"),
            feature_schema: "s1".into(),
            policy_version: "p1".into(),
            prompt_digest: keccak256(b"p"),
            model: "m".into(),
            watch_tier: WatchTier::High,
            confidence: 0.9,
            prep_probability: 0.5,
            value_score: 1.0,
            issued_at_ns: 1,
            expires_at_ns: 100,
            input_tokens: None,
            output_tokens: None,
            response_digest: keccak256(b"r"),
            mode: DecisionMode::Shadow,
        };
        assert_eq!(
            store.commit_decision(&mk(5)).unwrap(),
            ApplyOutcome::Inserted
        );
        assert!(matches!(
            store.commit_decision(&mk(4)),
            Err(AdvisorError::StaleRevision {
                decision: 4,
                current: 5
            })
        ));
        assert_eq!(store.decision_count(&mk(4).key).unwrap(), 1);
    }
}
