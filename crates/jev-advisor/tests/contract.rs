mod common;

use common::fixture;
use jev_advisor::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn policy() -> ResponsePolicy {
    ResponsePolicy::new(vec!["jev-1.13.0".to_string()])
}

fn parse(name: &str) -> Result<AdvisoryAnswer, TransportError> {
    parse_response(&fixture(name), &policy())
}

#[test]
fn valid_fixture_parses() {
    let a = parse("valid.json").unwrap();
    assert_eq!(a.watch_tier.choice, WatchTier::High);
    assert_eq!(a.monitoring_value.probabilities, [0.0, 0.1, 0.3, 0.5, 0.1]);
}

#[test]
fn every_invalid_fixture_is_rejected_with_its_category() {
    use TransportError::*;
    use ValidationError as V;
    type Check = fn(&TransportError) -> bool;
    let table: Vec<(&str, Check)> = vec![
        ("wrong_model.json", |e| matches!(e, WrongModel { .. })),
        ("probs_not_sum.json", |e| {
            matches!(e, SchemaDrift(V::DistributionSum("watch_tier")))
        }),
        ("probs_negative.json", |e| {
            matches!(e, SchemaDrift(V::OutOfRange(_)))
        }),
        ("probs_over_one.json", |e| {
            matches!(e, SchemaDrift(V::OutOfRange(_)))
        }),
        ("probs_string_nan.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("probs_extra_key.json", |e| {
            matches!(e, SchemaDrift(V::BadProbabilityKeys(_)))
        }),
        ("nan_literal.json", |e| matches!(e, MalformedJson)),
        ("inf_literal.json", |e| {
            matches!(e, SchemaDrift(_) | MalformedJson)
        }),
        ("missing_answer.json", |e| {
            matches!(e, SchemaDrift(V::MissingAnswer("extra_preparation")))
        }),
        ("extra_answer.json", |e| {
            matches!(e, SchemaDrift(V::UnexpectedAnswer(_)))
        }),
        (
            "dup_answer.json",
            |e| matches!(e, SchemaDrift(V::DuplicateKey(k)) if k == "watch_tier"),
        ),
        ("dup_probability_key.json", |e| {
            matches!(e, SchemaDrift(V::DuplicateKey(_)))
        }),
        ("dup_top_level_model.json", |e| {
            matches!(e, SchemaDrift(V::DuplicateKey(_)))
        }),
        ("unknown_type.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("wrong_type_for_id.json", |e| {
            matches!(
                e,
                SchemaDrift(V::WrongAnswerType {
                    id: "extra_preparation",
                    ..
                })
            )
        }),
        ("unknown_choice.json", |e| {
            matches!(e, SchemaDrift(V::UnknownChoice(_)))
        }),
        ("confidence_range.json", |e| {
            matches!(e, SchemaDrift(V::OutOfRange("watch_tier.confidence")))
        }),
        ("noul_bool.json", |e| matches!(e, SchemaDrift(V::Schema(_)))),
        ("noul_out_of_range.json", |e| {
            matches!(e, SchemaDrift(V::OutOfRange("extra_preparation")))
        }),
        ("score_out_of_range.json", |e| {
            matches!(e, SchemaDrift(V::OutOfRange("monitoring_value.score")))
        }),
        ("score_inconsistent.json", |e| {
            matches!(e, SchemaDrift(V::ScoreInconsistent))
        }),
        ("score_probs_len4.json", |e| {
            matches!(e, SchemaDrift(V::BadProbabilityKeys("monitoring_value")))
        }),
        ("score_probs_one_based.json", |e| {
            matches!(e, SchemaDrift(V::BadProbabilityKeys(_)))
        }),
        ("legend_bad.json", |e| {
            matches!(e, SchemaDrift(V::BadLegend))
        }),
        ("extra_top_level.json", |e| {
            matches!(e, SchemaDrift(V::UnknownTopLevelKey(_)))
        }),
        ("extra_answer_field.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("missing_usage.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("negative_tokens.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("schema_drift.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
        ("malformed.json", |e| matches!(e, MalformedJson)),
        ("not_an_object.json", |e| {
            matches!(e, SchemaDrift(V::Schema(_)))
        }),
    ];
    for (name, check) in table {
        let e = parse(name).expect_err(name);
        assert!(check(&e), "{name} -> {e:?}");
    }
}

#[test]
fn strictness_and_tolerance_are_configurable() {
    let mut p = policy();
    p.unknown_top_level = UnknownTopLevel::Ignore;
    assert!(parse_response(&fixture("extra_top_level.json"), &p).is_ok());
    // Extra top-level keys are the only thing relaxed: answers stay strict.
    assert!(parse_response(&fixture("extra_answer_field.json"), &p).is_err());

    assert!(parse("probs_not_sum.json").is_err());
    let mut p = policy();
    p.prob_sum_tolerance = 0.25; // sum is 1.2
    assert!(parse_response(&fixture("probs_not_sum.json"), &p).is_ok());
    p.prob_sum_tolerance = f64::NAN;
    assert!(matches!(
        parse_response(&fixture("valid.json"), &p),
        Err(TransportError::Config(_))
    ));
}

#[test]
fn digest_covers_the_exact_bytes() {
    let a = parse("valid.json").unwrap();
    let mut body = fixture("valid.json");
    body.push(b' '); // trailing whitespace is still valid JSON but a different response
    let b = parse_response(&body, &policy()).unwrap();
    assert_ne!(a.response_digest, b.response_digest);
}

// ---- property test: mutated responses never pass unless they satisfy the contract -------------

/// Independent oracle for the documented contract, written over `serde_json::Value`.
fn satisfies_contract(v: &Value) -> bool {
    let obj_keys = |v: &Value, keys: &[&str]| {
        v.as_object()
            .is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
    };
    let unit = |v: &Value| {
        v.as_f64()
            .is_some_and(|x| x.is_finite() && (0.0..=1.0).contains(&x))
    };
    let dist = |v: &Value, keys: &[&str]| -> Option<Vec<f64>> {
        if !obj_keys(v, keys) {
            return None;
        }
        let ps: Vec<f64> = keys
            .iter()
            .map(|k| v[*k].as_f64().unwrap_or(f64::NAN))
            .collect();
        let ok = ps.iter().all(|p| p.is_finite() && (0.0..=1.0).contains(p))
            && (ps.iter().sum::<f64>() - 1.0).abs() <= 1e-3;
        ok.then_some(ps)
    };
    if !obj_keys(v, &["model", "answers", "usage"]) {
        return false;
    }
    if v["model"] != json!("jev-1.13.0") {
        return false;
    }
    if !(v["usage"].is_object()
        && v["usage"].as_object().unwrap().len() == 2
        && v["usage"]["input_tokens"].is_u64()
        && v["usage"]["output_tokens"].is_u64())
    {
        return false;
    }
    let a = &v["answers"];
    if !obj_keys(a, &["watch_tier", "extra_preparation", "monitoring_value"]) {
        return false;
    }
    let w = &a["watch_tier"];
    if !(obj_keys(w, &["type", "choice", "probabilities", "confidence"])
        && w["type"] == "choice"
        && matches!(w["choice"].as_str(), Some("high" | "normal" | "low"))
        && unit(&w["confidence"])
        && dist(&w["probabilities"], &["high", "normal", "low"]).is_some())
    {
        return false;
    }
    let n = &a["extra_preparation"];
    if !(obj_keys(n, &["type", "noul"]) && n["type"] == "noul" && unit(&n["noul"])) {
        return false;
    }
    let s = &a["monitoring_value"];
    let has_legend = s.get("legend").is_some();
    let keys: &[&str] = if has_legend {
        &["type", "score", "legend", "probabilities", "confidence"]
    } else {
        &["type", "score", "probabilities", "confidence"]
    };
    if !(obj_keys(s, keys) && s["type"] == "score" && unit(&s["confidence"])) {
        return false;
    }
    let Some(ps) = dist(&s["probabilities"], &["0", "1", "2", "3", "4"]) else {
        return false;
    };
    let Some(score) = s["score"].as_f64() else {
        return false;
    };
    let expected: f64 = ps.iter().enumerate().map(|(i, p)| i as f64 * p).sum();
    if !(score.is_finite() && (0.0..=4.0).contains(&score) && (score - expected).abs() <= 0.1) {
        return false;
    }
    if has_legend {
        let l = &s["legend"];
        if !(obj_keys(l, &["0", "1", "2", "3", "4"])
            && l.as_object().unwrap().values().all(Value::is_string))
        {
            return false;
        }
    }
    true
}

fn paths(v: &Value, prefix: String, out: &mut Vec<String>) {
    out.push(prefix.clone());
    if let Some(o) = v.as_object() {
        for (k, c) in o {
            paths(c, format!("{prefix}/{k}"), out);
        }
    }
}

fn pool() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!("x"),
        json!("high"),
        json!("NaN"),
        json!(-1),
        json!(0),
        json!(0.5),
        json!(1),
        json!(1.5),
        json!(2),
        json!(4),
        json!(5),
        json!(1e308),
        json!([]),
        json!({}),
        json!(0.35),
        json!(0.7),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]
    #[test]
    fn mutated_responses_never_pass_unless_valid(
        muts in prop::collection::vec((0usize..64, 0u8..3, 0usize..18), 0..4)
    ) {
        let mut v: Value = serde_json::from_slice(&fixture("valid.json")).unwrap();
        let pool = pool();
        for (pi, op, vi) in muts {
            let mut all = Vec::new();
            paths(&v, String::new(), &mut all);
            let path = &all[pi % all.len()];
            if path.is_empty() { continue; }
            let (parent, key) = path.rsplit_once('/').unwrap();
            let Some(p) = v.pointer_mut(parent).and_then(Value::as_object_mut) else { continue };
            match op {
                0 => { p.remove(key); }
                1 => { p.insert(key.to_string(), pool[vi].clone()); }
                _ => { p.insert(format!("zz{vi}"), pool[vi].clone()); }
            }
        }
        let body = serde_json::to_vec(&v).unwrap();
        let got = parse_response(&body, &policy());
        prop_assert_eq!(got.is_ok(), satisfies_contract(&v), "body: {}", String::from_utf8_lossy(&body));
    }
}
