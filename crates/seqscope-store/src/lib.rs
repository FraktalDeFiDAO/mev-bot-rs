//! seqscope storage: decoded rows in SQLite (WAL, indexed), written by one thread in batches so the feed never waits.
//!
//! Chosen by benchmark (DECISIONS #113, `examples/bench.rs`): 1M rows, batch 1,000, four indexes: SQLite ingests
//! ~31k rows/s into 402 MB with 0.2-40 ms queries; redb ~55k rows/s into 1 GB and needs hand-built indexes; a JSONL
//! log ingests 537k rows/s but every query is a multi-second scan. The feed peaks near 1,000 transactions/s, so
//! SQLite has about 30x headroom even when storing everything, and the file is plain SQL for anyone's tools.
//!
//! - [`Writer`]: a bounded queue into a writer thread. [`Ingest::send`] never blocks; when the queue is full the row is
//!   dropped and counted ([`Stats::dropped`]) -- never silently.
//! - [`Reader`]: read-only connections (WAL lets them run while the writer writes): filtered queries and arbitrary
//!   read-only SQL.
//! - The schema is versioned (`PRAGMA user_version`); opening a database from a newer version is an error.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use seqscope_core::Row;
use serde::Serialize;

/// The schema version this build writes and reads.
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Other(String),
}

type Result<T> = std::result::Result<T, StoreError>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS rows(
  id INTEGER PRIMARY KEY,
  t_ms INTEGER NOT NULL,
  seq INTEGER NOT NULL,
  hash BLOB NOT NULL,
  from_addr BLOB,
  tx_to BLOB,
  to_addr BLOB,
  selector INTEGER,
  via TEXT NOT NULL,
  protocol TEXT,
  known TEXT,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  fields TEXT NOT NULL,
  profile TEXT NOT NULL,
  rule TEXT NOT NULL,
  tag TEXT,
  alert INTEGER NOT NULL,
  len INTEGER NOT NULL,
  input BLOB
);
CREATE INDEX IF NOT EXISTS rows_t ON rows(t_ms);
CREATE INDEX IF NOT EXISTS rows_selector ON rows(selector, t_ms);
CREATE INDEX IF NOT EXISTS rows_to ON rows(to_addr, t_ms);
CREATE INDEX IF NOT EXISTS rows_kind ON rows(protocol, kind, t_ms);
CREATE INDEX IF NOT EXISTS rows_rule ON rows(profile, rule, t_ms);
-- The same rows with addresses and hashes as 0x text, for people writing SQL by hand.
CREATE VIEW IF NOT EXISTS v_rows AS SELECT
  id, t_ms, seq, '0x'||lower(hex(hash)) AS hash, '0x'||lower(hex(from_addr)) AS from_addr,
  '0x'||lower(hex(tx_to)) AS tx_to, '0x'||lower(hex(to_addr)) AS to_addr,
  CASE WHEN selector IS NULL THEN NULL ELSE '0x'||printf('%08x', selector) END AS selector,
  via, protocol, known, kind, status, fields, profile, rule, tag, alert, len,
  CASE WHEN input IS NULL THEN NULL ELSE '0x'||lower(hex(input)) END AS input
FROM rows;
";

fn open_rw(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
    )?;
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(StoreError::Other(format!(
            "database schema version {v} is newer than this build ({SCHEMA_VERSION})"
        )));
    }
    c.execute_batch(SCHEMA)?;
    c.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(c)
}

fn hex_blob(s: &Option<String>) -> Option<Vec<u8>> {
    s.as_deref()
        .and_then(|s| alloy::hex_decode(s.trim_start_matches("0x")))
}

mod alloy {
    //! Tiny hex helpers (this crate has no other use for an Ethereum library).
    pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }
    pub fn hex_encode(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}

fn insert(c: &Connection, rows: &[Row]) -> Result<()> {
    let tx = c.unchecked_transaction()?;
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO rows(t_ms,seq,hash,from_addr,tx_to,to_addr,selector,via,protocol,known,kind,status,fields,profile,rule,tag,alert,len,input)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )?;
        for r in rows {
            let selector = r
                .selector
                .as_deref()
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .map(i64::from);
            st.execute(params![
                r.t_ms as i64,
                r.seq as i64,
                &r.hash[..],
                hex_blob(&r.from),
                hex_blob(&r.tx_to),
                hex_blob(&r.to),
                selector,
                r.via.join(","),
                r.protocol,
                r.known,
                r.kind,
                r.status,
                serde_json::to_string(&r.fields).unwrap_or_else(|_| "{}".into()),
                r.profile,
                r.rule,
                r.tag,
                r.alert,
                r.len as i64,
                hex_blob(&r.input),
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Counters of the writer, readable from any thread.
#[derive(Debug, Default)]
pub struct Stats {
    pub written: AtomicU64,
    /// Rows refused because the queue was full.
    pub dropped: AtomicU64,
    /// Rows lost because a batch failed to write (the error is in `last_error`).
    pub failed: AtomicU64,
    pub batches: AtomicU64,
    /// Slowest batch so far, in microseconds.
    pub max_batch_us: AtomicU64,
    pub last_error: Mutex<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    pub written: u64,
    pub dropped: u64,
    pub failed: u64,
    pub batches: u64,
    pub max_batch_us: u64,
    pub last_error: Option<String>,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            written: self.written.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            batches: self.batches.load(Ordering::Relaxed),
            max_batch_us: self.max_batch_us.load(Ordering::Relaxed),
            last_error: self.last_error.lock().ok().and_then(|e| e.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WriterConfig {
    /// Rows the queue holds before new ones are dropped.
    pub queue: usize,
    /// Rows per transaction.
    pub batch: usize,
    /// A partly filled batch is written after this long.
    pub flush: Duration,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            queue: 65_536,
            batch: 1_000,
            flush: Duration::from_millis(100),
        }
    }
}

/// The sending side; cheap to clone, usable from the feed thread.
#[derive(Clone)]
pub struct Ingest {
    tx: SyncSender<Row>,
    stats: Arc<Stats>,
    /// Set by [`Writer::shutdown`]: no more rows are accepted, whoever still holds a clone of this sender.
    closing: Arc<AtomicBool>,
}

impl Ingest {
    /// Queue a row without waiting. `false` means it was dropped (and counted).
    pub fn send(&self, row: Row) -> bool {
        if self.closing.load(Ordering::Acquire) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.tx.try_send(row) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn stats(&self) -> &Arc<Stats> {
        &self.stats
    }
}

/// The writer thread. `shutdown` writes what is queued and waits.
pub struct Writer {
    ingest: Option<Ingest>,
    join: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn(path: impl Into<PathBuf>, cfg: WriterConfig) -> Result<Writer> {
        let path = path.into();
        let conn = open_rw(&path)?;
        let (tx, rx) = mpsc::sync_channel::<Row>(cfg.queue.max(1));
        let stats = Arc::new(Stats::default());
        let closing = Arc::new(AtomicBool::new(false));
        let (st, stop) = (stats.clone(), closing.clone());
        let join = std::thread::Builder::new()
            .name("seqscope-writer".into())
            .spawn(move || {
                let mut batch: Vec<Row> = Vec::with_capacity(cfg.batch);
                let flush = |batch: &mut Vec<Row>| {
                    if batch.is_empty() {
                        return;
                    }
                    let t = std::time::Instant::now();
                    match insert(&conn, batch) {
                        Ok(()) => {
                            st.written.fetch_add(batch.len() as u64, Ordering::Relaxed);
                            st.batches.fetch_add(1, Ordering::Relaxed);
                            st.max_batch_us
                                .fetch_max(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                        }
                        Err(e) => {
                            st.failed.fetch_add(batch.len() as u64, Ordering::Relaxed);
                            if let Ok(mut l) = st.last_error.lock() {
                                *l = Some(e.to_string());
                            }
                        }
                    }
                    batch.clear();
                };
                loop {
                    if stop.load(Ordering::Acquire) {
                        // Shutting down: write what is already queued, whoever still holds a sender.
                        while let Ok(r) = rx.try_recv() {
                            batch.push(r);
                            if batch.len() >= cfg.batch {
                                flush(&mut batch);
                            }
                        }
                        flush(&mut batch);
                        break;
                    }
                    match rx.recv_timeout(cfg.flush) {
                        Ok(r) => {
                            batch.push(r);
                            // Drain what is already waiting, up to a full batch, without sleeping.
                            while batch.len() < cfg.batch {
                                match rx.try_recv() {
                                    Ok(r) => batch.push(r),
                                    Err(_) => break,
                                }
                            }
                            if batch.len() >= cfg.batch {
                                flush(&mut batch);
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => flush(&mut batch),
                        Err(RecvTimeoutError::Disconnected) => {
                            flush(&mut batch);
                            break;
                        }
                    }
                }
            })
            .map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(Writer {
            ingest: Some(Ingest { tx, stats, closing }),
            join: Some(join),
        })
    }

    pub fn ingest(&self) -> Ingest {
        self.ingest.clone().expect("writer is running")
    }

    /// Stop accepting rows (from every clone of the sender), write everything queued, and return the final counters.
    /// Does not wait for other holders of an [`Ingest`] to let go of it.
    pub fn shutdown(mut self) -> StatsSnapshot {
        let ingest = self.ingest.take().expect("writer is running");
        ingest.closing.store(true, Ordering::Release);
        let stats = ingest.stats.clone();
        drop(ingest);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        stats.snapshot()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(i) = &self.ingest {
            i.closing.store(true, Ordering::Release);
        }
        self.ingest = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Filters for [`Reader::query`]; every set field must match.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub protocol: Option<String>,
    pub kind: Option<String>,
    /// `0xa415bcad`.
    pub selector: Option<String>,
    /// Destination of the call (`0x` + 40 hex).
    pub to: Option<String>,
    pub profile: Option<String>,
    pub rule: Option<String>,
    pub tag: Option<String>,
    pub alert_only: bool,
    /// Newest first, at most this many (default 100).
    pub limit: Option<usize>,
}

/// A result of free-form SQL: column names and JSON-valued cells.
#[derive(Debug, Clone, Serialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// More rows existed than the cap.
    pub truncated: bool,
}

pub struct Reader {
    conn: Connection,
}

impl Reader {
    /// A read-only connection. The database must exist (a writer creates it).
    pub fn open(path: impl AsRef<Path>) -> Result<Reader> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA query_only=ON;")?;
        use rusqlite::limits::Limit;
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 1_048_576)?;
        conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 65_536)?;
        conn.set_limit(Limit::SQLITE_LIMIT_COLUMN, 128)?;
        conn.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, 64)?;
        conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        Ok(Reader { conn })
    }

    pub fn count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM rows", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn query(&self, q: &Query) -> Result<Vec<Row>> {
        let mut sql = String::from(
            "SELECT t_ms,seq,hash,from_addr,tx_to,to_addr,selector,via,protocol,known,kind,status,fields,profile,rule,tag,alert,len,input FROM rows WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut add = |clause: &str, v: rusqlite::types::Value| {
            sql.push_str(" AND ");
            sql.push_str(clause);
            args.push(v);
        };
        use rusqlite::types::Value as V;
        if let Some(t) = q.since_ms {
            add("t_ms >= ?", V::Integer(t as i64));
        }
        if let Some(t) = q.until_ms {
            add("t_ms <= ?", V::Integer(t as i64));
        }
        if let Some(p) = &q.protocol {
            add("protocol = ?", V::Text(p.clone()));
        }
        if let Some(k) = &q.kind {
            add("kind = ?", V::Text(k.clone()));
        }
        if let Some(s) = &q.selector {
            let n = u32::from_str_radix(s.trim_start_matches("0x"), 16)
                .map_err(|_| StoreError::Other(format!("bad selector '{s}'")))?;
            add("selector = ?", V::Integer(i64::from(n)));
        }
        if let Some(t) = &q.to {
            let b = alloy::hex_decode(t.trim_start_matches("0x"))
                .ok_or_else(|| StoreError::Other(format!("bad address '{t}'")))?;
            add("to_addr = ?", V::Blob(b));
        }
        if let Some(p) = &q.profile {
            add("profile = ?", V::Text(p.clone()));
        }
        if let Some(r) = &q.rule {
            add("rule = ?", V::Text(r.clone()));
        }
        if let Some(t) = &q.tag {
            add("tag = ?", V::Text(t.clone()));
        }
        if q.alert_only {
            sql.push_str(" AND alert = 1");
        }
        sql.push_str(&format!(
            " ORDER BY t_ms DESC, id DESC LIMIT {}",
            q.limit.unwrap_or(100)
        ));
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args), |r| {
            let blob_hex = |i: usize| -> rusqlite::Result<Option<String>> {
                Ok(r.get::<_, Option<Vec<u8>>>(i)?
                    .map(|b| format!("0x{}", alloy::hex_encode(&b))))
            };
            let hash: Vec<u8> = r.get(2)?;
            let via: String = r.get(7)?;
            let fields: String = r.get(12)?;
            Ok(Row {
                t_ms: r.get::<_, i64>(0)? as u64,
                seq: r.get::<_, i64>(1)? as u64,
                hash: seqscope_core::Row::hash_from(&hash),
                from: blob_hex(3)?,
                tx_to: blob_hex(4)?,
                to: blob_hex(5)?,
                selector: r.get::<_, Option<i64>>(6)?.map(|s| format!("0x{s:08x}")),
                via: if via.is_empty() {
                    Vec::new()
                } else {
                    via.split(',').map(str::to_string).collect()
                },
                protocol: r.get(8)?,
                known: r.get(9)?,
                kind: r.get(10)?,
                status: r.get(11)?,
                fields: serde_json::from_str::<BTreeMap<String, String>>(&fields)
                    .unwrap_or_default(),
                profile: r.get(13)?,
                rule: r.get(14)?,
                tag: r.get(15)?,
                alert: r.get::<_, i64>(16)? != 0,
                len: r.get::<_, i64>(17)? as usize,
                input: blob_hex(18)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Free-form SQL, read-only (the connection refuses writes). Use the `v_rows` view for text addresses.
    pub fn sql(&self, sql: &str, max_rows: usize) -> Result<SqlResult> {
        if sql.len() > 65_536 {
            return Err(StoreError::Other("SQL exceeds 64 KiB".into()));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut steps = 0usize;
        self.conn.progress_handler(
            1_000,
            Some(move || {
                steps += 1_000;
                steps >= 2_000_000 || std::time::Instant::now() >= deadline
            }),
        )?;
        struct ResetProgress<'a>(&'a Connection);
        impl Drop for ResetProgress<'_> {
            fn drop(&mut self) {
                let _ = self.0.progress_handler(0, None::<fn() -> bool>);
            }
        }
        let _reset = ResetProgress(&self.conn);
        let mut st = self.conn.prepare(sql)?;
        let columns: Vec<String> = st.column_names().iter().map(|s| (*s).to_string()).collect();
        let n = columns.len();
        let mut out = Vec::new();
        let mut truncated = false;
        let mut bytes = columns.iter().map(String::len).sum::<usize>();
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            if out.len() >= max_rows.min(10_000) {
                truncated = true;
                break;
            }
            let values: Vec<serde_json::Value> = (0..n)
                .map(|i| match r.get_ref(i) {
                    Ok(ValueRef::Null) | Err(_) => serde_json::Value::Null,
                    Ok(ValueRef::Integer(v)) => v.into(),
                    Ok(ValueRef::Real(v)) => v.into(),
                    Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned().into(),
                    Ok(ValueRef::Blob(b)) => format!("0x{}", alloy::hex_encode(b)).into(),
                })
                .collect();
            bytes += serde_json::to_vec(&values)
                .map_err(|e| StoreError::Other(e.to_string()))?
                .len();
            if bytes > 1_048_576 {
                return Err(StoreError::Other("SQL result exceeds 1 MiB".into()));
            }
            out.push(values);
        }
        Ok(SqlResult {
            columns,
            rows: out,
            truncated,
        })
    }
}

/// Delete rows older than `before_ms`; returns how many. Run from the writer's side, never while querying heavily.
pub fn prune(path: impl AsRef<Path>, before_ms: u64) -> Result<u64> {
    let c = open_rw(path.as_ref())?;
    let n = c.execute("DELETE FROM rows WHERE t_ms < ?", params![before_ms as i64])?;
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(n as u64)
}

/// Rows and file size, for the retention policy and the status screens.
pub fn disk_bytes(path: impl AsRef<Path>) -> u64 {
    let p = path.as_ref();
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    len(p) + len(&PathBuf::from(format!("{}-wal", p.display())))
}
