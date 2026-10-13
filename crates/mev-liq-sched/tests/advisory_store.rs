//! JEV-LIQ-011 integration: decisions stored by 008's `AdvisorStore` in a real SQLite file,
//! read back through `valid_decision_for`, validated again by the advisory policy.

use alloy::primitives::{Address, B256, U256, keccak256};
use mev_liq_sched::*;
use mev_liq_state::*;

const POLICY: &str = "policy-v1";
const SCHEMA: &str = "features-v1";
const MODEL: &str = "model-a";
const POOL: u8 = 0xaa;

fn key(n: u8) -> AccountKey {
    AccountKey {
        chain_id: 42161,
        pool: Address::repeat_byte(POOL),
        borrower: Address::repeat_byte(n),
    }
}

fn snap(n: u8, block: u64) -> BorrowerSnapshot {
    BorrowerSnapshot {
        key: key(n),
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: keccak256(block.to_be_bytes()),
            source: ChainSource::Provisional,
            observed_at_ns: block * 1_000,
        },
        health_factor_wad: WAD * U256::from(3u8),
        total_debt_base: U256::from(100u8),
        total_collateral_base: U256::from(1000u16),
        available_borrows_base: U256::from(7u8),
        liquidation_threshold: U256::from(8000u16),
        ltv: U256::from(7500u16),
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(block * 1_000),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

fn features(tag: &str) -> String {
    format!("{{\"schema_version\":\"{SCHEMA}\",\"health_factor_band\":\"{tag}\"}}")
}

struct Env {
    dir: tempfile::TempDir,
    repo: SnapshotRepository,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = SnapshotRepository::open(&dir.path().join("s.db")).unwrap();
        Self { dir, repo }
    }
    fn store(&self) -> AdvisorStore {
        AdvisorStore::open(
            &self.dir.path().join("a.db"),
            &self.dir.path().join("s.db"),
            QueueConfig {
                max_pending: 100,
                max_in_flight: 100,
                max_attempts: 3,
                base_backoff_ns: 10,
                max_backoff_ns: 1000,
            },
        )
        .unwrap()
    }
    fn export(&self, n: u8, block: u64, expected: u64, tag: &str) -> EnqueueOutcome {
        let j = features(tag);
        self.repo
            .enqueue_with_snapshot(
                &snap(n, block),
                expected,
                &ExportItem {
                    policy_version: POLICY,
                    feature_digest: keccak256(j.as_bytes()),
                    feature_json: &j,
                    enqueued_at_ns: block * 1_000,
                },
                &OutboxConfig {
                    max_export_rows: 100,
                },
            )
            .unwrap()
    }
}

fn dcfg() -> DecisionConfig {
    DecisionConfig {
        chain_id: 42161,
        policy_version: POLICY.into(),
        feature_schema: SCHEMA.into(),
        allowed_models: vec![MODEL.into()],
    }
}

fn stored_decision(o: &EnqueueOutcome, tag: &str, conf: f64, tier: WatchTier) -> AdvisoryDecision {
    AdvisoryDecision {
        key: o.snapshot.key,
        revision: o.snapshot.revision,
        feature_digest: keccak256(features(tag).as_bytes()),
        feature_schema: SCHEMA.into(),
        policy_version: POLICY.into(),
        prompt_digest: keccak256(b"prompt"),
        model: MODEL.into(),
        watch_tier: tier,
        confidence: conf,
        prep_probability: 0.4,
        value_score: 2.0,
        issued_at_ns: 15_000,
        expires_at_ns: 90_000,
        input_tokens: Some(10),
        output_tokens: Some(5),
        response_digest: keccak256(b"resp"),
        mode: DecisionMode::Shadow,
    }
}

fn adv() -> AdvisoryConfig {
    AdvisoryConfig::new(POLICY, SCHEMA, vec![MODEL.into()])
}

fn dirty(n: u8) -> DirtyEntry {
    DirtyEntry {
        key: key(n),
        reasons: vec![DirtyReason::Discovered],
        most_urgent: DirtyReason::Discovered,
        latest_chain: Some(ChainPoint {
            block_number: 500,
            block_hash: B256::repeat_byte(9),
        }),
        marked_at_ns: 16_000,
    }
}

fn order(o: &ScheduleOutput) -> Vec<u8> {
    o.jobs
        .iter()
        .map(|j| j.key.borrower.as_slice()[0])
        .collect()
}

/// Pull validated decisions and exports for the given accounts the way a shadow pass would.
fn inputs(env: &Env, store: &AdvisorStore, snaps: &[BorrowerSnapshot], now: u64) -> AdvisoryInputs {
    let reader = SnapshotReader::open(&env.dir.path().join("s.db")).unwrap();
    let mut out = AdvisoryInputs::default();
    for s in snaps {
        if let Some(d) = store.valid_decision_for(s, now, &dcfg()).unwrap() {
            out.decisions.push(OptionalDecision::from_stored(&d));
        }
        if let Some(e) = reader.current_export(&s.key).unwrap() {
            out.exports.insert(s.key, e);
        }
    }
    out
}

#[test]
fn stored_decisions_flow_into_the_policy_and_stale_ones_do_not() {
    let env = Env::new();
    let store = env.store();
    let now = 20_000u64;
    let e1 = env.export(1, 10, 0, "a");
    let e2 = env.export(2, 10, 0, "b");
    let e3 = env.export(3, 10, 0, "c");
    // account 3: high confidence; account 2: normal; account 1: no decision
    store
        .apply_decision(
            &stored_decision(&e3, "c", 0.97, WatchTier::High),
            now,
            &dcfg(),
        )
        .unwrap();
    store
        .apply_decision(
            &stored_decision(&e2, "b", 0.97, WatchTier::Normal),
            now,
            &dcfg(),
        )
        .unwrap();

    let accounts: Vec<AccountState> = [&e1, &e2, &e3]
        .iter()
        .map(|e| AccountState::new(e.snapshot.key, Some(e.snapshot.clone())))
        .collect();
    let dirty: Vec<DirtyEntry> = vec![dirty(1), dirty(2), dirty(3)];
    let snaps: Vec<BorrowerSnapshot> = accounts.iter().filter_map(|a| a.snapshot.clone()).collect();
    let cfg = PolicyConfig {
        concurrency: 1,
        ..PolicyConfig::default()
    };
    let run = |inp: &AdvisoryInputs, mode| {
        let input = ScheduleInput {
            now_ns: now,
            accounts: &accounts,
            dirty: &dirty,
            urgent_events: &[],
            pools: &[],
            in_flight: &[],
            optional_decisions: &inp.decisions,
        };
        let p = AdvisoryPolicy::new(mode, cfg.clone(), adv(), JevHealth::Healthy).unwrap();
        (
            p.run(&input, &inp.exports).unwrap(),
            schedule(&input, &cfg).unwrap(),
        )
    };

    let inp = inputs(&env, &store, &snaps, now);
    assert_eq!(inp.decisions.len(), 2);
    let (r, base) = run(&inp, AdvisoryMode::Shadow);
    assert_eq!(
        serde_json::to_string(&r.dispatch).unwrap(),
        serde_json::to_string(&base).unwrap()
    );
    assert_eq!(order(&r.dispatch), vec![1]);
    let rep = r.report.unwrap();
    assert_eq!(rep.baseline_p2_order.len(), 1);
    assert!(
        rep.order_changed,
        "account 3 would have been dispatched first"
    );
    assert_eq!(rep.applied.len(), 1);
    assert_eq!(
        rep.counters.copper_jev_fallback_total.get("no_decision"),
        Some(&1)
    );
    assert_eq!(
        rep.counters.copper_jev_applied_bonus_total.get("high"),
        Some(&1)
    );

    let old = store
        .valid_decision_for(&e3.snapshot, now, &dcfg())
        .unwrap()
        .unwrap();
    // Revision r+1 is stored for account 3 after the decision for revision r.
    let e3b = env.export(3, 11, e3.snapshot.revision, "c2");
    assert_eq!(e3b.snapshot.revision, e3.snapshot.revision + 1);
    // 008 itself no longer returns the old decision ...
    assert!(
        store
            .valid_decision_for(&e3b.snapshot, now, &dcfg())
            .unwrap()
            .is_none()
    );
    // ... and even a caller that kept the old decision cannot get it applied by the policy.
    let mut accounts2 = accounts.clone();
    accounts2[2] = AccountState::new(e3b.snapshot.key, Some(e3b.snapshot.clone()));
    let mut inp2 = inputs(&env, &store, std::slice::from_ref(&e3b.snapshot), now);
    inp2.decisions.push(OptionalDecision::from_stored(&old));
    let input = ScheduleInput {
        now_ns: now,
        accounts: &accounts2,
        dirty: &dirty,
        urgent_events: &[],
        pools: &[],
        in_flight: &[],
        optional_decisions: &inp2.decisions,
    };
    let wb = would_be_schedule(&input, &cfg, &adv(), JevHealth::Healthy, &inp2.exports).unwrap();
    let v = wb.verdicts.get(&key(3)).unwrap();
    assert_eq!(
        (v.bonus, v.fallback),
        (0, Some(FallbackReason::RevisionMismatch))
    );

    // Past its TTL, 008 stops serving it; a stale copy is rejected as expired by the policy.
    let late = 200_000u64;
    assert!(
        store
            .valid_decision_for(&e3.snapshot, late, &dcfg())
            .unwrap()
            .is_none()
    );
    let input = ScheduleInput {
        now_ns: late,
        accounts: &accounts,
        dirty: &dirty,
        urgent_events: &[],
        pools: &[],
        in_flight: &[],
        optional_decisions: &inp.decisions,
    };
    let wb = would_be_schedule(&input, &cfg, &adv(), JevHealth::Healthy, &inp.exports).unwrap();
    let v = wb.verdicts.get(&key(3)).unwrap();
    assert_eq!(v.fallback, Some(FallbackReason::Expired));
}
