//! Canonical account snapshots for the liquidation pipeline (JEV-LIQ-003).
//!
//! Layering: a leaf crate (alloy primitives + rusqlite only). Protocol adapters such as
//! `mev-lending-aave-v3` depend on it to build snapshots; nothing here knows about RPC or
//! the engine. The existing f64 `Health` path is untouched and still drives live decisions.
//!
//! All quantities are exact `U256`; the only floating point is the model-supplied confidence,
//! probability and score fields of advisory decisions (`advisor`), which never feed eligibility.

mod advisor;
mod coalesce;
mod domain;
mod history;
mod index;
mod repo;
mod tiering;

pub use advisor::{
    ADVISOR_SCHEMA_VERSION, AdvisorError, AdvisorStore, AdvisoryDecision,
    ApplyOutcome as DecisionApply, Claimed, DecisionConfig, DecisionMode, EnqueueOutcome,
    ExportItem, ExportRow, FailOutcome, MAX_FEATURE_JSON_BYTES, OutboxConfig, PolicyOutcome,
    PullOutcome, QueueConfig, QueueStats, SnapshotReader, WatchTier,
    event_key as advisor_event_key,
};
pub use coalesce::Coalescer;
pub use domain::{
    AccountKey, BorrowerSnapshot, ChainPoint, ChainRef, ChainSource, DirtyReason, ReserveBalance,
    SnapshotStatus, WAD,
};
pub use history::{HISTORY_RETENTION, HistoryPoint};
pub use index::{
    ApplyOutcome, BorrowerRecord, CoverageGap, DirtyEntry, Effect, FLAG_BACKLOG_ESCALATED,
    FLAG_FULL_REVERIFICATION, KIND_COLLATERAL, KIND_DEBT, NewGap, PoolFlag, ReconcileOutcome,
    RewindOutcome, ScanBatch, ScanCursor, ScanDir,
};
pub use repo::{DirtyTarget, RepoError, SCHEMA_VERSION, SnapshotRepository};
pub use tiering::{DebtTiers, LowHealth, PoolCensus, StoreOutcome};
