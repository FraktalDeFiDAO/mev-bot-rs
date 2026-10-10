//! The regression suite on real data. `tests/fixtures/corpus.jsonl` is two minutes of the live sequencer feed
//! (2,049 transactions); `corpus_chain.json` is what the chain says happened in each transaction the decoder reads
//! (receipt status and the pools its events touched), fetched once by `scripts/build-corpus-truth.py`.
//!
//! - invariants that must hold for every transaction, whatever the decoder learns later;
//! - accuracy of the decoder against the chain;
//! - a golden summary (`corpus_golden.json`): a change in what the decoder reads must be deliberate, so a test fails
//!   until it is regenerated (`UPDATE_GOLDEN=1 cargo test -p seqscope-core --test corpus`) and the diff is reviewed;
//! - mutation fuzzing: damaged calldata must never panic or hang;
//! - engine properties over the corpus.

use std::collections::{BTreeMap, HashMap, HashSet};

use alloy::primitives::Bytes;
use proptest::prelude::*;
use seqscope_core::{ActionKind, Engine, Mode, Profile, Row, Rule, RuleSet, Tx, preview};

fn corpus() -> &'static Vec<Tx> {
    static C: std::sync::OnceLock<Vec<Tx>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        include_str!("fixtures/corpus.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).expect("corpus line"))
            .collect()
    })
}

#[derive(serde::Deserialize)]
struct Truth {
    status: Option<u8>,
    events: Vec<TruthEvent>,
}
#[derive(serde::Deserialize)]
struct TruthEvent {
    kind: String,
    pool: String,
}

fn truth() -> &'static HashMap<String, Truth> {
    static T: std::sync::OnceLock<HashMap<String, Truth>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(include_str!("fixtures/corpus_chain.json")).expect("corpus_chain.json")
    })
}

fn rule(name: &str, when: &str, action: ActionKind, sample: u32) -> Rule {
    Rule {
        name: name.into(),
        enabled: true,
        when: when.into(),
        action,
        tag: None,
        sample,
    }
}

fn engine(rules: Vec<Rule>, mode: Mode) -> Engine {
    let set = RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "p".into(),
            enabled: true,
            mode,
            rule: rules,
        }],
    };
    Engine::compile(&set).unwrap_or_else(|e| panic!("{e:?}"))
}

fn all_rows() -> &'static Vec<Row> {
    static R: std::sync::OnceLock<Vec<Row>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let e = engine(vec![rule("all", "true", ActionKind::Raw, 1)], Mode::All);
        corpus().iter().flat_map(|t| e.process(t)).collect()
    })
}

// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_corpus_is_what_it_claims_to_be() {
    let c = corpus();
    assert!(c.len() >= 2000, "{} transactions", c.len());
    let hashes: HashSet<_> = c.iter().map(|t| t.hash).collect();
    assert_eq!(hashes.len(), c.len(), "no duplicate transactions");
    assert!(c.windows(2).all(|w| w[0].seq <= w[1].seq), "in feed order");
    assert!(truth().len() >= 80);
}

#[test]
fn every_transaction_produces_at_least_one_call_and_every_row_is_well_formed() {
    let rows = all_rows();
    let by_hash: HashMap<_, usize> = rows.iter().fold(HashMap::new(), |mut m, r| {
        *m.entry(r.hash).or_default() += 1;
        m
    });
    for t in corpus() {
        assert!(
            by_hash.contains_key(&t.hash),
            "{:#x} produced no call",
            t.hash
        );
    }
    for r in rows {
        assert_eq!(r.status, "pending", "calldata is an intent");
        assert!(
            [
                "swap",
                "liquidity",
                "liquidation",
                "pools",
                "undecoded_known",
                "unknown"
            ]
            .contains(&r.kind.as_str()),
            "{}",
            r.kind
        );
        assert!(
            r.input.as_deref().is_some_and(|i| i.starts_with("0x")),
            "a raw rule keeps the calldata"
        );
        if let Some(i) = &r.input {
            assert_eq!(
                (i.len() - 2) / 2,
                r.len,
                "stored calldata length matches `len`"
            );
        }
        // The decoded fields belong to the kind, and only to it.
        let has = |prefix: &str| r.fields.keys().any(|k| k.starts_with(prefix));
        assert_eq!(has("swap."), r.kind == "swap", "{r:?}");
        assert_eq!(has("liq."), r.kind == "liquidity", "{r:?}");
        assert_eq!(has("liquidation."), r.kind == "liquidation", "{r:?}");
        if r.kind == "swap" {
            for k in ["swap.token_in", "swap.amount_in", "swap.hops"] {
                assert!(r.fields.contains_key(k), "swap row without {k}: {r:?}");
            }
            assert!(r.fields["swap.hops"].parse::<u32>().unwrap() >= 1);
            assert!(
                r.fields["swap.amount_in"]
                    .parse::<alloy::primitives::U256>()
                    .is_ok()
            );
        }
        if r.kind == "liquidity" {
            assert!(["add", "remove"].contains(&r.fields["liq.action"].as_str()));
        }
        if r.kind == "undecoded_known" {
            assert!(
                r.known.is_some(),
                "undecoded_known means an identified contract"
            );
        }
        if r.kind == "unknown" {
            assert!(r.known.is_none(), "unknown means not identified");
        }
        if !r.via.is_empty() {
            assert!(r.to.is_some());
        }
    }
}

#[test]
fn processing_the_corpus_twice_gives_identical_rows() {
    let e = engine(
        vec![
            rule("a", "kind != unknown", ActionKind::Store, 1),
            rule("b", "true", ActionKind::Store, 3),
        ],
        Mode::All,
    );
    let run = || {
        corpus()
            .iter()
            .flat_map(|t| e.process(t))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

// ---------------------------------------------------------------------------------------------------------------
// Accuracy against the chain

#[test]
fn decoded_swaps_name_a_pool_the_transaction_really_swapped_in() {
    let rows = all_rows();
    let (mut checked, mut right, mut wrong) = (0u32, 0u32, Vec::new());
    for r in rows.iter().filter(|r| r.kind == "swap") {
        let Some(pool) = r.fields.get("swap.pool") else {
            continue;
        };
        let Some(t) = truth().get(&format!("{:#x}", r.hash)) else {
            continue;
        };
        if t.status != Some(1) {
            continue; // a reverted transaction swapped nothing; the intent cannot be checked
        }
        let swapped: HashSet<&str> = t
            .events
            .iter()
            .filter(|e| e.kind.starts_with("swap"))
            .map(|e| e.pool.as_str())
            .collect();
        if swapped.is_empty() {
            continue;
        }
        checked += 1;
        if swapped.contains(pool.as_str()) {
            right += 1;
        } else {
            wrong.push(format!(
                "{:#x} {} decoded {pool}, chain swapped in {swapped:?}",
                r.hash,
                r.selector.clone().unwrap_or_default()
            ));
        }
    }
    assert!(
        checked >= 30,
        "only {checked} swaps could be checked: the corpus is too small"
    );
    let acc = f64::from(right) / f64::from(checked);
    eprintln!(
        "swap accuracy against the chain: {right}/{checked} = {:.1}%",
        acc * 100.0
    );
    // Measured when this corpus was recorded; a decoder regression shows up as a drop. Known mismatches are routers
    // that wrap a pool (router B aliases) and multi-pool routes where only the first hop is read.
    assert!(
        acc >= 0.97,
        "accuracy {:.1}% ({right}/{checked}); wrong:\n{}",
        acc * 100.0,
        wrong.join("\n")
    );
}

#[test]
fn decoded_liquidity_changes_match_a_liquidity_event_on_chain() {
    let (mut checked, mut missing) = (0, Vec::new());
    for r in all_rows().iter().filter(|r| r.kind == "liquidity") {
        let Some(t) = truth().get(&format!("{:#x}", r.hash)) else {
            continue;
        };
        if t.status != Some(1) {
            continue;
        }
        checked += 1;
        let on_chain = t.events.iter().any(|e| {
            [
                "mint_v3",
                "burn_v3",
                "modify_v4",
                "increase_nft",
                "decrease_nft",
            ]
            .contains(&e.kind.as_str())
        });
        if !on_chain {
            missing.push(format!(
                "{:#x} {}",
                r.hash,
                r.fields.get("liq.action").cloned().unwrap_or_default()
            ));
        }
    }
    assert!(
        checked >= 5,
        "only {checked} liquidity changes could be checked"
    );
    // A remove with zero liquidity or a collect-only multicall can succeed without a Burn; allow a little.
    eprintln!(
        "liquidity changes with a liquidity event on chain: {}/{checked}",
        checked - missing.len()
    );
    assert!(
        missing.len() <= 1,
        "{} of {checked} decoded liquidity changes have no liquidity event on chain: {missing:?}",
        missing.len()
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Golden summary

fn summary() -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    m.insert("transactions".to_string(), corpus().len() as u64);
    for r in all_rows() {
        *m.entry(format!(
            "{}/{}",
            r.kind,
            r.protocol.as_deref().unwrap_or("-")
        ))
        .or_default() += 1;
        if !r.via.is_empty() {
            *m.entry(format!("wrapped/{}", r.via.join(">"))).or_default() += 1;
        }
    }
    m
}

#[test]
fn what_the_decoder_reads_has_not_changed_by_accident() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/corpus_golden.json"
    );
    let now = summary();
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
        return;
    }
    let want: BTreeMap<String, u64> = serde_json::from_str(
        &std::fs::read_to_string(path).expect("corpus_golden.json; create it with UPDATE_GOLDEN=1"),
    )
    .unwrap();
    assert_eq!(
        now, want,
        "the decoder now reads the corpus differently. If that is the intended improvement, regenerate with UPDATE_GOLDEN=1 and review the diff."
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Robustness

/// A small deterministic generator (no external crate, same output everywhere).
fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[test]
fn damaged_calldata_never_panics_or_hangs() {
    let e = engine(vec![rule("all", "true", ActionKind::Store, 1)], Mode::All);
    let started = std::time::Instant::now();
    let mut s = 0x2545_F491_4F6C_DD1D_u64;
    let mut runs = 0u64;
    for t in corpus() {
        let n = t.input.len();
        for k in 0..12u64 {
            let mut v = t.input.to_vec();
            match k % 4 {
                0 if n > 0 => v.truncate((next(&mut s) as usize) % n), // cut anywhere
                1 if n > 0 => {
                    for _ in 0..1 + (next(&mut s) % 8) {
                        let i = (next(&mut s) as usize) % n;
                        v[i] ^= 1 << (next(&mut s) % 8); // flip bits
                    }
                }
                2 if n > 4 => {
                    for b in v.iter_mut().skip(4).take(64) {
                        *b = 0xff; // huge offsets and lengths
                    }
                }
                3 => v.extend_from_slice(&[0u8; 37]), // trailing junk
                _ => {}
            }
            let mut m = t.clone();
            m.input = Bytes::from(v);
            let _ = e.process(&m);
            runs += 1;
        }
    }
    assert!(runs > 20_000);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(120),
        "damaged input must not make the decoder slow: {:?}",
        started.elapsed()
    );
}

#[test]
fn destinations_that_wrap_calls_survive_arbitrary_bytes() {
    // Every identified contract plus the wrappers, fed random bytes behind each selector we know.
    let mut targets: Vec<alloy::primitives::Address> =
        mev_decode::contracts().iter().map(|c| c.address).collect();
    targets.push(mev_decode::calls::MULTICALL3);
    targets.push(mev_decode::calls::ENTRYPOINT_V6);
    targets.push(mev_decode::calls::ENTRYPOINT_V7);
    let selectors: HashSet<[u8; 4]> = corpus()
        .iter()
        .filter_map(|t| t.input.get(..4).map(|s| [s[0], s[1], s[2], s[3]]))
        .collect();
    let mut s = 0x9E37_79B9_7F4A_7C15_u64;
    let mut runs = 0;
    for to in &targets {
        for sel in selectors.iter().take(150) {
            let len = (next(&mut s) % 400) as usize;
            let mut input = sel.to_vec();
            input.extend((0..len).map(|_| next(&mut s) as u8));
            let _ = mev_decode::calls::actions_in(*to, &input);
            runs += 1;
        }
    }
    assert!(runs > 1_000);
}

// ---------------------------------------------------------------------------------------------------------------
// Engine properties over the corpus

const WHENS: [&str; 10] = [
    "true",
    "kind == swap",
    "kind != unknown",
    "kind == unknown && len >= 4",
    "protocol == uniswap_router",
    "set(swap.pool)",
    "swap.amount_in > 1e15",
    "depth >= 1 || set(via)",
    "selector in [0x04e45aaf, 0xb858183f, 0x3593564c, 0xa9059cbb]",
    "tx_type == 2 && gas_limit > 100000",
];

fn pick(i: usize) -> &'static str {
    WHENS[i % WHENS.len()]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    /// `first` never produces more rows than `all` from the same rules, and its rows are a subset.
    #[test]
    fn first_mode_is_a_subset_of_all_mode(a in 0usize..10, b in 0usize..10, c in 0usize..10) {
        let rules = vec![rule("a", pick(a), ActionKind::Store, 1), rule("b", pick(b), ActionKind::Store, 1), rule("c", pick(c), ActionKind::Store, 1)];
        let all = engine(rules.clone(), Mode::All);
        let first = engine(rules, Mode::First);
        // The kind is part of the identity: one call can yield an exact swap and the pool set it names.
        let key = |r: &Row| (r.hash, r.to.clone(), r.selector.clone(), r.rule.clone(), r.kind.clone());
        for t in corpus().iter().step_by(7) {
            let a_rows: HashSet<_> = all.process(t).iter().map(key).collect();
            let f_rows = first.process(t);
            prop_assert!(f_rows.len() <= a_rows.len());
            for r in &f_rows { prop_assert!(a_rows.contains(&key(r))); }
        }
    }

    /// Sampling only removes rows; a larger N keeps a subset of a smaller N's rows divisible into it.
    #[test]
    fn sampling_only_removes(a in 0usize..10, n in 2u32..9) {
        let full = engine(vec![rule("r", pick(a), ActionKind::Store, 1)], Mode::All);
        let sampled = engine(vec![rule("r", pick(a), ActionKind::Store, n)], Mode::All);
        let (mut f, mut s) = (0usize, 0usize);
        for t in corpus().iter().step_by(3) {
            f += full.process(t).len();
            s += sampled.process(t).len();
        }
        prop_assert!(s <= f);
    }

    /// A first-position `drop` that matches everything silences the profile; a disabled rule is the same as no rule.
    #[test]
    fn drop_all_silences_and_disabled_equals_absent(a in 0usize..10) {
        let silenced = engine(vec![rule("d", "true", ActionKind::Drop, 1), rule("r", pick(a), ActionKind::Store, 1)], Mode::All);
        let mut off = rule("d", "true", ActionKind::Drop, 1);
        off.enabled = false;
        let with_disabled = engine(vec![off, rule("r", pick(a), ActionKind::Store, 1)], Mode::All);
        let without = engine(vec![rule("r", pick(a), ActionKind::Store, 1)], Mode::All);
        for t in corpus().iter().step_by(5) {
            prop_assert!(silenced.process(t).is_empty());
            prop_assert_eq!(with_disabled.process(t), without.process(t));
        }
    }

    /// `!x` selects exactly the calls `x` does not (for expressions with no missing-field cases: kind-based ones).
    #[test]
    fn negation_partitions_the_calls(i in 0usize..3) {
        let w = ["kind == swap", "kind != unknown", "kind == unknown && len >= 4"][i];
        let pos = engine(vec![rule("p", w, ActionKind::Count, 1)], Mode::All);
        let neg = engine(vec![rule("n", &format!("!({w})"), ActionKind::Count, 1)], Mode::All);
        for t in corpus().iter().step_by(11) { pos.process(t); neg.process(t); }
        let calls = pos.calls_seen();
        prop_assert_eq!(pos.stats()[0].matches + neg.stats()[0].matches, calls);
    }

    /// The dry-run preview and a real run of the same rule agree.
    #[test]
    fn preview_agrees_with_a_real_run(a in 0usize..10) {
        let sample: Vec<Tx> = corpus().iter().step_by(4).cloned().collect();
        let p = preview(pick(a), &sample, 3).unwrap();
        let e = engine(vec![rule("r", pick(a), ActionKind::Store, 1)], Mode::All);
        let real: usize = sample.iter().map(|t| e.process(t).len()).sum();
        prop_assert_eq!(p.matched as usize, real);
        prop_assert!(p.samples.len() <= 3);
    }
}
