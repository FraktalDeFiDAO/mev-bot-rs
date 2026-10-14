//! JEV-LIQ-011: advisory policy validation (shadow only). The core property: no decision
//! payload, however adversarial, changes anything mandatory.

mod common;

use alloy::primitives::{Address, B256};
use common::*;
use mev_liq_sched::*;
use mev_liq_state::{DirtyEntry, DirtyReason, WatchTier};
use proptest::prelude::*;

fn digest_of(n: u32) -> B256 {
    B256::repeat_byte(0xE0 | (n % 16) as u8)
}

fn exports(ns: &[u32]) -> ExportView {
    ns.iter().map(|n| (key(*n), (1, digest_of(*n)))).collect()
}

fn good(n: u32) -> OptionalDecision {
    good_decision(n, 1, digest_of(n))
}

struct W {
    accounts: Vec<AccountState>,
    dirty: Vec<DirtyEntry>,
}

/// `(n, event_age_s)`: healthy covered account with one unreflected `Discovered` event, a P2 job
/// due at `NOW - event_age_s + p2_deadline`.
fn p2_world(spec: &[(u32, u64)]) -> W {
    W {
        accounts: spec.iter().map(|(n, _)| healthy(*n, 10)).collect(),
        dirty: spec
            .iter()
            .map(|(n, age)| dirty(*n, &[DirtyReason::Discovered], 150, NOW - age * SEC))
            .collect(),
    }
}

fn input<'a>(w: &'a W, dec: &'a [OptionalDecision]) -> ScheduleInput<'a> {
    ScheduleInput {
        now_ns: NOW,
        accounts: &w.accounts,
        dirty: &w.dirty,
        urgent_events: &[],
        pools: &[],
        in_flight: &[],
        optional_decisions: dec,
    }
}

fn cfg1() -> PolicyConfig {
    PolicyConfig {
        concurrency: 1,
        ..PolicyConfig::default()
    }
}

fn wb(w: &W, dec: &[OptionalDecision], conc: u32, health: JevHealth) -> WouldBe {
    let cfg = PolicyConfig {
        concurrency: conc,
        ..PolicyConfig::default()
    };
    let all: Vec<u32> = (0..64).collect();
    would_be_schedule(&input(w, dec), &cfg, &adv_cfg(), health, &exports(&all)).unwrap()
}

fn verdict(b: &WouldBe, n: u32) -> Verdict {
    *b.verdicts.get(&key(n)).expect("P2 job evaluated")
}

fn order(o: &ScheduleOutput) -> Vec<u32> {
    o.jobs
        .iter()
        .map(|j| u32::from_be_bytes(j.key.borrower.as_slice()[16..].try_into().unwrap()))
        .collect()
}

// ---- mode / config ----

#[test]
fn default_mode_is_off_and_active_cannot_be_configured() {
    assert_eq!(AdvisoryMode::default(), AdvisoryMode::Off);
    assert_eq!(AdvisoryMode::parse_config("off"), Ok(AdvisoryMode::Off));
    assert_eq!(
        AdvisoryMode::parse_config("Shadow"),
        Ok(AdvisoryMode::Shadow)
    );
    for s in ["active", "ACTIVE", " active "] {
        let e = AdvisoryMode::parse_config(s).unwrap_err();
        assert_eq!(e, AdvisoryConfigError::ActiveRequiresActivationGuard);
        assert!(e.to_string().contains("JEV-LIQ-019"));
        assert!("active".parse::<AdvisoryMode>().is_err());
    }
    assert!(matches!(
        AdvisoryMode::parse_config("on"),
        Err(AdvisoryConfigError::UnknownMode(_))
    ));
    // The variant exists, but the policy refuses to be built with it.
    let e = AdvisoryPolicy::new(
        AdvisoryMode::Active,
        PolicyConfig::default(),
        adv_cfg(),
        JevHealth::Healthy,
    )
    .unwrap_err();
    assert_eq!(
        e,
        AdvisoryError::Config(AdvisoryConfigError::ActiveRequiresActivationGuard)
    );
}

#[test]
fn config_validation() {
    let mut c = adv_cfg();
    assert!(c.validate().is_ok());
    assert_eq!(c.confidence_threshold, 0.80);
    assert_eq!(c.max_bonus, 20);
    c.max_bonus = 21;
    assert!(c.validate().is_err());
    c = adv_cfg();
    c.confidence_threshold = f64::NAN;
    assert!(c.validate().is_err());
    c.confidence_threshold = 0.0;
    assert!(c.validate().is_err());
    c = adv_cfg();
    c.allowed_models.clear();
    assert!(c.validate().is_err());
    assert_eq!(JevHealth::from_circuit_state(0), JevHealth::Healthy);
    assert_eq!(JevHealth::from_circuit_state(1), JevHealth::Degraded);
    assert_eq!(JevHealth::from_circuit_state(2), JevHealth::Open);
    assert_eq!(JevHealth::from_circuit_state(-7), JevHealth::Open);
}

// ---- each predicate clause maps to its own FallbackReason ----

fn single(mutate: impl FnOnce(&mut OptionalDecision)) -> Verdict {
    let w = p2_world(&[(1, 5)]);
    let mut d = good(1);
    mutate(&mut d);
    verdict(&wb(&w, &[d], 4, JevHealth::Healthy), 1)
}

fn assert_fallback(v: Verdict, r: FallbackReason) {
    assert_eq!(v.fallback, Some(r));
    assert_eq!(v.bonus, 0);
}

#[test]
fn valid_decision_earns_a_bonus() {
    let v = single(|_| {});
    assert_eq!(v.fallback, None);
    assert!(v.bonus >= 1 && v.bonus <= 20);
    assert_eq!(v.applied_tier, Some("high"));
}

#[test]
fn each_clause_has_its_own_fallback_reason() {
    assert_fallback(
        single(|d| d.policy_version = "policy-v0".into()),
        FallbackReason::PolicyVersionMismatch,
    );
    assert_fallback(
        single(|d| d.feature_schema = "features-v0".into()),
        FallbackReason::FeatureSchemaMismatch,
    );
    assert_fallback(single(|d| d.revision = 0), FallbackReason::RevisionMismatch);
    assert_fallback(single(|d| d.revision = 2), FallbackReason::RevisionMismatch);
    assert_fallback(
        single(|d| d.feature_digest = B256::repeat_byte(1)),
        FallbackReason::DigestMismatch,
    );
    assert_fallback(single(|d| d.chain_id = 1), FallbackReason::ChainMismatch);
    assert_fallback(
        single(|d| d.pool = Address::repeat_byte(0x11)),
        FallbackReason::PoolMismatch,
    );
    assert_fallback(single(|d| d.expires_at_ns = NOW), FallbackReason::Expired);
    assert_fallback(single(|d| d.expires_at_ns = 0), FallbackReason::Expired);
    assert_fallback(
        single(|d| d.model = "model-z".into()),
        FallbackReason::ModelNotAllowed,
    );
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.5, -0.1] {
        assert_fallback(
            single(|d| d.confidence = bad),
            FallbackReason::InvalidAnswer,
        );
        assert_fallback(
            single(|d| d.prep_probability = bad),
            FallbackReason::InvalidAnswer,
        );
    }
    for bad in [f64::NAN, f64::INFINITY, 4.01, -1.0] {
        assert_fallback(
            single(|d| d.value_score = bad),
            FallbackReason::InvalidAnswer,
        );
    }
    assert_fallback(
        single(|d| d.confidence = 0.79),
        FallbackReason::BelowConfidence,
    );
}

#[test]
fn missing_decision_and_missing_export_fail_closed() {
    let w = p2_world(&[(1, 5), (2, 5)]);
    let all = [good(1), good(2)];
    // no decision for account 2
    let b = wb(&w, &all[..1], 4, JevHealth::Healthy);
    assert_fallback(verdict(&b, 2), FallbackReason::NoDecision);
    // no export row for account 1
    let b = would_be_schedule(
        &input(&w, &all),
        &PolicyConfig::default(),
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&[2]),
    )
    .unwrap();
    assert_fallback(verdict(&b, 1), FallbackReason::DigestMismatch);
    assert_eq!(verdict(&b, 2).fallback, None);
}

#[test]
fn jev_health_gates_everything() {
    let w = p2_world(&[(1, 5)]);
    let d = [good(1)];
    assert_fallback(
        verdict(&wb(&w, &d, 4, JevHealth::Degraded), 1),
        FallbackReason::JevDegraded,
    );
    assert_fallback(
        verdict(&wb(&w, &d, 4, JevHealth::Open), 1),
        FallbackReason::JevOpen,
    );
    assert!(verdict(&wb(&w, &d, 4, JevHealth::Healthy), 1).bonus > 0);
}

#[test]
fn stale_or_out_of_order_decision_is_rejected() {
    // snapshot is at revision 2; a decision computed for revision 1 arrives afterwards
    let mut w = p2_world(&[(1, 5)]);
    let mut s = snap(1, hf(3000), 100, 100, 10);
    s.revision = 2;
    w.accounts[0] = AccountState::new(key(1), Some(s));
    let d = [good_decision(1, 1, digest_of(1))];
    let ex: ExportView = [(key(1), (2, digest_of(1)))].into();
    let b = would_be_schedule(
        &input(&w, &d),
        &PolicyConfig::default(),
        &adv_cfg(),
        JevHealth::Healthy,
        &ex,
    )
    .unwrap();
    assert_fallback(verdict(&b, 1), FallbackReason::RevisionMismatch);
    // a decision for a revision AHEAD of the snapshot is rejected too
    let d = [good_decision(1, 3, digest_of(1))];
    let b = would_be_schedule(
        &input(&w, &d),
        &PolicyConfig::default(),
        &adv_cfg(),
        JevHealth::Healthy,
        &ex,
    )
    .unwrap();
    assert_fallback(verdict(&b, 1), FallbackReason::RevisionMismatch);
    // an export lagging the snapshot (export at revision 1, snapshot at 2) is also stale
    let d = [good_decision(1, 2, digest_of(1))];
    let ex: ExportView = [(key(1), (1, digest_of(1)))].into();
    let b = would_be_schedule(
        &input(&w, &d),
        &PolicyConfig::default(),
        &adv_cfg(),
        JevHealth::Healthy,
        &ex,
    )
    .unwrap();
    assert_fallback(verdict(&b, 1), FallbackReason::RevisionMismatch);
}

#[test]
fn only_p2_jobs_are_evaluated_and_p0_ignores_a_perfect_decision() {
    // account 1: verified eligible (P0); account 2: P2
    let mut w = p2_world(&[(2, 5)]);
    w.accounts.push(AccountState::new(
        key(1),
        Some(snap(1, hf(900), 100, 100, 10)),
    ));
    let d = [good(1), good(2)];
    let b = wb(&w, &d, 4, JevHealth::Healthy);
    assert!(!b.verdicts.contains_key(&key(1)));
    assert!(
        b.output
            .jobs
            .iter()
            .filter(|j| j.decision.tier != Tier::P2)
            .all(|j| j.decision.advisory_bonus == 0)
    );
    assert!(verdict(&b, 2).bonus > 0);
}

// ---- bonus rule ----

#[test]
fn normal_and_low_never_earn_or_lose_anything() {
    let c = adv_cfg();
    for t in [WatchTier::Normal, WatchTier::Low] {
        for conf in [0.0, 0.5, 0.8, 1.0, f64::NAN] {
            assert_eq!(bonus_for(t, conf, &c), 0);
        }
    }
    let v = single(|d| d.watch_tier = WatchTier::Low);
    assert_eq!((v.bonus, v.fallback), (0, None));
}

#[test]
fn bonus_is_bounded_monotonic_and_positive_at_threshold() {
    let c = adv_cfg();
    assert_eq!(bonus_for(WatchTier::High, 0.7999, &c), 0);
    assert!(bonus_for(WatchTier::High, 0.80, &c) >= 1);
    assert_eq!(bonus_for(WatchTier::High, 1.0, &c), 20);
    let mut last = 0;
    for i in 0..=1000 {
        let b = bonus_for(WatchTier::High, f64::from(i) / 1000.0, &c);
        assert!(b >= last && b <= 20);
        last = b;
    }
    let mut c2 = adv_cfg();
    c2.max_bonus = 3;
    assert_eq!(bonus_for(WatchTier::High, 1.0, &c2), 3);
    c2.max_bonus = 0;
    assert_eq!(bonus_for(WatchTier::High, 1.0, &c2), 0);
}

proptest! {
    #[test]
    fn bonus_for_is_total_and_monotone(a in any::<f64>(), b in any::<f64>(), max in 0u8..=255, thr in 0.01f64..=1.0) {
        let mut c = adv_cfg();
        c.max_bonus = max.min(20);
        c.confidence_threshold = thr;
        for t in [WatchTier::High, WatchTier::Normal, WatchTier::Low] {
            let x = bonus_for(t, a, &c);
            prop_assert!(x <= c.max_bonus);
            if t != WatchTier::High { prop_assert_eq!(x, 0); }
        }
        if a.is_finite() && b.is_finite() && a <= b {
            prop_assert!(bonus_for(WatchTier::High, a, &c) <= bonus_for(WatchTier::High, b, &c));
        }
    }
}

// ---- ordering inside P2 ----

#[test]
fn bonus_breaks_ties_between_equal_deadlines_only() {
    // equal events: baseline order is by key, so account 3 is last
    let w = p2_world(&[(1, 5), (2, 5), (3, 5)]);
    let mut d3 = good(3);
    d3.confidence = 0.99;
    let mut d2 = good(2);
    d2.confidence = 0.85;
    let dec = [d2, d3];
    let base = schedule(&input(&w, &dec), &cfg1()).unwrap();
    assert_eq!(order(&base), vec![1]);
    let all: Vec<u32> = (0..8).collect();
    let b = would_be_schedule(
        &input(&w, &dec),
        &cfg1(),
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_eq!(order(&b.output), vec![3]);
    assert!(verdict(&b, 3).bonus > verdict(&b, 2).bonus);
    // wider budget: full would-be order is bonus desc then key
    let cfg = PolicyConfig {
        concurrency: 8,
        ..PolicyConfig::default()
    };
    let b = would_be_schedule(
        &input(&w, &dec),
        &cfg,
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_eq!(order(&b.output), vec![3, 2, 1]);
}

#[test]
fn bonus_never_overtakes_an_earlier_deadline() {
    // account 1 is older (earlier due) and has no decision; account 2 is newer with a perfect one
    let w = p2_world(&[(1, 30), (2, 5)]);
    let dec = [good(2)];
    let all: Vec<u32> = (0..8).collect();
    let cfg = PolicyConfig {
        concurrency: 8,
        ..PolicyConfig::default()
    };
    let b = would_be_schedule(
        &input(&w, &dec),
        &cfg,
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_eq!(order(&b.output), vec![1, 2]);
    let b1 = would_be_schedule(
        &input(&w, &dec),
        &cfg1(),
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_eq!(order(&b1.output), vec![1]);
}

#[test]
fn aged_p2_job_gets_no_bonus_and_is_not_overtaken() {
    // aging_ns = 120 s, p2 deadline 60 s: an event 200 s old is due 140 s ago (aged)
    let w = p2_world(&[(1, 200), (2, 200), (3, 5)]);
    let dec = [good(2), good(3)];
    let all: Vec<u32> = (0..8).collect();
    let cfg = PolicyConfig {
        concurrency: 8,
        ..PolicyConfig::default()
    };
    let b = would_be_schedule(
        &input(&w, &dec),
        &cfg,
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_fallback(verdict(&b, 2), FallbackReason::AgedJob);
    assert_eq!(order(&b.output), vec![1, 2, 3]);
    assert!(
        b.output
            .jobs
            .iter()
            .filter(|j| j.key == key(2))
            .all(|j| j.decision.advisory_bonus == 0)
    );
    // with one slot the aged job still goes first
    let b = would_be_schedule(
        &input(&w, &dec),
        &cfg1(),
        &adv_cfg(),
        JevHealth::Healthy,
        &exports(&all),
    )
    .unwrap();
    assert_eq!(order(&b.output), vec![1]);
}

// ---- shadow semantics ----

#[test]
fn shadow_dispatches_baseline_and_reports_the_would_be_order() {
    let w = p2_world(&[(1, 5), (2, 5), (3, 5)]);
    let mut d3 = good(3);
    d3.confidence = 0.99;
    let mut stale = good(2);
    stale.expires_at_ns = NOW - 1;
    let dec = [stale, d3];
    let all: Vec<u32> = (0..8).collect();
    let inp = input(&w, &dec);
    let policy =
        AdvisoryPolicy::new(AdvisoryMode::Shadow, cfg1(), adv_cfg(), JevHealth::Healthy).unwrap();
    let run = policy.run(&inp, &exports(&all)).unwrap();
    let base = schedule(&inp, &cfg1()).unwrap();
    assert_eq!(
        serde_json::to_string(&run.dispatch).unwrap(),
        serde_json::to_string(&base).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&RankingPolicy::schedule(&policy, &inp).unwrap()).unwrap(),
        serde_json::to_string(&base).unwrap()
    );
    let r = run.report.unwrap();
    assert_eq!(r.mode, AdvisoryMode::Shadow);
    assert_eq!(r.p2_jobs, 3);
    assert_eq!(r.baseline_p2_order.len(), 1);
    assert_ne!(r.baseline_p2_order, r.would_be_p2_order);
    assert!(r.order_changed && r.mandatory_unchanged);
    assert_eq!(r.applied.len(), 1);
    assert_eq!(r.expired_decisions, 1);
    assert_eq!(r.stale_decisions, 1);
    assert_eq!(r.counters.copper_jev_stale_response_total, 1);
    assert_eq!(
        r.counters.copper_jev_fallback_total.get("expired"),
        Some(&1)
    );
    assert_eq!(
        r.counters.copper_jev_fallback_total.get("no_decision"),
        Some(&1)
    );
    assert_eq!(
        r.counters.copper_jev_applied_bonus_total.get("high"),
        Some(&1)
    );
    // no borrower identifier appears in any metric label
    let labels = serde_json::to_string(&r.counters).unwrap();
    assert!(!labels.contains("0x"));
}

#[test]
fn off_mode_reports_nothing() {
    let w = p2_world(&[(1, 5)]);
    let dec = [good(1)];
    let policy =
        AdvisoryPolicy::new(AdvisoryMode::Off, cfg1(), adv_cfg(), JevHealth::Healthy).unwrap();
    let run = policy.run(&input(&w, &dec), &exports(&[1])).unwrap();
    assert!(run.report.is_none());
}

// ---- the core invariant, over arbitrary worlds and adversarial payloads ----

#[derive(Debug, Clone)]
struct Spec {
    n: u32,
    hf_milli: u64,
    age_s: u64,
    snapshot: bool,
    reasons: Vec<u8>,
    event_age_s: u64,
    cohort: u64,
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        0u32..30,
        prop_oneof![Just(900u64), Just(1040), 1100u64..4000],
        0u64..400,
        prop::bool::weighted(0.9),
        prop::collection::vec(0u8..5, 0..3),
        (0u64..8).prop_map(|x| x * 20),
        0u64..3,
    )
        .prop_map(
            |(n, hf_milli, age_s, snapshot, reasons, event_age_s, cohort)| Spec {
                n,
                hf_milli,
                age_s,
                snapshot,
                reasons,
                event_age_s,
                cohort,
            },
        )
}

fn reason(c: u8) -> DirtyReason {
    match c {
        0 => DirtyReason::Discovered,
        1 => DirtyReason::Manual,
        2 => DirtyReason::PriceMove,
        3 => DirtyReason::BorrowerEvent,
        _ => DirtyReason::Stale,
    }
}

fn build(specs: &[Spec]) -> W {
    let mut w = W {
        accounts: vec![],
        dirty: vec![],
    };
    for s in specs {
        let sn = s
            .snapshot
            .then(|| snap(s.n, hf(s.hf_milli), 100, 100, s.age_s));
        let mut a = AccountState::new(key(s.n), sn);
        a.cohort = s.cohort;
        w.accounts.push(a);
        if !s.reasons.is_empty() {
            let rs: Vec<_> = s.reasons.iter().map(|r| reason(*r)).collect();
            w.dirty
                .push(dirty(s.n, &rs, 150, NOW - s.event_age_s * SEC));
        }
    }
    w
}

/// Decision for account `n`, derived from reality then corrupted according to `m`.
fn adversarial(n: u32, m: u8, conf: f64, extra: u64) -> OptionalDecision {
    let mut d = good(n);
    d.confidence = conf;
    match m % 20 {
        0 => {}
        1 => d.revision += 1 + extra % 5,
        2 => d.revision = d.revision.saturating_sub(1),
        3 => d.feature_digest = B256::repeat_byte(extra as u8),
        4 => d.chain_id = d.chain_id.wrapping_add(1 + extra),
        5 => d.pool = Address::repeat_byte(extra as u8),
        6 => d.expires_at_ns = extra % NOW,
        7 => d.model = format!("evil-{extra}"),
        8 => d.confidence = f64::NAN,
        9 => d.confidence = f64::INFINITY,
        10 => d.confidence = 1.0e300,
        11 => d.value_score = f64::NAN,
        12 => d.prep_probability = -1.0e9,
        13 => d.policy_version = "other".into(),
        14 => d.feature_schema = "other".into(),
        15 => d.watch_tier = WatchTier::Normal,
        16 => d.watch_tier = WatchTier::Low,
        17 => d.expires_at_ns = u64::MAX,
        18 => d.key = key(n + 1000), // filed under an unknown account
        _ => d.revision = u64::MAX,
    }
    d
}

fn mandatory(o: &ScheduleOutput) -> String {
    let jobs: Vec<_> = o
        .jobs
        .iter()
        .filter(|j| j.decision.tier <= Tier::P1)
        .collect();
    let miss: Vec<_> = o
        .deadline_misses
        .iter()
        .filter(|m| m.tier <= Tier::P1)
        .collect();
    serde_json::to_string(&(
        jobs,
        &o.backlog,
        miss,
        &o.coverage_violations,
        &o.candidates,
        &o.stale_rejected,
        o.free_slots,
        o.running,
        o.not_ready,
    ))
    .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn decisions_cannot_touch_anything_mandatory(
        specs in prop::collection::vec(spec(), 0..40),
        muts in prop::collection::vec((0u32..30, any::<u8>(), 0.0f64..=1.0, any::<u64>()), 0..50),
        conc in 1u32..20,
        health in 0u8..3,
        have_export in prop::collection::vec(any::<bool>(), 30),
    ) {
        let w = build(&specs);
        let dec: Vec<OptionalDecision> =
            muts.iter().map(|(n, m, c, e)| adversarial(*n, *m, *c, *e)).collect();
        let health = match health { 0 => JevHealth::Healthy, 1 => JevHealth::Degraded, _ => JevHealth::Open };
        let cfg = PolicyConfig { concurrency: conc, ..PolicyConfig::default() };
        let ex: ExportView = (0..30u32)
            .filter(|n| have_export[*n as usize])
            .map(|n| (key(n), (1, digest_of(n))))
            .collect();

        let base = schedule(&input(&w, &[]), &cfg).unwrap();
        // the baseline function ignores the payload entirely
        let base_with = schedule(&input(&w, &dec), &cfg).unwrap();
        prop_assert_eq!(serde_json::to_string(&base).unwrap(), serde_json::to_string(&base_with).unwrap());

        // Shadow: the full dispatch output is byte-identical
        let policy = AdvisoryPolicy::new(AdvisoryMode::Shadow, cfg.clone(), adv_cfg(), health).unwrap();
        let run = policy.run(&input(&w, &dec), &ex).unwrap();
        prop_assert_eq!(serde_json::to_string(&run.dispatch).unwrap(), serde_json::to_string(&base).unwrap());
        let report = run.report.unwrap();
        prop_assert!(report.mandatory_unchanged);

        // The would-be ordering changes nothing mandatory, in any input shape
        let wb = would_be_schedule(&input(&w, &dec), &cfg, &adv_cfg(), health, &ex).unwrap();
        prop_assert_eq!(mandatory(&base), mandatory(&wb.output));
        // P0/P1 jobs come first and in the same order; none carries a bonus
        let pre = |o: &ScheduleOutput| o.jobs.iter().filter(|j| j.decision.tier <= Tier::P1).cloned().collect::<Vec<_>>();
        prop_assert_eq!(pre(&base), pre(&wb.output));
        prop_assert_eq!(base.jobs.len(), wb.output.jobs.len());
        prop_assert_eq!(base.deferred_discretionary, wb.output.deferred_discretionary);
        for j in wb.output.jobs.iter().chain(&wb.output.backlog) {
            prop_assert!(j.decision.advisory_bonus <= MAX_ADVISORY_BONUS);
            if j.decision.tier != Tier::P2 { prop_assert_eq!(j.decision.advisory_bonus, 0); }
            // a job past its aging threshold never gets a bonus
            if NOW.saturating_sub(j.decision.earliest_due) >= cfg.aging_ns {
                prop_assert_eq!(j.decision.advisory_bonus, 0);
            }
        }
        // every dispatched job is also a candidate of the baseline: nothing invented
        let known: std::collections::BTreeSet<_> = base.jobs.iter().chain(&base.backlog).map(|j| j.key).collect();
        prop_assert!(wb.output.jobs.iter().filter(|j| j.decision.tier <= Tier::P1).all(|j| known.contains(&j.key)));
        // verdicts exist for P2 jobs only; unhealthy Jev means no bonus anywhere
        if health != JevHealth::Healthy {
            prop_assert!(wb.output.jobs.iter().all(|j| j.decision.advisory_bonus == 0));
            prop_assert_eq!(serde_json::to_string(&base).unwrap(), serde_json::to_string(&wb.output).unwrap());
        }
        // within the P2/P3 lane, each cohort still moves oldest-first (strict aging order)
        let lane: Vec<_> = wb.output.jobs.iter().filter(|j| j.decision.tier >= Tier::P2).collect();
        for c in 0..3u64 {
            let dues: Vec<_> = lane.iter().filter(|j| j.cohort == c).map(|j| (j.decision.earliest_due, j.decision.tier)).collect();
            prop_assert!(dues.windows(2).all(|w| w[0] <= w[1]));
        }
        // the report's counters add up
        let fb: u64 = report.counters.copper_jev_fallback_total.values().sum();
        let applied_or_zero = report.p2_jobs - fb;
        prop_assert!(report.applied.len() as u64 <= applied_or_zero);
    }
}
