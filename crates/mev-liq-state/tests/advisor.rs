//! JEV-LIQ-008: outbox + advisor storage against real SQLite files (no mocks).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use alloy::primitives::{Address, B256, U256, keccak256};
use mev_liq_state::*;
use proptest::prelude::*;
use rusqlite::Connection;

const POLICY: &str = "policy-v1";
const SCHEMA: &str = "features-v1";
const MODEL: &str = "model-a";

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
    fn snap_path(&self) -> PathBuf {
        self.dir.path().join("s.db")
    }
    fn adv_path(&self) -> PathBuf {
        self.dir.path().join("a.db")
    }
    fn store(&self, cfg: QueueConfig) -> AdvisorStore {
        AdvisorStore::open(&self.adv_path(), &self.snap_path(), cfg).unwrap()
    }
}

fn qcfg() -> QueueConfig {
    QueueConfig {
        max_pending: 1000,
        max_in_flight: 1000,
        max_attempts: 3,
        base_backoff_ns: 10,
        max_backoff_ns: 1000,
    }
}

fn ocfg() -> OutboxConfig {
    OutboxConfig {
        max_export_rows: 1000,
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

fn key(n: u8) -> AccountKey {
    AccountKey {
        chain_id: 42161,
        pool: Address::repeat_byte(0xaa),
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
        health_factor_wad: WAD + U256::from(5u8),
        total_debt_base: U256::from(100u8),
        total_collateral_base: U256::from(200u8),
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

fn json(tag: &str) -> String {
    format!("{{\"schema_version\":\"{SCHEMA}\",\"health_factor_band\":\"{tag}\"}}")
}

fn enqueue(env: &Env, n: u8, block: u64, expected: u64, tag: &str) -> EnqueueOutcome {
    let j = json(tag);
    env.repo
        .enqueue_with_snapshot(
            &snap(n, block),
            expected,
            &ExportItem {
                policy_version: POLICY,
                feature_digest: keccak256(j.as_bytes()),
                feature_json: &j,
                enqueued_at_ns: block * 1_000,
            },
            &ocfg(),
        )
        .unwrap()
}

fn decision(o: &EnqueueOutcome) -> AdvisoryDecision {
    AdvisoryDecision {
        key: o.snapshot.key,
        revision: o.snapshot.revision,
        feature_digest: {
            // the digest the producer exported for this revision
            let j = json("a");
            keccak256(j.as_bytes())
        },
        feature_schema: SCHEMA.into(),
        policy_version: POLICY.into(),
        prompt_digest: keccak256(b"prompt"),
        model: MODEL.into(),
        watch_tier: WatchTier::High,
        confidence: 0.9,
        prep_probability: 0.4,
        value_score: 2.0,
        issued_at_ns: 5_000,
        expires_at_ns: 50_000,
        input_tokens: Some(10),
        output_tokens: Some(5),
        response_digest: keccak256(b"resp"),
        mode: DecisionMode::Shadow,
    }
}

fn raw(env: &Env) -> Connection {
    Connection::open(env.snap_path()).unwrap()
}

// ---- enqueue_with_snapshot ----

#[test]
fn enqueue_stores_snapshot_and_export_and_dedups_by_event_key() {
    let env = Env::new();
    let a = enqueue(&env, 1, 10, 0, "a");
    assert!(a.enqueued && a.snapshot.revision == 1 && a.shed == 0);
    assert_eq!(env.repo.export_len().unwrap(), 1);
    assert_eq!(env.repo.fetch_by_key(&key(1)).unwrap().unwrap().revision, 1);
    // Same revision/digest/policy cannot be exported twice: a CAS at the same expected revision
    // is stale, and a new revision with the same digest has a different event key.
    let b = enqueue(&env, 1, 10, 1, "a");
    assert!(b.enqueued && b.snapshot.revision == 2);
    assert_ne!(a.event_key, b.event_key);
    // Pure dedup: replay the exact export through a raw duplicate key.
    let dup = raw(&env)
        .execute(
            "INSERT OR IGNORE INTO advisor_export(event_key,chain_id,pool,borrower,revision,
               feature_digest,policy_version,feature_json,enqueued_at_ns)
             SELECT event_key,chain_id,pool,borrower,revision,feature_digest,policy_version,
               feature_json,enqueued_at_ns FROM advisor_export",
            [],
        )
        .unwrap();
    assert_eq!(dup, 0);
    assert_eq!(env.repo.export_len().unwrap(), 2);
    assert_eq!(
        advisor_event_key(&key(1), 1, a_digest("a"), POLICY),
        a.event_key
    );
}

fn a_digest(tag: &str) -> B256 {
    keccak256(json(tag).as_bytes())
}

#[test]
fn enqueue_rejects_json_leaking_account_addresses_before_any_write() {
    let env = Env::new();
    let borrower = format!("{:x}", Address::repeat_byte(7));
    let j = format!("{{\"x\":\"0x{}\"}}", borrower.to_uppercase());
    let err = env
        .repo
        .enqueue_with_snapshot(
            &snap(7, 10),
            0,
            &ExportItem {
                policy_version: POLICY,
                feature_digest: keccak256(b"x"),
                feature_json: &j,
                enqueued_at_ns: 1,
            },
            &ocfg(),
        )
        .unwrap_err();
    assert!(matches!(err, RepoError::BadExport(_)));
    assert!(env.repo.fetch_by_key(&key(7)).unwrap().is_none());
    assert_eq!(env.repo.export_len().unwrap(), 0);
}

#[test]
fn failure_after_cas_rolls_back_snapshot_and_export_together() {
    let env = Env::new();
    // Failure #1: the export insert aborts (real SQLite trigger) after the CAS write ran.
    raw(&env)
        .execute_batch(
            "CREATE TRIGGER inject_export BEFORE INSERT ON advisor_export
             BEGIN SELECT RAISE(ABORT,'injected export failure'); END;",
        )
        .unwrap();
    let j = json("a");
    let item = ExportItem {
        policy_version: POLICY,
        feature_digest: keccak256(j.as_bytes()),
        feature_json: &j,
        enqueued_at_ns: 1,
    };
    let err = env
        .repo
        .enqueue_with_snapshot(&snap(1, 10), 0, &item, &ocfg())
        .unwrap_err();
    assert!(err.to_string().contains("injected export failure"), "{err}");
    assert!(
        env.repo.fetch_by_key(&key(1)).unwrap().is_none(),
        "CAS must roll back"
    );
    assert_eq!(env.repo.export_len().unwrap(), 0);
    raw(&env)
        .execute_batch("DROP TRIGGER inject_export")
        .unwrap();

    // Failure #2: the LAST statement (advisor_current) aborts after the export row was written.
    raw(&env)
        .execute_batch(
            "CREATE TRIGGER inject_current BEFORE INSERT ON advisor_current
             BEGIN SELECT RAISE(ABORT,'injected current failure'); END;",
        )
        .unwrap();
    let err = env
        .repo
        .enqueue_with_snapshot(&snap(1, 10), 0, &item, &ocfg())
        .unwrap_err();
    assert!(err.to_string().contains("injected current failure"));
    assert!(env.repo.fetch_by_key(&key(1)).unwrap().is_none());
    assert_eq!(
        env.repo.export_len().unwrap(),
        0,
        "export row must roll back"
    );
    raw(&env)
        .execute_batch("DROP TRIGGER inject_current")
        .unwrap();

    // Healthy again: the same call now commits both.
    let ok = env
        .repo
        .enqueue_with_snapshot(&snap(1, 10), 0, &item, &ocfg())
        .unwrap();
    assert!(ok.enqueued);
    assert_eq!(env.repo.export_len().unwrap(), 1);
}

#[test]
fn stale_cas_enqueues_nothing() {
    let env = Env::new();
    enqueue(&env, 1, 10, 0, "a");
    let j = json("b");
    let err = env
        .repo
        .enqueue_with_snapshot(
            &snap(1, 11),
            0, // stale: stored revision is 1
            &ExportItem {
                policy_version: POLICY,
                feature_digest: keccak256(j.as_bytes()),
                feature_json: &j,
                enqueued_at_ns: 1,
            },
            &ocfg(),
        )
        .unwrap_err();
    assert!(matches!(err, RepoError::Stale { .. }));
    assert_eq!(env.repo.export_len().unwrap(), 1);
}

#[test]
fn bounded_log_sheds_oldest_with_counted_record_and_advisor_counts_missed() {
    let env = Env::new();
    let cfg = OutboxConfig { max_export_rows: 3 };
    let mut shed = 0;
    for n in 1..=5u8 {
        let j = json("a");
        let o = env
            .repo
            .enqueue_with_snapshot(
                &snap(n, 10),
                0,
                &ExportItem {
                    policy_version: POLICY,
                    feature_digest: keccak256(j.as_bytes()),
                    feature_json: &j,
                    enqueued_at_ns: u64::from(n),
                },
                &cfg,
            )
            .unwrap();
        shed += o.shed;
        // mandatory state is never shed
        assert!(env.repo.fetch_by_key(&key(n)).unwrap().is_some());
    }
    assert_eq!(shed, 2);
    assert_eq!(env.repo.export_len().unwrap(), 3);
    assert_eq!(env.repo.shed_total().unwrap(), 2);
    let (first, last, count): (i64, i64, i64) = raw(&env)
        .query_row(
            "SELECT MIN(first_id), MAX(last_id), SUM(count) FROM advisor_shed",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((first, last, count), (1, 2, 2));
    let store = env.store(qcfg());
    let p = store.pull(100, 100).unwrap();
    assert_eq!(
        p,
        PullOutcome {
            copied: 3,
            missed: 2
        }
    );
    assert_eq!(store.stats(100).unwrap().missed_total, 2);
    assert_eq!(store.reader().shed_total().unwrap(), 2);
}

// ---- queue ----

fn filled(n: u8) -> (Env, AdvisorStore) {
    let env = Env::new();
    for i in 1..=n {
        enqueue(&env, i, 10, 0, "a");
    }
    let store = env.store(qcfg());
    assert_eq!(store.pull(0, 1000).unwrap().copied, u64::from(n));
    (env, store)
}

#[test]
fn pull_is_idempotent_and_respects_max_pending() {
    let env = Env::new();
    for i in 1..=5 {
        enqueue(&env, i, 10, 0, "a");
    }
    let mut c = qcfg();
    c.max_pending = 3;
    let store = env.store(c);
    assert_eq!(store.pull(0, 100).unwrap().copied, 3);
    assert_eq!(store.pull(0, 100).unwrap().copied, 0, "queue full");
    let got = store.claim("w", 0, 100, 10).unwrap();
    for g in &got {
        store.ack("w", g.id, 1).unwrap();
    }
    assert_eq!(store.pull(2, 100).unwrap().copied, 2);
    assert_eq!(store.pull(3, 100).unwrap().copied, 0, "caught up");
}

#[test]
fn claim_is_exclusive_across_handles_and_threads() {
    for round in 0..6 {
        let env = Env::new();
        for i in 1..=40u8 {
            enqueue(&env, i, 10, 0, "a");
        }
        env.store(qcfg()).pull(0, 1000).unwrap();
        let n_threads = 4;
        let barrier = Arc::new(Barrier::new(n_threads));
        let handles: Vec<_> = (0..n_threads)
            .map(|t| {
                let (a, s) = (env.adv_path(), env.snap_path());
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    // each thread has its own file handles, like a separate process
                    let store = AdvisorStore::open(&a, &s, qcfg()).unwrap();
                    barrier.wait();
                    let mut mine = vec![];
                    loop {
                        let got = store.claim(&format!("w{t}"), 1, 1_000_000, 3).unwrap();
                        if got.is_empty() {
                            break;
                        }
                        mine.extend(got.into_iter().map(|c| c.id));
                    }
                    mine
                })
            })
            .collect();
        let mut all = vec![];
        for h in handles {
            all.extend(h.join().unwrap());
        }
        let uniq: HashSet<_> = all.iter().copied().collect();
        assert_eq!(all.len(), 40, "round {round}: every row claimed");
        assert_eq!(uniq.len(), 40, "round {round}: no row claimed twice");
    }
}

#[test]
fn expired_lease_is_reclaimed_and_old_holder_cannot_ack() {
    let (_env, store) = filled(1);
    let a = store.claim("a", 100, 50, 5).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].lease_until_ns, 150);
    assert!(
        store.claim("b", 149, 50, 5).unwrap().is_empty(),
        "lease still held"
    );
    let b = store.claim("b", 150, 50, 5).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].attempts, 2);
    assert!(matches!(
        store.ack("a", a[0].id, 160),
        Err(AdvisorError::LeaseLost(_))
    ));
    store.ack("b", b[0].id, 160).unwrap();
    assert!(matches!(
        store.ack("b", b[0].id, 161),
        Err(AdvisorError::LeaseLost(_))
    ));
    assert!(store.claim("c", 10_000, 50, 5).unwrap().is_empty());
}

#[test]
fn expired_ack_is_refused_and_max_in_flight_is_enforced() {
    let env = Env::new();
    for i in 1..=4 {
        enqueue(&env, i, 10, 0, "a");
    }
    let mut c = qcfg();
    c.max_in_flight = 2;
    let store = env.store(c);
    store.pull(0, 100).unwrap();
    let first = store.claim("a", 0, 10, 10).unwrap();
    assert_eq!(first.len(), 2, "capped by max_in_flight");
    assert!(store.claim("b", 5, 10, 10).unwrap().is_empty());
    assert_eq!(store.stats(5).unwrap().in_flight, 2);
    assert!(matches!(
        store.ack("a", first[0].id, 10),
        Err(AdvisorError::LeaseLost(_))
    ));
    let second = store.claim("b", 10, 10, 10).unwrap();
    assert_eq!(second.len(), 2, "expired leases free slots");
}

#[test]
fn crash_restart_restores_unacked_work_and_watermark() {
    let env = Env::new();
    for i in 1..=3 {
        enqueue(&env, i, 10, 0, "a");
    }
    let acked_id;
    {
        let store = env.store(qcfg());
        store.pull(0, 100).unwrap();
        let got = store.claim("w", 0, 100, 3).unwrap();
        acked_id = got[0].id;
        store.ack("w", acked_id, 1).unwrap();
        // "crash": two rows are leased and never settled
    }
    let store = env.store(qcfg());
    assert_eq!(store.pull(2, 100).unwrap().copied, 0, "watermark persisted");
    assert!(
        store.claim("w2", 50, 100, 10).unwrap().is_empty(),
        "leases survive a restart"
    );
    let got = store.claim("w2", 100, 100, 10).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|c| c.id != acked_id));
    let st = store.stats(100).unwrap();
    assert_eq!((st.done, st.in_flight), (1, 2));
}

#[test]
fn retry_backoff_and_dead_letter_schedule_is_exact() {
    let env = Env::new();
    enqueue(&env, 1, 10, 0, "a");
    let mut c = qcfg();
    c.max_attempts = 4;
    c.base_backoff_ns = 10;
    c.max_backoff_ns = 35;
    let store = env.store(c);
    store.pull(0, 10).unwrap();
    let want = [(0u64, 10u64), (10, 30), (30, 65)];
    let mut t = 0;
    for (i, (at, avail)) in want.iter().enumerate() {
        assert_eq!(t, *at);
        let got = store.claim("w", t, 100, 1).unwrap();
        assert_eq!(got[0].attempts, i as u32 + 1);
        let out = store.fail("w", got[0].id, t, "boom").unwrap();
        assert_eq!(
            out,
            FailOutcome::Retry {
                available_at_ns: *avail
            },
            "attempt {}",
            i + 1
        );
        assert!(
            store.claim("w", avail - 1, 100, 1).unwrap().is_empty(),
            "not before backoff"
        );
        t = *avail;
    }
    let got = store.claim("w", t, 100, 1).unwrap();
    assert_eq!(got[0].attempts, 4);
    assert_eq!(
        store.fail("w", got[0].id, t, "boom").unwrap(),
        FailOutcome::Dead
    );
    let st = store.stats(t).unwrap();
    assert_eq!((st.dead, st.pending), (1, 0));
    assert!(store.claim("w", t + 10_000, 100, 1).unwrap().is_empty());
}

#[test]
fn poison_row_that_keeps_expiring_is_dead_lettered() {
    let env = Env::new();
    enqueue(&env, 1, 10, 0, "a");
    let mut c = qcfg();
    c.max_attempts = 2;
    let store = env.store(c);
    store.pull(0, 10).unwrap();
    for t in [0u64, 100] {
        assert_eq!(store.claim("w", t, 50, 1).unwrap().len(), 1); // crashes, never settles
    }
    assert!(store.claim("w", 1000, 50, 1).unwrap().is_empty());
    assert_eq!(store.stats(1000).unwrap().dead, 1);
}

#[test]
fn release_refunds_the_attempt_and_is_immediately_claimable() {
    let (_env, store) = filled(1);
    let g = store.claim("a", 5, 100, 1).unwrap();
    assert!(matches!(
        store.release("other", g[0].id, 6),
        Err(AdvisorError::LeaseLost(_))
    ));
    store.release("a", g[0].id, 6).unwrap();
    let again = store.claim("b", 6, 100, 1).unwrap();
    assert_eq!(again[0].attempts, 1);
}

#[test]
fn dead_letter_requires_the_lease_and_is_final() {
    let (_env, store) = filled(1);
    let g = store.claim("a", 5, 100, 1).unwrap();
    assert!(matches!(
        store.dead_letter("other", g[0].id, 6, "x"),
        Err(AdvisorError::LeaseLost(_))
    ));
    store
        .dead_letter("a", g[0].id, 6, "invalid features")
        .unwrap();
    assert_eq!(store.stats(7).unwrap().dead, 1);
    assert!(store.claim("a", 1000, 100, 1).unwrap().is_empty());
    assert!(matches!(
        store.dead_letter("a", g[0].id, 8, "again"),
        Err(AdvisorError::LeaseLost(_))
    ));
}

// ---- decisions ----

fn live() -> (Env, AdvisorStore, EnqueueOutcome) {
    let env = Env::new();
    let o = enqueue(&env, 1, 10, 0, "a");
    let store = env.store(qcfg());
    (env, store, o)
}

#[test]
fn apply_decision_happy_path_and_read_helper() {
    let (_e, store, o) = live();
    let d = decision(&o);
    assert_eq!(
        store.apply_decision(&d, 6_000, &dcfg()).unwrap(),
        DecisionApply::Inserted
    );
    let got = store
        .valid_decision_for(&o.snapshot, 7_000, &dcfg())
        .unwrap()
        .unwrap();
    assert_eq!(got, d);
    // expiry boundary: expires_at > now
    assert!(
        store
            .valid_decision_for(&o.snapshot, 50_000, &dcfg())
            .unwrap()
            .is_none()
    );
    // model removed from the allow list
    let mut other = dcfg();
    other.allowed_models = vec!["model-b".into()];
    assert!(
        store
            .valid_decision_for(&o.snapshot, 7_000, &other)
            .unwrap()
            .is_none()
    );
    // wrong policy
    let mut other = dcfg();
    other.policy_version = "policy-v2".into();
    assert!(
        store
            .valid_decision_for(&o.snapshot, 7_000, &other)
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_future_and_newer_revision_race_are_rejected() {
    let (env, store, o) = live();
    let r1 = decision(&o);
    // The producer commits revision 2 while the response for revision 1 is in flight.
    let o2 = enqueue(&env, 1, 11, 1, "a");
    assert_eq!(o2.snapshot.revision, 2);
    assert!(matches!(
        store.apply_decision(&r1, 20_000, &dcfg()),
        Err(AdvisorError::StaleRevision {
            decision: 1,
            current: 2
        })
    ));
    assert!(
        store
            .valid_decision_for(&o2.snapshot, 20_000, &dcfg())
            .unwrap()
            .is_none()
    );
    let mut ahead = decision(&o2);
    ahead.revision = 3;
    assert!(matches!(
        store.apply_decision(&ahead, 20_000, &dcfg()),
        Err(AdvisorError::FutureRevision { .. })
    ));
    assert_eq!(store.decision_count(&key(1)).unwrap(), 0);
    // a response for the current revision still applies
    let mut r2 = decision(&o2);
    r2.feature_digest = a_digest("a");
    assert!(store.apply_decision(&r2, 20_000, &dcfg()).is_ok());
}

#[test]
fn expired_digest_policy_chain_model_and_nan_are_rejected() {
    let (_e, store, o) = live();
    let base = decision(&o);
    let now = 6_000;
    let mut d = base.clone();
    d.expires_at_ns = now;
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::Expired { .. })
    ));
    let mut d = base.clone();
    d.feature_digest = a_digest("different");
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::DigestMismatch)
    ));
    let mut d = base.clone();
    d.policy_version = "policy-v0".into();
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::PolicyVersionMismatch)
    ));
    let mut d = base.clone();
    d.feature_schema = "features-v0".into();
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::FeatureSchemaMismatch)
    ));
    let mut d = base.clone();
    d.key.chain_id = 1;
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::ChainMismatch)
    ));
    let mut d = base.clone();
    d.model = "model-z".into();
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::ModelNotAllowed)
    ));
    let mut d = base.clone();
    d.key = key(9); // pool is unchanged but this borrower has no snapshot
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::UnknownAccount)
    ));
    let mut d = base.clone();
    d.issued_at_ns = now + 1;
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::IssuedInFuture)
    ));
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.0000001] {
        let mut d = base.clone();
        d.confidence = bad;
        assert!(matches!(
            store.apply_decision(&d, now, &dcfg()),
            Err(AdvisorError::Malformed(_))
        ));
        let mut d = base.clone();
        d.prep_probability = bad;
        assert!(matches!(
            store.apply_decision(&d, now, &dcfg()),
            Err(AdvisorError::Malformed(_))
        ));
    }
    for bad in [f64::NAN, -1.0, 4.5] {
        let mut d = base.clone();
        d.value_score = bad;
        assert!(matches!(
            store.apply_decision(&d, now, &dcfg()),
            Err(AdvisorError::Malformed(_))
        ));
    }
    let mut d = base.clone();
    d.expires_at_ns = d.issued_at_ns;
    assert!(matches!(
        store.apply_decision(&d, 1, &dcfg()),
        Err(AdvisorError::Malformed(_))
    ));
    let mut d = base.clone();
    d.model = "bad model!".into();
    assert!(matches!(
        store.apply_decision(&d, now, &dcfg()),
        Err(AdvisorError::Malformed(_))
    ));
    assert_eq!(store.decision_count(&key(1)).unwrap(), 0, "nothing stored");
}

#[test]
fn account_updated_without_export_has_no_current_export() {
    let (env, store, o) = live();
    // a plain CAS (no export) moves the revision; the old export no longer describes it
    env.repo.upsert_cas(&snap(1, 12), 1).unwrap();
    assert!(matches!(
        store.apply_decision(&decision(&o), 20_000, &dcfg()),
        Err(AdvisorError::StaleRevision { .. })
    ));
    let mut d = decision(&o);
    d.revision = 2;
    assert!(matches!(
        store.apply_decision(&d, 20_000, &dcfg()),
        Err(AdvisorError::NoCurrentExport)
    ));
}

#[test]
fn one_decision_per_key_and_refresh_on_conflict() {
    let (_e, store, o) = live();
    let d = decision(&o);
    assert_eq!(
        store.apply_decision(&d, 6_000, &dcfg()).unwrap(),
        DecisionApply::Inserted
    );
    let mut again = d.clone();
    again.issued_at_ns = 40_000;
    again.expires_at_ns = 90_000;
    again.confidence = 0.95;
    again.response_digest = keccak256(b"resp2");
    assert_eq!(
        store.apply_decision(&again, 41_000, &dcfg()).unwrap(),
        DecisionApply::Refreshed
    );
    assert_eq!(store.decision_count(&key(1)).unwrap(), 1);
    // the previously expired decision is usable again after the refresh
    let got = store
        .valid_decision_for(&o.snapshot, 60_000, &dcfg())
        .unwrap()
        .unwrap();
    assert_eq!(got.expires_at_ns, 90_000);
    assert_eq!(got.response_digest, keccak256(b"resp2"));
    // an older call finishing late cannot overwrite the newer response
    let mut late = d.clone();
    late.issued_at_ns = 10_000;
    assert!(matches!(
        store.apply_decision(&late, 41_000, &dcfg()),
        Err(AdvisorError::SupersededResponse)
    ));
    // a different prompt is a different key
    let mut other = d.clone();
    other.prompt_digest = keccak256(b"prompt-2");
    assert_eq!(
        store.apply_decision(&other, 41_000, &dcfg()).unwrap(),
        DecisionApply::Inserted
    );
    assert_eq!(store.decision_count(&key(1)).unwrap(), 2);
}

#[test]
fn policy_outcome_round_trips() {
    let (_e, store, _o) = live();
    let out = PolicyOutcome {
        run_id: "run1".into(),
        policy_name: "baseline".into(),
        key: key(1),
        state_block: 10,
        event_kind: "price_move".into(),
        scheduled_at_ns: Some(1),
        observed_at_ns: None,
        check_completed_at_ns: Some(9),
        eligible: Some(true),
        simulatable: None,
        estimated_profit_base: Some(U256::from(12345u32)),
        rpc_calls: 3,
        simulation_count: 1,
        reason_codes: vec!["P1".into(), "DEADLINE".into()],
    };
    store.record_policy_outcome(&out).unwrap();
    assert_eq!(store.policy_outcomes("run1").unwrap(), vec![out]);
    assert!(store.policy_outcomes("nope").unwrap().is_empty());
}

// ---- migrations / isolation ----

fn tables(c: &Connection) -> Vec<String> {
    let mut st = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn migration_4_up_down_preserves_v1_to_v3_data() {
    let env = Env::new();
    assert_eq!(env.repo.schema_version().unwrap(), 4);
    assert_eq!(SCHEMA_VERSION, 4);
    env.repo.upsert_cas(&snap(1, 10), 0).unwrap();
    env.repo.rollback_to(3).unwrap();
    assert_eq!(env.repo.schema_version().unwrap(), 3);
    let t = tables(&raw(&env));
    for gone in ["advisor_export", "advisor_current", "advisor_shed"] {
        assert!(!t.contains(&gone.to_string()), "{gone} dropped");
    }
    for kept in ["snapshots", "dirty", "checkpoints", "snapshot_history"] {
        assert!(t.contains(&kept.to_string()), "{kept} kept");
    }
    assert_eq!(env.repo.fetch_by_key(&key(1)).unwrap().unwrap().revision, 1);
    // reopening re-applies 4 on top of surviving data
    let dir = env.dir;
    drop(env.repo);
    let repo = SnapshotRepository::open(&dir.path().join("s.db")).unwrap();
    assert_eq!(repo.schema_version().unwrap(), 4);
    assert_eq!(repo.fetch_by_key(&key(1)).unwrap().unwrap().revision, 1);
    assert_eq!(repo.export_len().unwrap(), 0);
}

#[test]
fn advisor_schema_rolls_back_and_reapplies() {
    let (env, store, _o) = live();
    assert_eq!(store.schema_version().unwrap(), ADVISOR_SCHEMA_VERSION);
    store.rollback_to(0).unwrap();
    assert_eq!(store.schema_version().unwrap(), 0);
    let c = Connection::open(env.adv_path()).unwrap();
    assert!(!tables(&c).contains(&"advisory_decision".to_string()));
    drop(store);
    let store = env.store(qcfg());
    assert_eq!(store.schema_version().unwrap(), 1);
}

#[test]
fn reader_requires_schema_4_and_never_creates_files() {
    let d = tempfile::tempdir().unwrap();
    assert!(SnapshotReader::open(&d.path().join("missing.db")).is_err());
    let p = d.path().join("old.db");
    let repo = SnapshotRepository::open(&p).unwrap();
    repo.rollback_to(3).unwrap();
    assert!(matches!(
        SnapshotReader::open(&p),
        Err(AdvisorError::SnapshotSchemaTooOld(3))
    ));
}

// ---- property: claim/ack sequences ----

#[derive(Debug, Clone)]
enum Op {
    Claim(u8, u8),
    Ack(u8),
    Fail(u8),
    Release(u8),
    Advance(u16),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..3, 1u8..4).prop_map(|(o, n)| Op::Claim(o, n)),
        any::<u8>().prop_map(Op::Ack),
        any::<u8>().prop_map(Op::Fail),
        any::<u8>().prop_map(Op::Release),
        (0u16..300).prop_map(Op::Advance),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn claim_ack_sequences_never_double_ack_or_lose_a_row(ops in proptest::collection::vec(op(), 1..60)) {
        const ROWS: u8 = 6;
        let (_env, store) = filled(ROWS);
        let mut now = 1_000u64;
        let lease = 100u64;
        let mut seen: Vec<(i64, String)> = vec![];
        let mut live: HashMap<i64, (String, u64)> = HashMap::new();
        let mut settled: HashSet<i64> = HashSet::new();
        for o in ops {
            match o {
                Op::Claim(w, n) => {
                    let owner = format!("w{w}");
                    for c in store.claim(&owner, now, lease, n as usize).unwrap() {
                        if let Some((_, until)) = live.get(&c.id) {
                            prop_assert!(*until <= now, "row {} double-leased", c.id);
                        }
                        prop_assert!(!settled.contains(&c.id), "settled row {} re-claimed", c.id);
                        live.insert(c.id, (owner.clone(), now + lease));
                        seen.push((c.id, owner.clone()));
                    }
                }
                Op::Ack(i) | Op::Fail(i) | Op::Release(i) if seen.is_empty() => { let _ = i; }
                Op::Ack(i) => {
                    let (id, owner) = seen[i as usize % seen.len()].clone();
                    let r = store.ack(&owner, id, now);
                    if r.is_ok() {
                        prop_assert!(settled.insert(id), "row {id} acked twice");
                        live.remove(&id);
                    }
                }
                Op::Fail(i) => {
                    let (id, owner) = seen[i as usize % seen.len()].clone();
                    if let Ok(out) = store.fail(&owner, id, now, "e") {
                        live.remove(&id);
                        if out == FailOutcome::Dead { prop_assert!(settled.insert(id)); }
                    }
                }
                Op::Release(i) => {
                    let (id, owner) = seen[i as usize % seen.len()].clone();
                    if store.release(&owner, id, now).is_ok() { live.remove(&id); }
                }
                Op::Advance(dt) => now += u64::from(dt),
            }
            let st = store.stats(now).unwrap();
            prop_assert_eq!(st.pending + st.done + st.dead, u64::from(ROWS), "no row lost");
        }
        // Drain: far in the future every unsettled row is still claimable and ackable once.
        now += 1_000_000;
        let mut guard = 0;
        loop {
            let got = store.claim("drain", now, lease, 10).unwrap();
            if got.is_empty() { break; }
            for c in got {
                store.ack("drain", c.id, now).unwrap();
                prop_assert!(settled.insert(c.id), "drained row {} was already settled", c.id);
            }
            guard += 1;
            prop_assert!(guard < 20);
        }
        let st = store.stats(now).unwrap();
        prop_assert_eq!(st.pending, 0, "unacked row lost or stuck");
        prop_assert_eq!(st.done + st.dead, u64::from(ROWS));
    }
}
