mod common;

use alloy::primitives::B256;
use common::*;
use mev_liq_sched::*;
use mev_liq_state::{CoverageGap, DirtyEntry, DirtyReason, PoolFlag, ScanDir};
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct Spec {
    n: u32,
    hf_milli: u64,
    debt: u64,
    age_s: u64,
    block: u64,
    snapshot: bool,
    reasons: Vec<u8>,
    event_block: u64,
    event_age_s: u64,
    unreconciled: bool,
    awaiting: bool,
    cohort: u64,
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        (
            0u32..40,
            0u64..4000,
            0u64..3,
            0u64..700,
            50u64..150,
            any::<bool>(),
        ),
        (
            proptest::collection::vec(0u8..7, 0..3),
            40u64..200,
            0u64..120,
            any::<bool>(),
            any::<bool>(),
            0u64..4,
        ),
    )
        .prop_map(|(a, b)| Spec {
            n: a.0,
            hf_milli: a.1,
            debt: a.2,
            age_s: a.3,
            block: a.4,
            snapshot: a.5,
            reasons: b.0,
            event_block: b.1,
            event_age_s: b.2,
            unreconciled: b.3,
            awaiting: b.4,
            cohort: b.5,
        })
}

fn reason(c: u8) -> DirtyReason {
    match c {
        0 => DirtyReason::PriceMove,
        1 => DirtyReason::ReserveConfig,
        2 => DirtyReason::BorrowerEvent,
        3 => DirtyReason::Reorg,
        4 => DirtyReason::Stale,
        5 => DirtyReason::Manual,
        _ => DirtyReason::Discovered,
    }
}

struct World {
    accounts: Vec<AccountState>,
    dirty: Vec<DirtyEntry>,
    pools: Vec<PoolState>,
    in_flight: Vec<InFlight>,
}

fn world(specs: &[Spec], gap: bool, leases: &[u32]) -> World {
    let mut accounts = vec![];
    let mut dirty_v = vec![];
    for s in specs {
        let snap_opt = s
            .snapshot
            .then(|| snap(s.n, hf(s.hf_milli), s.debt, s.block, s.age_s));
        let mut a = AccountState::new(key(s.n), snap_opt);
        a.unreconciled = s.unreconciled;
        a.awaiting_verification_since_ns = s.awaiting.then_some(NOW - SEC);
        a.cohort = s.cohort;
        accounts.push(a);
        if !s.reasons.is_empty() {
            let rs: Vec<_> = s.reasons.iter().map(|r| reason(*r)).collect();
            dirty_v.push(dirty(s.n, &rs, s.event_block, NOW - s.event_age_s * SEC));
        }
    }
    let pools = if gap {
        vec![PoolState {
            chain_id: 42161,
            pool: pool(),
            flags: vec![PoolFlag {
                flag: mev_liq_state::FLAG_BACKLOG_ESCALATED.into(),
                block_number: None,
                detail: String::new(),
                set_ns: NOW - 200 * SEC,
            }],
            gaps: vec![CoverageGap {
                id: 1,
                name: "n".into(),
                dir: ScanDir::Forward,
                from_block: 1,
                to_block: 2,
                reason: "x".into(),
                recorded_ns: NOW - 100 * SEC,
                attempts: 0,
            }],
        }]
    } else {
        vec![]
    };
    World {
        accounts,
        dirty: dirty_v,
        pools,
        in_flight: leases
            .iter()
            .map(|n| InFlight {
                key: key(*n),
                lease_expires_ns: NOW + SEC,
            })
            .collect(),
    }
}

fn exec(w: &World, dec: &[OptionalDecision], cfg: &PolicyConfig) -> ScheduleOutput {
    schedule(
        &ScheduleInput {
            now_ns: NOW,
            accounts: &w.accounts,
            dirty: &w.dirty,
            urgent_events: &[],
            pools: &w.pools,
            in_flight: &w.in_flight,
            optional_decisions: dec,
        },
        cfg,
    )
    .unwrap()
}

fn json(o: &ScheduleOutput) -> String {
    serde_json::to_string(o).unwrap()
}

fn permute<T: Clone>(v: &[T], keys: &[u64]) -> Vec<T> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by_key(|i| keys.get(*i).copied().unwrap_or(0));
    idx.into_iter().map(|i| v[i].clone()).collect()
}

fn small_cfg(conc: u32) -> PolicyConfig {
    PolicyConfig {
        concurrency: conc,
        ..PolicyConfig::default()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Shuffled input order and repeated calls give byte-identical serialized output.
    #[test]
    fn deterministic_under_shuffle_and_repeat(
        specs in proptest::collection::vec(spec(), 0..40),
        gap in any::<bool>(),
        conc in 1u32..20,
        k1 in proptest::collection::vec(any::<u64>(), 64),
        k2 in proptest::collection::vec(any::<u64>(), 64),
        leases in proptest::collection::vec(0u32..40, 0..4),
    ) {
        let w = world(&specs, gap, &leases);
        let cfg = small_cfg(conc);
        let a = json(&exec(&w, &[], &cfg));
        prop_assert_eq!(&a, &json(&exec(&w, &[], &cfg)));
        let shuffled = World {
            accounts: permute(&w.accounts, &k1),
            dirty: permute(&w.dirty, &k2),
            pools: w.pools.clone(),
            in_flight: permute(&w.in_flight, &k1),
        };
        prop_assert_eq!(&a, &json(&exec(&shuffled, &[], &cfg)));
    }

    /// Arbitrary advisory payloads change nothing: the complete output, hence the P0/P1 set,
    /// order and deadlines, equals the no-decision output.
    #[test]
    fn jev_payloads_cannot_influence_the_schedule(
        specs in proptest::collection::vec(spec(), 0..40),
        decs in proptest::collection::vec((0u32..60, any::<u64>(), any::<u8>(), any::<u8>(), any::<u64>()), 0..30),
        conc in 1u32..20,
    ) {
        let w = world(&specs, true, &[]);
        let cfg = small_cfg(conc);
        let dec: Vec<OptionalDecision> = decs.iter().map(|(n, rev, d, bonus, exp)| {
            let mut x = good_decision(*n, *rev, B256::repeat_byte(*d));
            x.confidence = f64::from(*bonus) / 255.0;
            x.expires_at_ns = *exp;
            x
        }).collect();
        let base = exec(&w, &[], &cfg);
        let with = exec(&w, &dec, &cfg);
        prop_assert_eq!(json(&base), json(&with));
        prop_assert!(with.jobs.iter().chain(&with.backlog).all(|j| j.decision.advisory_bonus == 0));
    }

    /// Nothing mandatory is lost: every candidate is dispatched, backlogged, deferred, not ready
    /// or running; dispatch never exceeds free slots; P0/P1 come before discretionary work.
    #[test]
    fn mandatory_work_is_never_dropped(
        specs in proptest::collection::vec(spec(), 0..60),
        conc in 1u32..12,
        leases in proptest::collection::vec(0u32..40, 0..4),
    ) {
        let w = world(&specs, false, &leases);
        let cfg = small_cfg(conc);
        let o = exec(&w, &[], &cfg);
        prop_assert!(o.jobs.len() as u32 <= o.free_slots);
        let total = o.jobs.len() as u64 + o.backlog.len() as u64 + o.deferred_discretionary
            + o.not_ready + o.running;
        prop_assert_eq!(total, o.candidates.p0 + o.candidates.p1 + o.candidates.p2 + o.candidates.p3);
        let tiers: Vec<Tier> = o.jobs.iter().map(|j| j.decision.tier).collect();
        prop_assert!(tiers.windows(2).all(|t| t[0].min(Tier::P2) <= t[1].min(Tier::P2)));
        prop_assert!(o.backlog.iter().all(|j| j.decision.tier <= Tier::P1));
        // an expired snapshot is always mandatory, with or without events
        for s in specs.iter().filter(|s| s.snapshot && s.age_s > 300) {
            let leased = leases.contains(&s.n);
            let dup = specs.iter().filter(|o| o.n == s.n).count() > 1;
            if !leased && !dup {
                let j = o.jobs.iter().chain(&o.backlog).find(|j| j.key == key(s.n));
                prop_assert!(j.is_some_and(|j| j.decision.tier <= Tier::P1), "account {} age {}", s.n, s.age_s);
            }
        }
    }
}
