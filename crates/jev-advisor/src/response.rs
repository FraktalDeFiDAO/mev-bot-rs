//! Strict response parsing and validation. Shapes follow docs.typesafe.ai (verified 2026-10-13,
//! see docs/design/jev-liq-009-client.md): `noul` is a probability number in [0,1], `score` is a
//! number in [0,4] (probability weighted), `probabilities` are maps (choice: option names; score:
//! 0-based string levels). The output carries no raw JSON and no authority: it is advisory.

use std::fmt;

use alloy::primitives::{B256, keccak256};
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};

use crate::error::{TransportError, ValidationError, sanitize};
use crate::request::ANSWER_IDS;

/// Whether keys beyond `model`/`answers`/`usage` are tolerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownTopLevel {
    /// Reject (default: drift is surfaced, advisory is skipped, monitoring continues).
    Reject,
    /// Ignore extra top-level keys (answers/usage stay strict).
    Ignore,
}

#[derive(Debug, Clone)]
pub struct ResponsePolicy {
    pub allowed_models: Vec<String>,
    /// Max |sum(probabilities) - 1|.
    pub prob_sum_tolerance: f64,
    /// Max |score - sum(i * p_i)| (docs: score is the probability-weighted level).
    pub score_tolerance: f64,
    pub unknown_top_level: UnknownTopLevel,
}

impl ResponsePolicy {
    pub fn new(allowed_models: Vec<String>) -> Self {
        Self {
            allowed_models,
            prob_sum_tolerance: 1e-3,
            score_tolerance: 0.1,
            unknown_top_level: UnknownTopLevel::Reject,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTier {
    High,
    Normal,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchTierAnswer {
    pub choice: WatchTier,
    /// Order: high, normal, low.
    pub probabilities: [f64; 3],
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitoringValueAnswer {
    /// 0.0..=4.0 (may fall between levels).
    pub score: f64,
    /// Levels 0..=4.
    pub probabilities: [f64; 5],
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A validated advisory. Never eligibility, profit or safety evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvisoryAnswer {
    /// The model that actually answered (allowlisted).
    pub model: String,
    pub watch_tier: WatchTierAnswer,
    /// Probability that extra route preparation is useful, 0..=1.
    pub extra_preparation: f64,
    pub monitoring_value: MonitoringValueAnswer,
    pub usage: Usage,
    /// keccak256 of the exact body, domain separated.
    pub response_digest: B256,
}

pub fn response_digest(body: &[u8]) -> B256 {
    let mut b = Vec::with_capacity(body.len() + 32);
    b.extend_from_slice(b"jev-advisor/response/v1\0");
    b.extend_from_slice(body);
    keccak256(&b)
}

// ---- raw, duplicate-rejecting deserialization -------------------------------------------------

/// Ordered map that fails on duplicate keys (serde_json::Value silently keeps the last).
struct UniqueMap<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for UniqueMap<T> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = UniqueMap<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out: Vec<(String, T)> = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    if out.iter().any(|(e, _)| *e == k) {
                        return Err(de::Error::custom(format!(
                            "duplicate key `{}`",
                            sanitize(&k)
                        )));
                    }
                    out.push((k, m.next_value()?));
                }
                Ok(UniqueMap(out))
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawAnswer {
    Choice(RawChoice),
    Noul(RawNoul),
    Score(RawScore),
}

impl RawAnswer {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Choice(_) => "choice",
            Self::Noul(_) => "noul",
            Self::Score(_) => "score",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChoice {
    choice: String,
    probabilities: UniqueMap<f64>,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNoul {
    noul: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScore {
    score: f64,
    #[serde(default, deserialize_with = "some_map")]
    legend: Option<UniqueMap<String>>,
    probabilities: UniqueMap<f64>,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUsage {
    input_tokens: u64,
    output_tokens: u64,
}

struct RawResponse {
    model: String,
    answers: UniqueMap<RawAnswer>,
    usage: RawUsage,
    unknown_keys: Vec<String>,
}

impl<'de> Deserialize<'de> for RawResponse {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = RawResponse;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the TypeSafe response object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let (mut model, mut answers, mut usage) = (None, None, None);
                let mut unknown_keys = Vec::new();
                let mut seen: Vec<String> = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    if seen.contains(&k) {
                        return Err(de::Error::custom(format!(
                            "duplicate key `{}`",
                            sanitize(&k)
                        )));
                    }
                    match k.as_str() {
                        "model" => model = Some(m.next_value::<String>()?),
                        "answers" => answers = Some(m.next_value::<UniqueMap<RawAnswer>>()?),
                        "usage" => usage = Some(m.next_value::<RawUsage>()?),
                        _ => {
                            m.next_value::<de::IgnoredAny>()?;
                            unknown_keys.push(k.clone());
                        }
                    }
                    seen.push(k);
                }
                Ok(RawResponse {
                    model: model.ok_or_else(|| de::Error::missing_field("model"))?,
                    answers: answers.ok_or_else(|| de::Error::missing_field("answers"))?,
                    usage: usage.ok_or_else(|| de::Error::missing_field("usage"))?,
                    unknown_keys,
                })
            }
        }
        d.deserialize_map(V)
    }
}

// ---- validation -------------------------------------------------------------------------------

fn unit(x: f64, what: &'static str) -> Result<f64, ValidationError> {
    if x.is_finite() && (0.0..=1.0).contains(&x) {
        Ok(x)
    } else {
        Err(ValidationError::OutOfRange(what))
    }
}

/// Exactly the expected keys, each a finite probability in [0,1], summing to 1 within `tol`.
fn dist<const N: usize>(
    raw: &UniqueMap<f64>,
    keys: [&str; N],
    what: &'static str,
    tol: f64,
) -> Result<[f64; N], ValidationError> {
    if raw.0.len() != N {
        return Err(ValidationError::BadProbabilityKeys(what));
    }
    let mut out = [0.0; N];
    for (i, key) in keys.iter().enumerate() {
        let (_, v) = raw
            .0
            .iter()
            .find(|(k, _)| k == key)
            .ok_or(ValidationError::BadProbabilityKeys(what))?;
        out[i] = unit(*v, what)?;
    }
    let sum: f64 = out.iter().sum();
    if !(sum - 1.0).abs().le(&tol) {
        return Err(ValidationError::DistributionSum(what));
    }
    Ok(out)
}

fn schema(e: serde_json::Error) -> TransportError {
    use serde_json::error::Category;
    match e.classify() {
        Category::Syntax | Category::Eof => TransportError::MalformedJson,
        _ => {
            let msg = e.to_string();
            match msg.strip_prefix("duplicate key `") {
                Some(rest) => TransportError::SchemaDrift(ValidationError::DuplicateKey(
                    rest.split('`').next().unwrap_or("").to_string(),
                )),
                None => TransportError::SchemaDrift(ValidationError::Schema(sanitize(&msg))),
            }
        }
    }
}

/// Parse and validate a 2xx JSON body. `Ok` only if every documented-contract rule holds.
pub fn parse_response(
    body: &[u8],
    policy: &ResponsePolicy,
) -> Result<AdvisoryAnswer, TransportError> {
    let raw: RawResponse = serde_json::from_slice(body).map_err(schema)?;
    let drift = TransportError::SchemaDrift;

    if policy.unknown_top_level == UnknownTopLevel::Reject
        && let Some(k) = raw.unknown_keys.first()
    {
        return Err(drift(ValidationError::UnknownTopLevelKey(sanitize(k))));
    }
    if !policy.allowed_models.contains(&raw.model) {
        return Err(TransportError::WrongModel {
            model: sanitize(&raw.model),
        });
    }
    for (id, _) in &raw.answers.0 {
        if !ANSWER_IDS.contains(&id.as_str()) {
            return Err(drift(ValidationError::UnexpectedAnswer(sanitize(id))));
        }
    }
    let get = |id: &'static str| {
        raw.answers
            .0
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, a)| a)
            .ok_or(drift_missing(id))
    };
    let tol = policy.prob_sum_tolerance;
    if !(tol.is_finite() && (0.0..0.5).contains(&tol)) {
        return Err(TransportError::Config(
            "prob_sum_tolerance must be in [0, 0.5)".into(),
        ));
    }

    let watch_tier = match get("watch_tier")? {
        RawAnswer::Choice(c) => {
            let probabilities = dist(
                &c.probabilities,
                ["high", "normal", "low"],
                "watch_tier",
                tol,
            )
            .map_err(drift)?;
            let choice = match c.choice.as_str() {
                "high" => WatchTier::High,
                "normal" => WatchTier::Normal,
                "low" => WatchTier::Low,
                other => return Err(drift(ValidationError::UnknownChoice(sanitize(other)))),
            };
            let confidence = unit(c.confidence, "watch_tier.confidence").map_err(drift)?;
            WatchTierAnswer {
                choice,
                probabilities,
                confidence,
            }
        }
        other => {
            return Err(drift(ValidationError::WrongAnswerType {
                id: "watch_tier",
                got: other.type_name(),
            }));
        }
    };

    let extra_preparation = match get("extra_preparation")? {
        RawAnswer::Noul(n) => unit(n.noul, "extra_preparation").map_err(drift)?,
        other => {
            return Err(drift(ValidationError::WrongAnswerType {
                id: "extra_preparation",
                got: other.type_name(),
            }));
        }
    };

    let monitoring_value = match get("monitoring_value")? {
        RawAnswer::Score(s) => {
            let probabilities = dist(
                &s.probabilities,
                ["0", "1", "2", "3", "4"],
                "monitoring_value",
                tol,
            )
            .map_err(drift)?;
            if !(s.score.is_finite() && (0.0..=4.0).contains(&s.score)) {
                return Err(drift(ValidationError::OutOfRange("monitoring_value.score")));
            }
            let expected: f64 = probabilities
                .iter()
                .enumerate()
                .map(|(i, p)| i as f64 * p)
                .sum();
            if (s.score - expected).abs() > policy.score_tolerance {
                return Err(drift(ValidationError::ScoreInconsistent));
            }
            if let Some(legend) = &s.legend {
                let ok = legend.0.len() == 5
                    && (0..5).all(|i| legend.0.iter().any(|(k, _)| *k == i.to_string()));
                if !ok {
                    return Err(drift(ValidationError::BadLegend));
                }
            }
            let confidence = unit(s.confidence, "monitoring_value.confidence").map_err(drift)?;
            MonitoringValueAnswer {
                score: s.score,
                probabilities,
                confidence,
            }
        }
        other => {
            return Err(drift(ValidationError::WrongAnswerType {
                id: "monitoring_value",
                got: other.type_name(),
            }));
        }
    };

    Ok(AdvisoryAnswer {
        model: raw.model,
        watch_tier,
        extra_preparation,
        monitoring_value,
        usage: Usage {
            input_tokens: raw.usage.input_tokens,
            output_tokens: raw.usage.output_tokens,
        },
        response_digest: response_digest(body),
    })
}

/// Present means a real object: JSON `null` is not "absent".
fn some_map<'de, D: de::Deserializer<'de>>(d: D) -> Result<Option<UniqueMap<String>>, D::Error> {
    UniqueMap::deserialize(d).map(Some)
}

fn drift_missing(id: &'static str) -> TransportError {
    TransportError::SchemaDrift(ValidationError::MissingAnswer(id))
}
