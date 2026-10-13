use std::collections::{BTreeMap, BTreeSet};

use alloy::primitives::{Address, B256, U256, keccak256};
use mev_liq_sched::{
    AccountState, BaselinePolicy, InFlight, PolicyConfig, PoolState, RankingPolicy, RecheckJob,
    ScheduleInput, Tier, TierCounts, UrgentEvent, cohort_of,
};
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainPoint, ChainRef, ChainSource, CoverageGap, DirtyReason,
    ScanDir, SnapshotStatus,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::event::{Body, Event, GroundTruth, ReplayStream, TruthPoint};

#[derive(Debug, Clone)]
pub struct ReplayConfig {
    pub policy: PolicyConfig,
    pub tick_ns: u64,
    pub max_steps: u64,
    pub max_checks: usize,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            policy: PolicyConfig::default(),
            tick_ns: 100_000_000,
            max_steps: 1_000_000,
            max_checks: 1_000_000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("invalid replay: {0}")]
    Invalid(String),
    #[error(transparent)]
    Schedule(#[from] mev_liq_sched::ScheduleError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckRecord {
    pub borrower: Address,
    pub started_ns: u64,
    pub completed_ns: u64,
    pub state_block: u64,
    pub tier: Tier,
    pub earliest_due_ns: u64,
    pub snapshot_revision: u64,
    pub state_available: bool,
    pub eligible: bool,
    /// A reorg superseded the pinned read before completion; this consumes work, never state.
    pub invalidated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Latency {
    pub samples: u64,
    pub p50_ns: Option<u64>,
    pub p95_ns: Option<u64>,
    pub p99_ns: Option<u64>,
}

impl Latency {
    fn from_samples(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        let percentile = |p: usize| {
            (!samples.is_empty()).then(|| samples[(samples.len() * p).div_ceil(100) - 1])
        };
        Self {
            samples: samples.len() as u64,
            p50_ns: percentile(50),
            p95_ns: percentile(95),
            p99_ns: percentile(99),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplayReport {
    pub report_version: u32,
    pub policy: String,
    pub code_revision: String,
    pub dataset_id: String,
    pub synthetic: bool,
    pub provenance: String,
    pub scenario: String,
    #[serde(rename = "stream_hash")]
    pub stream_digest: B256,
    pub source_block_start: u64,
    pub source_block_end: u64,
    pub duration_ns: u64,
    pub measure_from_ns: u64,
    pub seed: Option<u64>,
    pub policy_config: Value,
    pub tick_ns: u64,
    pub scheduler_calls: u64,
    pub dispatched: TierCounts,
    pub completed_checks: u64,
    pub unavailable_state_checks: u64,
    pub invalidated_checks: u64,
    pub pending_checks: u64,
    /// Modeled one block-pinned account read per dispatched check, not measured RPC usage.
    pub modeled_account_reads: u64,
    pub modeled_busy_ns: u64,
    pub peak_mandatory_backlog: u64,
    pub mandatory_deadline_miss_observations: u64,
    pub unique_mandatory_deadline_misses: u64,
    pub coverage_violation_observations: u64,
    pub reorgs: u64,
    pub eligible_episodes: u64,
    pub invalidated_label_episodes: u64,
    pub detected_episodes: u64,
    pub actionable_label_episodes: u64,
    pub detected_actionable_label_episodes: u64,
    /// Synthetic/recorded labels only. No REVM economics, competitor outcomes or profit estimates.
    pub eligibility_detection_latency: Latency,
    pub ordered_checks: Vec<CheckRecord>,
}

struct Running {
    job: RecheckJob,
    started: u64,
    completed: u64,
    chain: ChainPoint,
    truth: Option<TruthPoint>,
    epoch: u64,
}

struct World {
    accounts: BTreeMap<Address, AccountState>,
    dependencies: BTreeMap<Address, Address>,
    urgent: BTreeMap<(AccountKey, DirtyReason), UrgentEvent>,
    gaps: Vec<CoverageGap>,
    running: Vec<Running>,
    block: u64,
    epoch: u64,
    reorgs: u64,
}

impl World {
    fn mark(&mut self, key: AccountKey, reason: DirtyReason, now: u64) {
        let event = UrgentEvent {
            key,
            reason,
            chain: Some(point(self.block, self.epoch)),
            at_ns: now,
        };
        self.urgent
            .entry((key, reason))
            .and_modify(|old| {
                old.chain = event.chain;
                old.at_ns = old.at_ns.min(now);
            })
            .or_insert(event);
    }

    fn apply(&mut self, ev: &Event, chain_id: u64, pool: Address) {
        self.block = ev.block;
        match ev.body {
            Body::Register { borrower } => {
                let key = AccountKey {
                    chain_id,
                    pool,
                    borrower,
                };
                self.accounts
                    .entry(borrower)
                    .or_insert_with(|| AccountState::new(key, None));
                self.mark(key, DirtyReason::Discovered, ev.t_ns);
            }
            Body::BorrowerEvent { borrower } => {
                if let Some(a) = self.accounts.get(&borrower) {
                    self.mark(a.key, DirtyReason::BorrowerEvent, ev.t_ns);
                }
            }
            Body::OracleMove { asset, .. } | Body::ReserveChanged { asset } => {
                let reason = if matches!(ev.body, Body::OracleMove { .. }) {
                    DirtyReason::PriceMove
                } else {
                    DirtyReason::ReserveConfig
                };
                // Unknown dependencies cannot safely be excluded from an affected-reserve sweep.
                let affected: Vec<_> = self
                    .accounts
                    .values()
                    .filter(|a| {
                        self.dependencies
                            .get(&a.key.borrower)
                            .is_none_or(|dep| *dep == asset)
                    })
                    .map(|a| a.key)
                    .collect();
                for key in affected {
                    self.mark(key, reason, ev.t_ns);
                }
            }
            Body::Reorg { ancestor_block } => {
                self.block = ancestor_block;
                self.epoch += 1;
                self.reorgs += 1;
                self.dependencies.clear();
                let keys: Vec<_> = self
                    .accounts
                    .values_mut()
                    .map(|a| {
                        if let Some(s) = &mut a.snapshot {
                            s.status = SnapshotStatus::Invalidated;
                        }
                        a.key
                    })
                    .collect();
                for key in keys {
                    self.mark(key, DirtyReason::Reorg, ev.t_ns);
                }
            }
            Body::Gap {
                from_block,
                to_block,
            } => self.gaps.push(CoverageGap {
                id: ev.seq as i64,
                name: "replay".into(),
                dir: ScanDir::Forward,
                from_block,
                to_block,
                reason: "recorded log gap".into(),
                recorded_ns: ev.t_ns,
                attempts: 0,
            }),
            Body::GapClosed {
                from_block,
                to_block,
            } => {
                self.gaps
                    .retain(|g| (g.from_block, g.to_block) != (from_block, to_block));
            }
            // Neither unobserved chain values nor labels are ranking inputs.
            Body::State { .. } | Body::Eligible { .. } => {}
        }
    }

    fn complete(&mut self, now: u64, out: &mut Vec<CheckRecord>) {
        let mut pending = Vec::new();
        for r in std::mem::take(&mut self.running) {
            if r.completed > now {
                pending.push(r);
                continue;
            }
            let invalidated = r.epoch != self.epoch;
            let eligible = !invalidated && r.truth.as_ref().is_some_and(TruthPoint::eligible);
            let mut revision = r.job.decision.source_version.snapshot_revision;
            if !invalidated && let Some(p) = &r.truth {
                let a = self
                    .accounts
                    .get_mut(&r.job.key.borrower)
                    .expect("registered job");
                revision = a.snapshot.as_ref().map_or(1, |s| s.revision + 1);
                let s = BorrowerSnapshot {
                    key: a.key,
                    revision,
                    chain: ChainRef {
                        block_number: r.chain.block_number,
                        block_hash: r.chain.block_hash,
                        source: ChainSource::Canonical,
                        observed_at_ns: r.started,
                    },
                    health_factor_wad: p.health_factor_wad,
                    total_debt_base: p.total_debt_base,
                    total_collateral_base: p.total_collateral_base,
                    available_borrows_base: U256::ZERO,
                    liquidation_threshold: U256::ZERO,
                    ltv: U256::ZERO,
                    reserves: vec![],
                    risk_metadata_version: 0,
                    dirty_reasons: vec![],
                    // Age starts at the pinned read, never at completion of an old read.
                    last_successful_check_ns: Some(r.started),
                    status: SnapshotStatus::Valid,
                    provenance: "offline recorded account data; no reserve token balances modeled"
                        .into(),
                    feature_digest: B256::ZERO,
                }
                .seal();
                a.cohort = cohort_of(&s);
                a.snapshot = Some(s);
                if let Some(asset) = p.reserve_asset {
                    self.dependencies.insert(a.key.borrower, asset);
                } else {
                    self.dependencies.remove(&a.key.borrower);
                }
                self.urgent.retain(|_, u| {
                    u.key != r.job.key
                        || u.at_ns > r.started
                        || u.chain
                            .is_some_and(|c| c.block_number > r.chain.block_number)
                });
            }
            out.push(CheckRecord {
                borrower: r.job.key.borrower,
                started_ns: r.started,
                completed_ns: r.completed,
                state_block: r.chain.block_number,
                tier: r.job.decision.tier,
                earliest_due_ns: r.job.decision.earliest_due,
                snapshot_revision: revision,
                state_available: r.truth.is_some(),
                eligible,
                invalidated,
            });
        }
        self.running = pending;
    }
}

fn point(block: u64, epoch: u64) -> ChainPoint {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&block.to_be_bytes());
    b[8..].copy_from_slice(&epoch.to_be_bytes());
    ChainPoint {
        block_number: block,
        block_hash: keccak256(b),
    }
}

/// Run the exact mandatory baseline against an event stream and a fixed service-time model.
/// Scheduling occurs on clock ticks, observed event times and completions; truth-only events
/// never add a scheduling opportunity. Ground truth is sampled when a check starts, not finishes.
pub fn replay(
    stream: &ReplayStream,
    cfg: &ReplayConfig,
    code_revision: &str,
) -> Result<ReplayReport, ReplayError> {
    let policy = BaselinePolicy {
        config: cfg.policy.clone(),
    };
    replay_with_policy(stream, cfg, code_revision, &policy, "BASELINE")
}

/// Same replay with any ranking policy at the same event stream, service model and budgets
/// (`cfg`). `label` is recorded as `ReplayReport::policy` (e.g. `REPLAY_JEV`). `cfg.policy`
/// must be the scheduler config the policy itself uses so the budgets are equal.
pub fn replay_with_policy(
    stream: &ReplayStream,
    cfg: &ReplayConfig,
    code_revision: &str,
    policy: &dyn RankingPolicy,
    label: &str,
) -> Result<ReplayReport, ReplayError> {
    cfg.policy.validate()?;
    if cfg.tick_ns == 0 || cfg.max_steps == 0 || cfg.max_checks == 0 {
        return Err(ReplayError::Invalid(
            "tick and resource limits must be positive".into(),
        ));
    }
    // ReplayStream is public: validate even for callers that did not use the strict JSONL loader.
    let stream = crate::event::validate(
        stream.header.clone(),
        stream
            .events
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, e)| (i + 2, e))
            .collect(),
    )?;
    let truth = GroundTruth::build(&stream.events).map_err(|(_, e)| ReplayError::Invalid(e))?;
    let mut world = World {
        accounts: BTreeMap::new(),
        dependencies: BTreeMap::new(),
        urgent: BTreeMap::new(),
        gaps: vec![],
        running: vec![],
        block: stream.header.source_block_start,
        epoch: 0,
        reorgs: 0,
    };
    let mut records = Vec::new();
    let mut report = ReplayReport {
        report_version: 1,
        policy: label.into(),
        code_revision: code_revision.into(),
        dataset_id: stream.header.dataset_id.clone(),
        synthetic: stream.header.synthetic,
        provenance: stream.header.provenance.clone(),
        scenario: stream.header.scenario.clone(),
        stream_digest: stream.digest,
        source_block_start: stream.header.source_block_start,
        source_block_end: stream.header.source_block_end,
        duration_ns: stream.header.duration_ns,
        measure_from_ns: stream.header.measure_from_ns,
        seed: stream.header.seed,
        policy_config: config_json(&cfg.policy),
        tick_ns: cfg.tick_ns,
        scheduler_calls: 0,
        dispatched: TierCounts::default(),
        completed_checks: 0,
        unavailable_state_checks: 0,
        invalidated_checks: 0,
        pending_checks: 0,
        modeled_account_reads: 0,
        modeled_busy_ns: 0,
        peak_mandatory_backlog: 0,
        mandatory_deadline_miss_observations: 0,
        unique_mandatory_deadline_misses: 0,
        coverage_violation_observations: 0,
        reorgs: 0,
        eligible_episodes: 0,
        invalidated_label_episodes: 0,
        detected_episodes: 0,
        actionable_label_episodes: 0,
        detected_actionable_label_episodes: 0,
        eligibility_detection_latency: Latency::default(),
        ordered_checks: vec![],
    };
    let mut missed = BTreeSet::new();
    let mut index = 0;
    let mut now = 0;
    let mut next_tick = 0;
    let observable_times: Vec<_> = stream
        .events
        .iter()
        .filter(|e| !matches!(e.body, Body::State { .. } | Body::Eligible { .. }))
        .map(|e| e.t_ns)
        .collect();
    loop {
        while index < stream.events.len() && stream.events[index].t_ns <= now {
            world.apply(
                &stream.events[index],
                stream.header.chain_id,
                stream.header.pool,
            );
            index += 1;
        }
        world.complete(now, &mut records);
        report.scheduler_calls += 1;
        if report.scheduler_calls > cfg.max_steps {
            return Err(ReplayError::Invalid("scheduler step limit exceeded".into()));
        }
        let accounts: Vec<_> = world.accounts.values().cloned().collect();
        let urgent: Vec<_> = world.urgent.values().copied().collect();
        let in_flight: Vec<_> = world
            .running
            .iter()
            .map(|r| InFlight {
                key: r.job.key,
                lease_expires_ns: r.completed,
            })
            .collect();
        let pools = [PoolState {
            chain_id: stream.header.chain_id,
            pool: stream.header.pool,
            flags: vec![],
            gaps: world.gaps.clone(),
        }];
        let output = policy.schedule(&ScheduleInput {
            now_ns: now,
            accounts: &accounts,
            dirty: &[],
            urgent_events: &urgent,
            pools: &pools,
            in_flight: &in_flight,
            optional_decisions: &[],
        })?;
        report.peak_mandatory_backlog = report
            .peak_mandatory_backlog
            .max(output.backlog.len() as u64);
        report.coverage_violation_observations += output.coverage_violations.len() as u64;
        for miss in output.deadline_misses.iter().filter(|m| m.tier <= Tier::P1) {
            report.mandatory_deadline_miss_observations += 1;
            missed.insert((miss.account, miss.tier, now - miss.overdue_ns));
        }
        // No new work at the right-censored end; completions at the boundary remain observable.
        if now < stream.header.duration_ns {
            for job in output.jobs {
                if records.len() + world.running.len() >= cfg.max_checks {
                    return Err(ReplayError::Invalid("check record limit exceeded".into()));
                }
                let completed = now
                    .checked_add(cfg.policy.recheck_cost_ns)
                    .ok_or_else(|| ReplayError::Invalid("completion clock overflow".into()))?;
                report.dispatched.add(job.decision.tier);
                report.modeled_account_reads += 1;
                report.modeled_busy_ns = report
                    .modeled_busy_ns
                    .checked_add(
                        cfg.policy
                            .recheck_cost_ns
                            .min(stream.header.duration_ns - now),
                    )
                    .ok_or_else(|| ReplayError::Invalid("busy time overflow".into()))?;
                world.running.push(Running {
                    truth: truth.at(&job.key.borrower, now).cloned(),
                    job,
                    started: now,
                    completed,
                    chain: point(world.block, world.epoch),
                    epoch: world.epoch,
                });
            }
        }
        if now == stream.header.duration_ns {
            break;
        }
        if next_tick <= now {
            next_tick = now / cfg.tick_ns;
            next_tick = next_tick
                .checked_add(1)
                .and_then(|t| t.checked_mul(cfg.tick_ns))
                .unwrap_or(stream.header.duration_ns);
        }
        let next_observed = observable_times
            .get(observable_times.partition_point(|t| *t <= now))
            .copied()
            .unwrap_or(u64::MAX);
        let next_completion = world
            .running
            .iter()
            .map(|r| r.completed)
            .min()
            .unwrap_or(u64::MAX);
        now = next_tick
            .min(next_observed)
            .min(next_completion)
            .min(stream.header.duration_ns);
    }
    report.completed_checks = records.len() as u64;
    report.unavailable_state_checks = records.iter().filter(|r| !r.state_available).count() as u64;
    report.invalidated_checks = records.iter().filter(|r| r.invalidated).count() as u64;
    report.pending_checks = world.running.len() as u64;
    report.reorgs = world.reorgs;
    report.unique_mandatory_deadline_misses = missed.len() as u64;
    let mut latencies = Vec::new();
    let mut checks_by_borrower: BTreeMap<Address, Vec<&CheckRecord>> = BTreeMap::new();
    for record in &records {
        checks_by_borrower
            .entry(record.borrower)
            .or_default()
            .push(record);
    }
    for episode in truth
        .episodes
        .iter()
        .filter(|e| e.start_ns >= stream.header.measure_from_ns)
    {
        if episode.invalidated {
            report.invalidated_label_episodes += 1;
            continue;
        }
        report.eligible_episodes += 1;
        if episode.actionable {
            report.actionable_label_episodes += 1;
        }
        let checks = checks_by_borrower
            .get(&episode.borrower)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let start = checks.partition_point(|r| r.started_ns < episode.start_ns);
        let detection = checks[start..]
            .iter()
            .take_while(|r| episode.end_ns.is_none_or(|end| r.started_ns < end))
            .filter(|r| r.eligible && episode.end_ns.is_none_or(|end| r.completed_ns < end))
            .map(|r| r.completed_ns)
            .min();
        if let Some(detected) = detection {
            report.detected_episodes += 1;
            if episode.actionable {
                report.detected_actionable_label_episodes += 1;
            }
            latencies.push(detected - episode.start_ns);
        }
    }
    report.eligibility_detection_latency = Latency::from_samples(latencies);
    report.ordered_checks = records;
    Ok(report)
}

impl From<crate::event::LoadError> for ReplayError {
    fn from(e: crate::event::LoadError) -> Self {
        Self::Invalid(e.to_string())
    }
}

fn config_json(c: &PolicyConfig) -> Value {
    json!({
        "policy_version": c.policy_version,
        "max_account_age_ns": c.max_account_age_ns,
        "expiring_window_ns": c.expiring_window_ns,
        "near_threshold_wad": c.near_threshold_wad.to_string(),
        "p0_deadline_ns": c.p0_deadline_ns, "p1_deadline_ns": c.p1_deadline_ns,
        "p2_deadline_ns": c.p2_deadline_ns, "background_after_ns": c.background_after_ns,
        "eligible_refresh_ns": c.eligible_refresh_ns, "aging_ns": c.aging_ns,
        "concurrency": c.concurrency, "p0_share_permille": c.p0_share_permille,
        "p1_share_permille": c.p1_share_permille, "p2_share_permille": c.p2_share_permille,
        "p2_floor_slots": c.p2_floor_slots, "recheck_cost_ns": c.recheck_cost_ns
    })
}
