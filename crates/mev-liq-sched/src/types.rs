//! Public vocabulary of the scheduler: tiers, reason codes, inputs and outputs.

use alloy::primitives::{Address, B256, U256};
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainPoint, CoverageGap, DirtyEntry, DirtyReason, PoolFlag,
};
use serde::Serialize;

/// `AccountKey` has no serde impl (it lives in a crate without serde); render it stably.
pub(crate) fn ser_key<S: serde::Serializer>(k: &AccountKey, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&format_args!(
        "{}:{:#x}:{:#x}",
        k.chain_id, k.pool, k.borrower
    ))
}

pub(crate) fn ser_reason<S: serde::Serializer>(r: &DirtyReason, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&format_args!("{r:?}"))
}

/// Fixed mandatory tiers (spec section 5). Derived `Ord`: `P0` is the most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Tier {
    P0,
    P1,
    P2,
    P3,
}

/// Why an account is scheduled. Derived `Ord` only fixes a stable presentation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ReasonCode {
    VerifiedEligible,
    AwaitingVerification,
    RiskEventPriceMove,
    RiskEventBorrower,
    RiskEventReserveConfig,
    UnknownState,
    NeverChecked,
    SnapshotInvalidated,
    ReorgResync,
    StaleMark,
    DigestMismatch,
    NewDebtPossible,
    MaxAgeExpired,
    MaxAgeExpiring,
    FullReverification,
    BacklogEscalated,
    UnreconciledDependencies,
    CoverageGap,
    DiscretionaryEvent,
    Background,
}

/// A direct risk event outside the durable dirty list (e.g. a speculative Fast Feed oracle move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrgentEvent {
    pub key: AccountKey,
    pub reason: DirtyReason,
    pub chain: Option<ChainPoint>,
    pub at_ns: u64,
}

/// What the caller knows about one tracked account.
#[derive(Debug, Clone)]
pub struct AccountState {
    pub key: AccountKey,
    pub snapshot: Option<BorrowerSnapshot>,
    /// Dependency set never reconciled or stale (`list_unreconciled`).
    pub unreconciled: bool,
    /// A previously actionable candidate waiting for fresh verification, since this time.
    pub awaiting_verification_since_ns: Option<u64>,
    /// Fairness cohort (reserve / size bucket); see `load::cohort_of`.
    pub cohort: u64,
}

impl AccountState {
    pub fn new(key: AccountKey, snapshot: Option<BorrowerSnapshot>) -> Self {
        Self {
            key,
            snapshot,
            unreconciled: false,
            awaiting_verification_since_ns: None,
            cohort: 0,
        }
    }
}

/// Pool-wide durable facts: flags and open coverage gaps.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub chain_id: u64,
    pub pool: Address,
    pub flags: Vec<PoolFlag>,
    pub gaps: Vec<CoverageGap>,
}

/// A job a worker currently holds. Unexpired leases are not re-dispatched (and never cancelled);
/// an expired lease puts the account back in the queue (restart / crashed worker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlight {
    pub key: AccountKey,
    pub lease_expires_ns: u64,
}

/// Advisory decision payload (the fields of 008's decision row). The mandatory `schedule` never
/// reads it; only `advisory::AdvisoryPolicy` validates it (spec section 7). Every field is an
/// untrusted claim: the requested bonus is not part of it, the policy derives the bonus.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionalDecision {
    /// Account the decision was filed under (lookup key).
    pub key: AccountKey,
    /// Chain and pool the decision claims to be about (checked against the current job).
    pub chain_id: u64,
    pub pool: Address,
    pub policy_version: String,
    pub feature_schema: String,
    /// Snapshot revision the decision was derived from.
    pub revision: u64,
    pub feature_digest: B256,
    pub expires_at_ns: u64,
    pub model: String,
    pub watch_tier: mev_liq_state::WatchTier,
    pub confidence: f64,
    pub prep_probability: f64,
    pub value_score: f64,
}

impl OptionalDecision {
    /// Build from a stored 008 decision row (chain and pool come from its account key).
    pub fn from_stored(d: &mev_liq_state::AdvisoryDecision) -> Self {
        Self {
            key: d.key,
            chain_id: d.key.chain_id,
            pool: d.key.pool,
            policy_version: d.policy_version.clone(),
            feature_schema: d.feature_schema.clone(),
            revision: d.revision,
            feature_digest: d.feature_digest,
            expires_at_ns: d.expires_at_ns,
            model: d.model.clone(),
            watch_tier: d.watch_tier,
            confidence: d.confidence,
            prep_probability: d.prep_probability,
            value_score: d.value_score,
        }
    }
}

pub struct ScheduleInput<'a> {
    pub now_ns: u64,
    pub accounts: &'a [AccountState],
    pub dirty: &'a [DirtyEntry],
    pub urgent_events: &'a [UrgentEvent],
    pub pools: &'a [PoolState],
    pub in_flight: &'a [InFlight],
    pub optional_decisions: &'a [OptionalDecision],
}

/// Versions the decision was derived from (spec section 7 `source_version`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceVersion {
    pub snapshot_revision: u64,
    pub snapshot_block: Option<u64>,
    pub feature_digest: Option<B256>,
    pub newest_event_block: Option<u64>,
    pub policy_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleDecision {
    pub tier: Tier,
    /// 0 unless `schedule_with_advisory` granted a bounded P2 tie-break bonus (never in `schedule`).
    pub advisory_bonus: u8,
    /// Deadline in the scheduler clock (ns): earliest required deadline of the mandatory reasons.
    pub earliest_due: u64,
    pub reason_codes: Vec<ReasonCode>,
    pub source_version: SourceVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecheckJob {
    #[serde(serialize_with = "ser_key")]
    pub key: AccountKey,
    pub decision: ScheduleDecision,
    pub cohort: u64,
    /// The job may not run before this time (a verified-eligible account was just rechecked).
    pub not_before_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeadlineMiss {
    pub tier: Tier,
    #[serde(serialize_with = "ser_key")]
    pub account: AccountKey,
    pub overdue_ns: u64,
    /// Whether the job was dispatched this round (false: sits in the backlog).
    pub dispatched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CoverageViolation {
    /// Even with every reserved P1 slot busy for a whole max-age window the tracked accounts
    /// cannot all be rechecked once. No coverage claim can be made for this budget.
    SweepCapacity {
        tracked_accounts: u64,
        sweep_capacity_per_window: u64,
        window_capacity: u64,
    },
    /// Mandatory (P0/P1) work is overdue and could not be dispatched this round.
    MandatoryBacklogOverdue {
        overdue_jobs: u64,
        max_overdue_ns: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleRejection {
    #[serde(serialize_with = "ser_key")]
    pub key: AccountKey,
    #[serde(serialize_with = "ser_reason")]
    pub reason: DirtyReason,
    pub event_block: u64,
    pub snapshot_block: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct TierCounts {
    pub p0: u64,
    pub p1: u64,
    pub p2: u64,
    pub p3: u64,
}

impl TierCounts {
    pub fn add(&mut self, t: Tier) {
        match t {
            Tier::P0 => self.p0 += 1,
            Tier::P1 => self.p1 += 1,
            Tier::P2 => self.p2 += 1,
            Tier::P3 => self.p3 += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleOutput {
    pub now_ns: u64,
    /// Dispatch order for this round (P0, then P1, then discretionary).
    pub jobs: Vec<RecheckJob>,
    /// Ready P0/P1 jobs that did not fit this round, most urgent first. Never discarded.
    pub backlog: Vec<RecheckJob>,
    /// Every job with a passed deadline (any tier), P0/P1 first.
    pub deadline_misses: Vec<DeadlineMiss>,
    pub coverage_violations: Vec<CoverageViolation>,
    pub stale_rejected: Vec<StaleRejection>,
    /// Jobs per tier among all candidates (dispatched, backlog, deferred).
    pub candidates: TierCounts,
    /// Ready P2/P3 jobs not dispatched this round (aging keeps them moving).
    pub deferred_discretionary: u64,
    /// Jobs whose `not_before_ns` is still in the future.
    pub not_ready: u64,
    /// Jobs held by an unexpired lease.
    pub running: u64,
    pub free_slots: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    InvalidConfig(&'static str),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(w) => write!(f, "invalid scheduler config: {w}"),
        }
    }
}
impl std::error::Error for ScheduleError {}

/// Spec section 5 starting hypotheses; shares are permille so the policy is integer-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConfig {
    pub policy_version: u32,
    pub max_account_age_ns: u64,
    /// P1 begins this long before the max-age deadline.
    pub expiring_window_ns: u64,
    /// Strictly below this HF (WAD) with debt > 0 an event is "near threshold".
    pub near_threshold_wad: U256,
    pub p0_deadline_ns: u64,
    pub p1_deadline_ns: u64,
    pub p2_deadline_ns: u64,
    /// Healthy covered accounts older than this become P3 background candidates.
    pub background_after_ns: u64,
    /// A verified-eligible account is rechecked at most this often.
    pub eligible_refresh_ns: u64,
    /// A discretionary job this far past its due time is "aged" and keeps a floor of slots.
    pub aging_ns: u64,
    pub concurrency: u32,
    pub p0_share_permille: u32,
    pub p1_share_permille: u32,
    pub p2_share_permille: u32,
    pub p2_floor_slots: u32,
    /// Average cost of one recheck on one slot (feasibility check).
    pub recheck_cost_ns: u64,
}

const SEC: u64 = 1_000_000_000;

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            policy_version: 1,
            max_account_age_ns: 300 * SEC,
            expiring_window_ns: 60 * SEC,
            near_threshold_wad: U256::from(1_050_000_000_000_000_000u64),
            p0_deadline_ns: 2 * SEC,
            p1_deadline_ns: 30 * SEC,
            p2_deadline_ns: 60 * SEC,
            background_after_ns: 150 * SEC,
            eligible_refresh_ns: 5 * SEC,
            aging_ns: 120 * SEC,
            concurrency: 16,
            p0_share_permille: 500,
            p1_share_permille: 300,
            p2_share_permille: 200,
            p2_floor_slots: 1,
            recheck_cost_ns: 20_000_000,
        }
    }
}

impl PolicyConfig {
    pub fn validate(&self) -> Result<(), ScheduleError> {
        let sum = u64::from(self.p0_share_permille)
            + u64::from(self.p1_share_permille)
            + u64::from(self.p2_share_permille);
        if sum != 1000 {
            return Err(ScheduleError::InvalidConfig("shares must sum to 1000"));
        }
        if self.concurrency == 0 {
            return Err(ScheduleError::InvalidConfig("concurrency must be > 0"));
        }
        if self.max_account_age_ns == 0 {
            return Err(ScheduleError::InvalidConfig(
                "max_account_age_ns must be > 0",
            ));
        }
        if self.recheck_cost_ns == 0 {
            return Err(ScheduleError::InvalidConfig("recheck_cost_ns must be > 0"));
        }
        Ok(())
    }
}
