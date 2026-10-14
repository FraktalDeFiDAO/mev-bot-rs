//! Shadow pass: build the schedule from the repository and render it as a record. It decides
//! nothing; task 006 compares these job lists with what the legacy runner actually rechecked.

use alloy::primitives::Address;
use mev_liq_state::{RepoError, SnapshotRepository};
use serde::Serialize;

use crate::{
    load::{LoadBounds, LoadedInputs, load_inputs, load_inputs_bounded},
    policy::{schedule, sweep_capacity},
    types::{
        CoverageViolation, DeadlineMiss, PolicyConfig, RecheckJob, ScheduleInput, ScheduleOutput,
        TierCounts,
    },
};

#[derive(Debug, Clone)]
pub struct ShadowParams {
    pub chain_id: u64,
    pub pool: Address,
    /// Name of the scan whose open coverage gaps count (`SnapshotRepository::open_gaps`).
    pub scan_name: String,
    pub policy: PolicyConfig,
    /// Jobs and misses embedded in the record (counts are always complete).
    pub max_records: usize,
    /// `Some`: read through `load_inputs_bounded` (large populations); `None`: every active
    /// borrower is loaded.
    pub bounds: Option<LoadBounds>,
}

#[derive(Debug)]
pub enum ShadowError {
    Repo(RepoError),
    Policy(crate::types::ScheduleError),
}

impl std::fmt::Display for ShadowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repo(e) => write!(f, "repository: {e}"),
            Self::Policy(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ShadowError {}

/// Counter set in the metrics event format (flat numeric fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowCounters {
    pub now_ns: u64,
    pub candidates: TierCounts,
    pub dispatched: u64,
    pub backlog_p0: u64,
    pub backlog_p1: u64,
    pub deadline_misses_p0: u64,
    pub deadline_misses_p1: u64,
    pub deadline_misses_other: u64,
    /// Of the misses above, those still undispatched (in the backlog) this round; the rest were
    /// dispatched late. Feeds `copper_liq_recheck_deadline_miss_total{reason}`.
    pub deadline_misses_backlog_p0: u64,
    pub deadline_misses_backlog_p1: u64,
    pub deadline_misses_backlog_other: u64,
    pub coverage_violations: u64,
    pub stale_rejected: u64,
    pub deferred_discretionary: u64,
    pub not_ready: u64,
    pub free_slots: u32,
    /// Accounts loaded into this pass.
    pub tracked: u64,
    /// Never-read borrowers left out of this pass by the load bounds (still counted for the
    /// sweep-capacity check).
    pub deferred_unknown: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShadowReport {
    pub counters: ShadowCounters,
    pub jobs: Vec<RecheckJob>,
    pub backlog: Vec<RecheckJob>,
    pub deadline_misses: Vec<DeadlineMiss>,
    pub coverage_violations: Vec<CoverageViolation>,
    pub truncated: bool,
}

impl ShadowReport {
    pub fn from_output(out: &ScheduleOutput, max_records: usize) -> Self {
        use crate::types::Tier;
        let count =
            |t: Tier, v: &[RecheckJob]| v.iter().filter(|j| j.decision.tier == t).count() as u64;
        let miss = |t: Tier| out.deadline_misses.iter().filter(|m| m.tier == t).count() as u64;
        let miss_backlog = |ts: &[Tier]| {
            out.deadline_misses
                .iter()
                .filter(|m| !m.dispatched && ts.contains(&m.tier))
                .count() as u64
        };
        let counters = ShadowCounters {
            now_ns: out.now_ns,
            candidates: out.candidates.clone(),
            dispatched: out.jobs.len() as u64,
            backlog_p0: count(Tier::P0, &out.backlog),
            backlog_p1: count(Tier::P1, &out.backlog),
            deadline_misses_p0: miss(Tier::P0),
            deadline_misses_p1: miss(Tier::P1),
            deadline_misses_other: miss(Tier::P2) + miss(Tier::P3),
            deadline_misses_backlog_p0: miss_backlog(&[Tier::P0]),
            deadline_misses_backlog_p1: miss_backlog(&[Tier::P1]),
            deadline_misses_backlog_other: miss_backlog(&[Tier::P2, Tier::P3]),
            coverage_violations: out.coverage_violations.len() as u64,
            stale_rejected: out.stale_rejected.len() as u64,
            deferred_discretionary: out.deferred_discretionary,
            not_ready: out.not_ready,
            free_slots: out.free_slots,
            tracked: 0,
            deferred_unknown: 0,
        };
        let truncated = out.jobs.len() > max_records
            || out.backlog.len() > max_records
            || out.deadline_misses.len() > max_records;
        Self {
            counters,
            jobs: out.jobs.iter().take(max_records).cloned().collect(),
            backlog: out.backlog.iter().take(max_records).cloned().collect(),
            deadline_misses: out
                .deadline_misses
                .iter()
                .take(max_records)
                .cloned()
                .collect(),
            coverage_violations: out.coverage_violations.clone(),
            truncated,
        }
    }

    /// True when an operator must be alerted: overdue mandatory work or an impossible budget.
    pub fn needs_alert(&self) -> bool {
        self.counters.deadline_misses_p0 > 0
            || self.counters.deadline_misses_p1 > 0
            || self.counters.coverage_violations > 0
    }
}

fn load(repo: &SnapshotRepository, p: &ShadowParams) -> Result<LoadedInputs, ShadowError> {
    match &p.bounds {
        None => load_inputs(repo, p.chain_id, p.pool, &p.scan_name),
        Some(b) => load_inputs_bounded(repo, p.chain_id, p.pool, &p.scan_name, b),
    }
    .map_err(ShadowError::Repo)
}

/// Record the pass size, and keep the sweep-capacity claim honest about the accounts the load
/// bounds deferred: they still need a sweep, so they count toward `tracked_accounts`.
fn account_for_load(report: &mut ShadowReport, loaded: &LoadedInputs, cfg: &PolicyConfig) {
    let tracked = loaded.accounts.len() as u64;
    report.counters.tracked = tracked;
    report.counters.deferred_unknown = loaded.deferred_unknown;
    if loaded.deferred_unknown == 0 {
        return;
    }
    let total = tracked.saturating_add(loaded.deferred_unknown);
    let existing = report.coverage_violations.iter_mut().find_map(|v| match v {
        CoverageViolation::SweepCapacity {
            tracked_accounts, ..
        } => Some(tracked_accounts),
        _ => None,
    });
    match existing {
        Some(t) => *t = (*t).max(total),
        None => {
            let (window_capacity, sweep_capacity_per_window) = sweep_capacity(cfg);
            if total > sweep_capacity_per_window {
                report
                    .coverage_violations
                    .push(CoverageViolation::SweepCapacity {
                        tracked_accounts: total,
                        sweep_capacity_per_window,
                        window_capacity,
                    });
            }
        }
    }
    report.counters.coverage_violations = report.coverage_violations.len() as u64;
}

/// One shadow pass at wall-clock `now_ns` (supplied by the caller; this crate never reads time).
pub fn run_shadow_pass(
    repo: &SnapshotRepository,
    p: &ShadowParams,
    now_ns: u64,
) -> Result<ShadowReport, ShadowError> {
    let loaded = load(repo, p)?;
    let input = ScheduleInput {
        now_ns,
        accounts: &loaded.accounts,
        dirty: &loaded.dirty,
        urgent_events: &[],
        pools: &loaded.pools,
        in_flight: &[],
        optional_decisions: &[],
    };
    let out = schedule(&input, &p.policy).map_err(ShadowError::Policy)?;
    let mut report = ShadowReport::from_output(&out, p.max_records);
    account_for_load(&mut report, &loaded, &p.policy);
    Ok(report)
}

/// One shadow pass with the advisory policy. The returned `ShadowReport` is the dispatched
/// schedule, which in `Off`/`Shadow` equals `run_shadow_pass` output exactly; the second value
/// describes what the active ordering would have been (`None` when `Off`). `decisions` builds
/// the validated-at-read advisory decisions from the loaded state (e.g. 008's
/// `valid_decision_for`); the policy re-checks every clause.
pub fn run_shadow_pass_advisory(
    repo: &SnapshotRepository,
    p: &ShadowParams,
    now_ns: u64,
    policy: &crate::advisory::AdvisoryPolicy,
    decisions: crate::advisory::DecisionSource<'_>,
) -> Result<(ShadowReport, Option<crate::advisory::ShadowAdvisoryReport>), ShadowError> {
    let loaded = load(repo, p)?;
    let crate::advisory::AdvisoryInputs {
        decisions: dec,
        exports,
    } = decisions(&loaded, now_ns);
    let input = ScheduleInput {
        now_ns,
        accounts: &loaded.accounts,
        dirty: &loaded.dirty,
        urgent_events: &[],
        pools: &loaded.pools,
        in_flight: &[],
        optional_decisions: &dec,
    };
    // The scheduler config is `p.policy`, exactly as in `run_shadow_pass`.
    let mut policy = policy.clone();
    policy.sched = p.policy.clone();
    let run = policy.run(&input, &exports).map_err(ShadowError::Policy)?;
    let mut report = ShadowReport::from_output(&run.dispatch, p.max_records);
    account_for_load(&mut report, &loaded, &p.policy);
    Ok((report, run.report))
}
