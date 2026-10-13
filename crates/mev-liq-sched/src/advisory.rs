//! Advisory policy validation, shadow only (JEV-LIQ-011).
//!
//! `AdvisoryPolicy` wraps the baseline `schedule` (it never re-implements it): it computes the
//! baseline output first, validates each P2 job's optional Jev decision against the spec
//! section 7 `allow_advisory` predicate, and describes what an active ordering would have been.
//! In `Shadow` (and `Off`) the dispatched schedule is the baseline, byte for byte.
//!
//! Safety structure: the bonus can only reach the schedule through `schedule_with_advisory`,
//! whose core grants it to P2 jobs only, clamps it to `MAX_ADVISORY_BONUS`, withholds it from
//! aged jobs, and uses it solely to order jobs with equal `earliest_due` inside the
//! discretionary lane. P0/P1 selection, order, deadlines, backlog and coverage never see it.

use std::cell::RefCell;
use std::collections::BTreeMap;

use alloy::primitives::B256;
use mev_liq_state::{AccountKey, WatchTier};
use serde::Serialize;

use crate::{
    load::LoadedInputs,
    policy::{MAX_ADVISORY_BONUS, RankingPolicy, schedule, schedule_with_advisory},
    types::{
        OptionalDecision, PolicyConfig, RecheckJob, ScheduleError, ScheduleInput, ScheduleOutput,
        Tier,
    },
};

/// How advisory decisions are used. `Off` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum AdvisoryMode {
    #[default]
    Off,
    Shadow,
    /// Exists in the type only. Config parsing and `AdvisoryPolicy::new` reject it until the
    /// JEV-LIQ-019 activation guard adds a guarded path.
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisoryConfigError {
    /// `active` was requested from configuration.
    ActiveRequiresActivationGuard,
    UnknownMode(String),
    Invalid(&'static str),
}

impl std::fmt::Display for AdvisoryConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActiveRequiresActivationGuard => write!(
                f,
                "advisory mode `active` cannot be configured: it requires the JEV-LIQ-019 activation guard"
            ),
            Self::UnknownMode(m) => write!(f, "unknown advisory mode `{m}` (expected off|shadow)"),
            Self::Invalid(w) => write!(f, "invalid advisory config: {w}"),
        }
    }
}
impl std::error::Error for AdvisoryConfigError {}

impl AdvisoryMode {
    /// Parse a config value. `active` is a typed error, never a mode.
    pub fn parse_config(s: &str) -> Result<Self, AdvisoryConfigError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "shadow" => Ok(Self::Shadow),
            "active" => Err(AdvisoryConfigError::ActiveRequiresActivationGuard),
            _ => Err(AdvisoryConfigError::UnknownMode(s.to_string())),
        }
    }
}

impl std::str::FromStr for AdvisoryMode {
    type Err = AdvisoryConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_config(s)
    }
}

/// Health of the Jev path as seen by the 010 breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JevHealth {
    Healthy,
    /// Half-open (probing) or otherwise impaired.
    Degraded,
    Open,
}

impl JevHealth {
    /// Map 010's `copper_jev_circuit_state` gauge (0 closed, 1 half-open, 2 open/misconfigured).
    pub fn from_circuit_state(gauge: i64) -> Self {
        match gauge {
            0 => Self::Healthy,
            1 => Self::Degraded,
            _ => Self::Open,
        }
    }
}

/// Why a P2 job got no bonus. Every variant means `advisory_bonus = 0` and
/// `reason = BASELINE_FALLBACK`. `Ord` is only a stable presentation / tie order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum FallbackReason {
    NoDecision,
    JevDegraded,
    JevOpen,
    PolicyVersionMismatch,
    FeatureSchemaMismatch,
    RevisionMismatch,
    DigestMismatch,
    ChainMismatch,
    PoolMismatch,
    Expired,
    ModelNotAllowed,
    InvalidAnswer,
    BelowConfidence,
    /// Past the aging threshold: strict oldest-first, no bonus.
    AgedJob,
}

impl FallbackReason {
    /// Fixed metric label (`copper_jev_fallback_total{reason}`).
    pub fn label(self) -> &'static str {
        match self {
            Self::NoDecision => "no_decision",
            Self::JevDegraded => "jev_degraded",
            Self::JevOpen => "jev_open",
            Self::PolicyVersionMismatch => "policy_version_mismatch",
            Self::FeatureSchemaMismatch => "feature_schema_mismatch",
            Self::RevisionMismatch => "revision_mismatch",
            Self::DigestMismatch => "digest_mismatch",
            Self::ChainMismatch => "chain_mismatch",
            Self::PoolMismatch => "pool_mismatch",
            Self::Expired => "expired",
            Self::ModelNotAllowed => "model_not_allowed",
            Self::InvalidAnswer => "invalid_answer",
            Self::BelowConfidence => "below_confidence",
            Self::AgedJob => "aged_job",
        }
    }

    /// Decision describes an older or newer account state, or is past its TTL.
    pub fn is_stale(self) -> bool {
        matches!(
            self,
            Self::RevisionMismatch | Self::DigestMismatch | Self::Expired
        )
    }
}

/// The configured side of the spec section 7 predicate and the bonus bounds.
/// (Named `AdvisoryConfig`: `PolicyConfig` is the scheduler's own config.)
#[derive(Debug, Clone, PartialEq)]
pub struct AdvisoryConfig {
    pub policy_version: String,
    pub feature_schema: String,
    pub allowed_models: Vec<String>,
    /// `confidence >= threshold` for `high` to earn a bonus. Spec trial value 0.80.
    pub confidence_threshold: f64,
    /// Bonus ceiling, `0..=MAX_ADVISORY_BONUS`.
    pub max_bonus: u8,
}

impl AdvisoryConfig {
    pub fn new(
        policy_version: impl Into<String>,
        feature_schema: impl Into<String>,
        allowed_models: Vec<String>,
    ) -> Self {
        Self {
            policy_version: policy_version.into(),
            feature_schema: feature_schema.into(),
            allowed_models,
            confidence_threshold: 0.80,
            max_bonus: MAX_ADVISORY_BONUS,
        }
    }

    pub fn validate(&self) -> Result<(), AdvisoryConfigError> {
        let bad = AdvisoryConfigError::Invalid;
        if self.policy_version.is_empty() || self.feature_schema.is_empty() {
            return Err(bad("policy_version and feature_schema must be set"));
        }
        if self.allowed_models.is_empty() || self.allowed_models.iter().any(String::is_empty) {
            return Err(bad("allowed_models must be non-empty"));
        }
        if !(self.confidence_threshold.is_finite()
            && self.confidence_threshold > 0.0
            && self.confidence_threshold <= 1.0)
        {
            return Err(bad("confidence_threshold must be in (0, 1]"));
        }
        if self.max_bonus > MAX_ADVISORY_BONUS {
            return Err(bad("max_bonus above the hard cap of 20"));
        }
        Ok(())
    }
}

/// Spec 7 phase 1: positive-only. `high` at or above the threshold earns `1..=max_bonus`,
/// monotonic in confidence; every other tier earns 0 (never negative).
pub fn bonus_for(tier: WatchTier, confidence: f64, cfg: &AdvisoryConfig) -> u8 {
    if tier != WatchTier::High
        || cfg.max_bonus == 0
        || !confidence.is_finite()
        || confidence < cfg.confidence_threshold
    {
        return 0;
    }
    let c = (confidence * 1000.0).floor().clamp(0.0, 1000.0) as i64;
    let t = (cfg.confidence_threshold * 1000.0)
        .ceil()
        .clamp(1.0, 1000.0) as i64;
    let span = 1000 - t;
    let max = i64::from(cfg.max_bonus);
    let b = if span <= 0 {
        max
    } else {
        1 + ((c - t).max(0) * (max - 1)) / span
    };
    b.clamp(1, max) as u8
}

fn valid_answer(d: &OptionalDecision) -> bool {
    let unit = |x: f64| x.is_finite() && (0.0..=1.0).contains(&x);
    unit(d.confidence)
        && unit(d.prep_probability)
        && d.value_score.is_finite()
        && (0.0..=4.0).contains(&d.value_score)
}

/// Latest exported `(revision, feature_digest)` per account (008 `advisor_current`). A decision's
/// `feature_digest` is the digest of the exported feature payload, which differs from the
/// snapshot's own `feature_digest`, so the scheduler cannot derive it: the caller supplies it.
/// A missing entry fails closed (`DigestMismatch`).
pub type ExportView = BTreeMap<AccountKey, (u64, B256)>;

/// The `allow_advisory` predicate for one P2 job and one decision (mode and tier are decided by
/// the caller). Returns the bonus (0 for `normal`/`low`) or the first failing clause.
pub fn evaluate_decision(
    d: &OptionalDecision,
    job: &RecheckJob,
    export: Option<(u64, B256)>,
    now_ns: u64,
    aging_ns: u64,
    health: JevHealth,
    cfg: &AdvisoryConfig,
) -> Result<u8, FallbackReason> {
    match health {
        JevHealth::Healthy => {}
        JevHealth::Degraded => return Err(FallbackReason::JevDegraded),
        JevHealth::Open => return Err(FallbackReason::JevOpen),
    }
    let sv = &job.decision.source_version;
    if d.policy_version != cfg.policy_version {
        return Err(FallbackReason::PolicyVersionMismatch);
    }
    if d.feature_schema != cfg.feature_schema {
        return Err(FallbackReason::FeatureSchemaMismatch);
    }
    if d.key != job.key || d.revision != sv.snapshot_revision {
        return Err(FallbackReason::RevisionMismatch);
    }
    match export {
        Some((rev, _)) if rev != sv.snapshot_revision => {
            return Err(FallbackReason::RevisionMismatch);
        }
        Some((_, digest)) if digest == d.feature_digest => {}
        _ => return Err(FallbackReason::DigestMismatch),
    }
    if d.chain_id != job.key.chain_id {
        return Err(FallbackReason::ChainMismatch);
    }
    if d.pool != job.key.pool {
        return Err(FallbackReason::PoolMismatch);
    }
    if d.expires_at_ns <= now_ns {
        return Err(FallbackReason::Expired);
    }
    if !cfg.allowed_models.contains(&d.model) {
        return Err(FallbackReason::ModelNotAllowed);
    }
    if !valid_answer(d) {
        return Err(FallbackReason::InvalidAnswer);
    }
    if d.watch_tier == WatchTier::High && d.confidence < cfg.confidence_threshold {
        return Err(FallbackReason::BelowConfidence);
    }
    if now_ns.saturating_sub(job.decision.earliest_due) >= aging_ns {
        return Err(FallbackReason::AgedJob);
    }
    Ok(bonus_for(d.watch_tier, d.confidence, cfg))
}

fn tier_label(t: WatchTier) -> &'static str {
    match t {
        WatchTier::High => "high",
        WatchTier::Normal => "normal",
        WatchTier::Low => "low",
    }
}

/// Outcome of the advisory evaluation of one P2 job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub bonus: u8,
    pub fallback: Option<FallbackReason>,
    /// Watch tier of the decision that produced the bonus (`None` on fallback).
    pub applied_tier: Option<&'static str>,
}

fn judge(
    ds: &[&OptionalDecision],
    job: &RecheckJob,
    export: Option<(u64, B256)>,
    now_ns: u64,
    aging_ns: u64,
    health: JevHealth,
    cfg: &AdvisoryConfig,
) -> Verdict {
    if ds.is_empty() {
        return Verdict {
            bonus: 0,
            fallback: Some(FallbackReason::NoDecision),
            applied_tier: None,
        };
    }
    let mut best: Option<(u8, &'static str)> = None;
    let mut worst: Option<FallbackReason> = None;
    for d in ds {
        match evaluate_decision(d, job, export, now_ns, aging_ns, health, cfg) {
            Ok(b) => {
                if best.is_none_or(|(x, _)| b > x) {
                    best = Some((b, tier_label(d.watch_tier)));
                }
            }
            // Order-independent choice of the reported reason.
            Err(r) => worst = Some(worst.map_or(r, |w| w.min(r))),
        }
    }
    match best {
        Some((bonus, t)) => Verdict {
            bonus,
            fallback: None,
            applied_tier: Some(t),
        },
        None => Verdict {
            bonus: 0,
            fallback: worst,
            applied_tier: None,
        },
    }
}

/// The active ordering and the per-job verdicts behind it.
#[derive(Debug, Clone)]
pub struct WouldBe {
    pub output: ScheduleOutput,
    pub verdicts: BTreeMap<AccountKey, Verdict>,
}

/// Pure function (also the REPLAY_JEV arm): the schedule that an active advisory policy would
/// produce from `input`. Validates `optional_decisions` against the predicate with
/// `now_ns = input.now_ns`. Used for reporting only in this task.
pub fn would_be_schedule(
    input: &ScheduleInput<'_>,
    sched: &PolicyConfig,
    adv: &AdvisoryConfig,
    health: JevHealth,
    exports: &ExportView,
) -> Result<WouldBe, ScheduleError> {
    adv.validate().map_err(|e| match e {
        AdvisoryConfigError::Invalid(w) => ScheduleError::InvalidConfig(w),
        _ => ScheduleError::InvalidConfig("invalid advisory config"),
    })?;
    let mut by_key: BTreeMap<AccountKey, Vec<&OptionalDecision>> = BTreeMap::new();
    for d in input.optional_decisions {
        by_key.entry(d.key).or_default().push(d);
    }
    let verdicts: RefCell<BTreeMap<AccountKey, Verdict>> = RefCell::new(BTreeMap::new());
    let eval = |job: &RecheckJob| -> u8 {
        let ds = by_key.get(&job.key).map_or(&[][..], Vec::as_slice);
        let v = judge(
            ds,
            job,
            exports.get(&job.key).copied(),
            input.now_ns,
            sched.aging_ns,
            health,
            adv,
        );
        verdicts.borrow_mut().insert(job.key, v);
        v.bonus
    };
    let output = schedule_with_advisory(input, sched, &eval)?;
    Ok(WouldBe {
        output,
        verdicts: verdicts.into_inner(),
    })
}

/// Metric snapshot names mirror the Prometheus series (no borrower identifiers as labels).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AdvisoryCounters {
    pub copper_jev_fallback_total: BTreeMap<&'static str, u64>,
    pub copper_jev_stale_response_total: u64,
    pub copper_jev_applied_bonus_total: BTreeMap<&'static str, u64>,
}

/// What the Active ordering would have done. Recorded only; nothing in it is dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowAdvisoryReport {
    pub mode: AdvisoryMode,
    pub health: JevHealth,
    /// P2 jobs evaluated (dispatched, deferred, or otherwise).
    pub p2_jobs: u64,
    /// `(account, bonus)` for every P2 job whose would-be bonus is above 0.
    pub applied: Vec<(String, u8)>,
    pub counters: AdvisoryCounters,
    /// Fallback counts per reason label; same data as `counters`, kept for event readers.
    pub stale_decisions: u64,
    pub expired_decisions: u64,
    /// Dispatched P2 accounts under the baseline and under the would-be ordering.
    pub baseline_p2_order: Vec<String>,
    pub would_be_p2_order: Vec<String>,
    pub order_changed: bool,
    /// Sanity: P0/P1 dispatch, backlog, deadline misses and coverage identical to baseline.
    pub mandatory_unchanged: bool,
}

fn key_string(k: &AccountKey) -> String {
    format!("{}:{:#x}:{:#x}", k.chain_id, k.pool, k.borrower)
}

fn mandatory_view(o: &ScheduleOutput) -> impl PartialEq + use<> {
    let jobs: Vec<RecheckJob> = o
        .jobs
        .iter()
        .filter(|j| j.decision.tier <= Tier::P1)
        .cloned()
        .collect();
    let misses: Vec<_> = o
        .deadline_misses
        .iter()
        .filter(|m| m.tier <= Tier::P1)
        .cloned()
        .collect();
    (
        jobs,
        o.backlog.clone(),
        misses,
        o.coverage_violations.clone(),
        o.candidates.clone(),
        o.stale_rejected.clone(),
        o.free_slots,
        o.running,
        o.not_ready,
    )
}

fn p2_order(o: &ScheduleOutput) -> Vec<String> {
    o.jobs
        .iter()
        .filter(|j| j.decision.tier == Tier::P2)
        .map(|j| key_string(&j.key))
        .collect()
}

impl ShadowAdvisoryReport {
    pub fn build(
        mode: AdvisoryMode,
        health: JevHealth,
        baseline: &ScheduleOutput,
        wb: &WouldBe,
    ) -> Self {
        let mut counters = AdvisoryCounters::default();
        let (mut stale, mut expired) = (0u64, 0u64);
        let mut applied = Vec::new();
        for (k, v) in &wb.verdicts {
            if let Some(r) = v.fallback {
                *counters
                    .copper_jev_fallback_total
                    .entry(r.label())
                    .or_default() += 1;
                if r.is_stale() {
                    stale += 1;
                }
                if r == FallbackReason::Expired {
                    expired += 1;
                }
            }
            if v.bonus > 0 {
                applied.push((key_string(k), v.bonus));
                if let Some(t) = v.applied_tier {
                    *counters
                        .copper_jev_applied_bonus_total
                        .entry(t)
                        .or_default() += 1;
                }
            }
        }
        counters.copper_jev_stale_response_total = stale;
        let baseline_order = p2_order(baseline);
        let would_be = p2_order(&wb.output);
        Self {
            mode,
            health,
            p2_jobs: wb.verdicts.len() as u64,
            applied,
            counters,
            stale_decisions: stale,
            expired_decisions: expired,
            order_changed: baseline_order != would_be,
            baseline_p2_order: baseline_order,
            would_be_p2_order: would_be,
            mandatory_unchanged: mandatory_view(baseline) == mandatory_view(&wb.output),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisoryError {
    Config(AdvisoryConfigError),
    Schedule(ScheduleError),
}

impl std::fmt::Display for AdvisoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "{e}"),
            Self::Schedule(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for AdvisoryError {}

/// Dispatch schedule plus the shadow description.
#[derive(Debug, Clone)]
pub struct AdvisoryRun {
    /// What to dispatch: always the baseline schedule while `Off` or `Shadow`.
    pub dispatch: ScheduleOutput,
    /// `None` when `Off`.
    pub report: Option<ShadowAdvisoryReport>,
}

/// Baseline schedule plus advisory validation. See the module docs.
#[derive(Debug, Clone)]
pub struct AdvisoryPolicy {
    mode: AdvisoryMode,
    pub sched: PolicyConfig,
    pub advisory: AdvisoryConfig,
    pub health: JevHealth,
}

impl AdvisoryPolicy {
    /// `Active` is rejected here until the JEV-LIQ-019 activation guard exists.
    pub fn new(
        mode: AdvisoryMode,
        sched: PolicyConfig,
        advisory: AdvisoryConfig,
        health: JevHealth,
    ) -> Result<Self, AdvisoryError> {
        if mode == AdvisoryMode::Active {
            return Err(AdvisoryError::Config(
                AdvisoryConfigError::ActiveRequiresActivationGuard,
            ));
        }
        advisory.validate().map_err(AdvisoryError::Config)?;
        sched.validate().map_err(AdvisoryError::Schedule)?;
        Ok(Self {
            mode,
            sched,
            advisory,
            health,
        })
    }

    pub fn mode(&self) -> AdvisoryMode {
        self.mode
    }

    pub fn with_health(mut self, h: JevHealth) -> Self {
        self.health = h;
        self
    }

    pub fn run(
        &self,
        input: &ScheduleInput<'_>,
        exports: &ExportView,
    ) -> Result<AdvisoryRun, ScheduleError> {
        let baseline = schedule(input, &self.sched)?;
        if self.mode == AdvisoryMode::Off {
            return Ok(AdvisoryRun {
                dispatch: baseline,
                report: None,
            });
        }
        let wb = would_be_schedule(input, &self.sched, &self.advisory, self.health, exports)?;
        let report = ShadowAdvisoryReport::build(self.mode, self.health, &baseline, &wb);
        let dispatch = if self.mode == AdvisoryMode::Active {
            wb.output
        } else {
            baseline
        };
        Ok(AdvisoryRun {
            dispatch,
            report: Some(report),
        })
    }
}

impl RankingPolicy for AdvisoryPolicy {
    fn schedule(&self, input: &ScheduleInput<'_>) -> Result<ScheduleOutput, ScheduleError> {
        // Dispatch is the baseline in Off/Shadow, so the export view cannot affect it.
        self.run(input, &ExportView::new()).map(|r| r.dispatch)
    }
}

/// Advisory decisions plus the current export view for one scheduler call.
#[derive(Debug, Clone, Default)]
pub struct AdvisoryInputs {
    pub decisions: Vec<OptionalDecision>,
    pub exports: ExportView,
}

/// REPLAY_JEV arm (spec section 10): the active ordering for offline replay at equal budgets.
/// `decisions` supplies the advisory inputs for each scheduler call (the replay harness passes
/// none itself). Never used for live dispatch.
pub struct ReplayAdvisoryPolicy<F> {
    pub sched: PolicyConfig,
    pub advisory: AdvisoryConfig,
    pub health: JevHealth,
    pub decisions: F,
}

impl<F> RankingPolicy for ReplayAdvisoryPolicy<F>
where
    F: Fn(&ScheduleInput<'_>) -> AdvisoryInputs,
{
    fn schedule(&self, input: &ScheduleInput<'_>) -> Result<ScheduleOutput, ScheduleError> {
        let AdvisoryInputs { decisions, exports } = (self.decisions)(input);
        let with = ScheduleInput {
            now_ns: input.now_ns,
            accounts: input.accounts,
            dirty: input.dirty,
            urgent_events: input.urgent_events,
            pools: input.pools,
            in_flight: input.in_flight,
            optional_decisions: &decisions,
        };
        would_be_schedule(&with, &self.sched, &self.advisory, self.health, &exports)
            .map(|w| w.output)
    }
}

/// Shadow pass inputs: decisions are produced from the loaded repository state.
pub type DecisionSource<'a> = &'a dyn Fn(&LoadedInputs, u64) -> AdvisoryInputs;
