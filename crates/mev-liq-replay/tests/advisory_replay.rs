//! JEV-LIQ-011: the advisory policy plugs into the 006 replay harness as the REPLAY_JEV arm at
//! equal budgets, and leaves the mandatory (P0/P1) metrics identical to the baseline.

use alloy::primitives::{Address, B256, U256};
use mev_liq_replay::{ReplayConfig, event::*, replay, replay_with_policy};
use mev_liq_sched::{
    AdvisoryConfig, AdvisoryInputs, JevHealth, OptionalDecision, ReplayAdvisoryPolicy, Tier,
};
use mev_liq_state::{WAD, WatchTier};

const MS: u64 = 1_000_000;
const POOL: Address = Address::repeat_byte(0xaa);

fn borrower(n: u8) -> Address {
    Address::repeat_byte(n)
}

fn digest(n: u8) -> B256 {
    B256::repeat_byte(0xD0 | (n & 0x0f))
}

fn state(n: u8, hf: U256) -> Body {
    Body::State {
        borrower: borrower(n),
        health_factor_wad: hf,
        total_debt_base: U256::from(100),
        total_collateral_base: U256::from(200),
        reserve_asset: Some(borrower(0xee)),
    }
}

fn scenario() -> ReplayStream {
    let header = Header {
        format_version: 1,
        dataset_id: "synthetic-advisory".into(),
        synthetic: true,
        scenario: "ten healthy accounts touched at once plus one eligible account".into(),
        provenance: "hand-written synthetic fixture".into(),
        chain_id: 42161,
        pool: POOL,
        source_block_start: 100,
        source_block_end: 200,
        duration_ns: 400 * MS,
        measure_from_ns: 0,
        seed: Some(1),
    };
    let mut ev = vec![];
    let mut seq = 0;
    let mut push = |ms: u64, block: u64, body: Body| {
        seq += 1;
        ev.push(Event {
            seq,
            t_ns: ms * MS,
            block,
            body,
        });
    };
    for n in 1..=11u8 {
        push(
            0,
            100,
            Body::Register {
                borrower: borrower(n),
            },
        );
        push(0, 100, state(n, WAD * U256::from(2)));
    }
    for n in 1..=10u8 {
        push(
            100,
            101,
            Body::BorrowerEvent {
                borrower: borrower(n),
            },
        );
    }
    push(150, 102, state(11, WAD - U256::from(1)));
    push(
        150,
        102,
        Body::Eligible {
            borrower: borrower(11),
            actionable: true,
        },
    );
    push(
        150,
        102,
        Body::BorrowerEvent {
            borrower: borrower(11),
        },
    );
    parse_jsonl(&to_jsonl(&header, &ev)).unwrap()
}

fn config() -> ReplayConfig {
    let mut c = ReplayConfig::default();
    c.policy.concurrency = 2;
    c.policy.background_after_ns = 10_000 * MS;
    c.policy.max_account_age_ns = 5_000 * MS;
    c.policy.expiring_window_ns = 100 * MS;
    c.policy.eligible_refresh_ns = 100 * MS;
    c
}

fn advisory_policy(
    cfg: &ReplayConfig,
    health: JevHealth,
) -> ReplayAdvisoryPolicy<impl Fn(&mev_liq_sched::ScheduleInput<'_>) -> AdvisoryInputs> {
    ReplayAdvisoryPolicy {
        sched: cfg.policy.clone(),
        advisory: AdvisoryConfig::new("policy-v1", "features-v1", vec!["model-a".into()]),
        health,
        // An oracle advisor that is right about the current state of every tracked account and
        // calls odd borrowers `high`.
        decisions: |input: &mev_liq_sched::ScheduleInput<'_>| {
            let mut out = AdvisoryInputs::default();
            for a in input.accounts {
                let Some(s) = &a.snapshot else { continue };
                let n = a.key.borrower.as_slice()[0];
                out.exports.insert(a.key, (s.revision, digest(n)));
                out.decisions.push(OptionalDecision {
                    key: a.key,
                    chain_id: a.key.chain_id,
                    pool: a.key.pool,
                    policy_version: "policy-v1".into(),
                    feature_schema: "features-v1".into(),
                    revision: s.revision,
                    feature_digest: digest(n),
                    expires_at_ns: u64::MAX,
                    model: "model-a".into(),
                    watch_tier: if n % 2 == 1 {
                        WatchTier::High
                    } else {
                        WatchTier::Normal
                    },
                    confidence: 0.95,
                    prep_probability: 0.5,
                    value_score: 1.0,
                });
            }
            out
        },
    }
}

#[test]
fn replay_jev_has_equal_budgets_and_unchanged_mandatory_metrics() {
    let cfg = config();
    let s = scenario();
    let base = replay(&s, &cfg, "test").unwrap();
    let jev = replay_with_policy(
        &s,
        &cfg,
        "test",
        &advisory_policy(&cfg, JevHealth::Healthy),
        "REPLAY_JEV",
    )
    .unwrap();

    assert_eq!(base.policy, "BASELINE");
    assert_eq!(jev.policy, "REPLAY_JEV");
    // same stream, same budget model
    assert_eq!(base.stream_digest, jev.stream_digest);
    assert_eq!(base.policy_config, jev.policy_config);
    assert_eq!(base.tick_ns, jev.tick_ns);
    assert_eq!(
        base.dispatched.p0 + base.dispatched.p1 + base.dispatched.p2 + base.dispatched.p3,
        jev.dispatched.p0 + jev.dispatched.p1 + jev.dispatched.p2 + jev.dispatched.p3
    );
    assert_eq!(base.modeled_account_reads, jev.modeled_account_reads);
    assert_eq!(base.modeled_busy_ns, jev.modeled_busy_ns);
    // mandatory metrics unchanged
    assert_eq!(base.dispatched.p0, jev.dispatched.p0);
    assert_eq!(base.dispatched.p1, jev.dispatched.p1);
    assert_eq!(base.peak_mandatory_backlog, jev.peak_mandatory_backlog);
    assert_eq!(
        base.mandatory_deadline_miss_observations,
        jev.mandatory_deadline_miss_observations
    );
    assert_eq!(
        base.unique_mandatory_deadline_misses,
        jev.unique_mandatory_deadline_misses
    );
    assert_eq!(
        base.coverage_violation_observations,
        jev.coverage_violation_observations
    );
    assert_eq!(base.detected_episodes, jev.detected_episodes);
    assert_eq!(
        base.eligibility_detection_latency,
        jev.eligibility_detection_latency
    );
    let mandatory = |r: &mev_liq_replay::ReplayReport| {
        r.ordered_checks
            .iter()
            .filter(|c| c.tier <= Tier::P1)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(mandatory(&base), mandatory(&jev));
    // and the experiment is not vacuous: the discretionary order did change
    assert!(base.dispatched.p2 > 0);
    assert_ne!(base.ordered_checks, jev.ordered_checks);
}

#[test]
fn an_unhealthy_jev_replays_exactly_as_the_baseline() {
    let cfg = config();
    let s = scenario();
    let base = replay(&s, &cfg, "test").unwrap();
    for h in [JevHealth::Degraded, JevHealth::Open] {
        let jev =
            replay_with_policy(&s, &cfg, "test", &advisory_policy(&cfg, h), "BASELINE").unwrap();
        assert_eq!(base, jev);
    }
}
