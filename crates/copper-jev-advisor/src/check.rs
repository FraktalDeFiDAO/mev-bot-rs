//! Readiness probe (`copper-jev-advisor check`). Proves that both databases are reachable with the
//! right access mode and schema. It deliberately never contacts the Jev API: an external outage
//! must not flip readiness (spec section 12).

use mev_liq_state::{ADVISOR_SCHEMA_VERSION, AdvisorStore, QueueConfig, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckReport {
    pub snapshot_schema: i64,
    pub advisor_schema: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("snapshot database: {0}")]
    Snapshot(String),
    #[error(
        "snapshot database schema is {found}, this build expects {expected}; \
         start Process A (or migrate) first"
    )]
    SnapshotSchema { found: i64, expected: i64 },
    #[error("advisor database: {0}")]
    Advisor(String),
    #[error("advisor database schema is {found}, this build expects {expected}")]
    AdvisorSchema { found: i64, expected: i64 },
}

/// Open the snapshot DB read-only and the advisor DB read-write, and compare schema versions.
pub fn check(cfg: &Config) -> Result<CheckReport, CheckError> {
    let conn = Connection::open_with_flags(
        &cfg.snapshot_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CheckError::Snapshot(e.to_string()))?;
    let found: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| CheckError::Snapshot(e.to_string()))?;
    drop(conn);
    if found != SCHEMA_VERSION {
        return Err(CheckError::SnapshotSchema {
            found,
            expected: SCHEMA_VERSION,
        });
    }

    let queue = QueueConfig {
        max_pending: cfg.max_pending,
        max_in_flight: cfg.resilience.max_inflight as u64,
        max_attempts: cfg.max_attempts,
        base_backoff_ns: 1,
        max_backoff_ns: 1,
    };
    // `open` opens the snapshot read-only again (refusing schema < 4), then opens/creates the
    // advisory file read-write and applies its own migrations.
    let store = AdvisorStore::open(&cfg.advisor_db, &cfg.snapshot_db, queue)
        .map_err(|e| CheckError::Advisor(e.to_string()))?;
    let advisor_schema = store
        .schema_version()
        .map_err(|e| CheckError::Advisor(e.to_string()))?;
    if advisor_schema != ADVISOR_SCHEMA_VERSION {
        return Err(CheckError::AdvisorSchema {
            found: advisor_schema,
            expected: ADVISOR_SCHEMA_VERSION,
        });
    }
    store
        .stats(0)
        .map_err(|e| CheckError::Advisor(e.to_string()))?;
    Ok(CheckReport {
        snapshot_schema: found,
        advisor_schema,
    })
}
