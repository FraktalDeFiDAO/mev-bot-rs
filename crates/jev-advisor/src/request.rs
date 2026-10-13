//! Request body construction (spec section 6). The body is assembled from three parts only: the
//! validated model id, the `RedactedFeatures` canonical JSON (the only feature payload type) and
//! the constant questions block. Nothing else can reach the wire.

use alloy::primitives::{B256, keccak256};
use mev_liq_features::{RedactedFeatures, RedactionError, RequestKey, forbidden_pattern};

/// The three questions for one borrower, compact JSON, fixed key order. Hashed by
/// [`prompt_digest`]; changing any byte changes the digest and therefore every `RequestKey`.
pub const QUESTIONS_JSON: &str = concat!(
    r#"{"watch_tier":{"type":"choice","instructions":"For optional monitoring beyond mandatory "#,
    r#"deterministic checks, which relative watch tier deserves additional scheduling resources? "#,
    r#"Do not infer liquidation eligibility or transaction profitability.","criteria":{"#,
    r#""high":"Strong reason to give this account extra discretionary monitoring","#,
    r#""normal":"No strong reason to adjust baseline optional-monitoring frequency","#,
    r#""low":"Low priority for extra checks only; never waive mandatory checks"}},"#,
    r#""extra_preparation":{"type":"noul","instructions":"Is extra optional route preparation "#,
    r#"likely to be useful relative to similar non-urgent tracked accounts?"},"#,
    r#""monitoring_value":{"type":"score","instructions":"How valuable is extra monitoring, "#,
    r#"based only on the supplied evidence?","criteria":["No evidence of added value","#,
    r#""Weak evidence","Moderate evidence","Strong evidence","Very strong evidence"]}}"#,
);

/// Answer ids, in the order of [`QUESTIONS_JSON`].
pub const ANSWER_IDS: [&str; 3] = ["watch_tier", "extra_preparation", "monitoring_value"];

/// Instructions hash exposed for `RequestKey::prompt_hash`.
pub fn prompt_digest() -> B256 {
    let mut b = Vec::with_capacity(QUESTIONS_JSON.len() + 32);
    b.extend_from_slice(b"jev-advisor/prompt/v1\0");
    b.extend_from_slice(QUESTIONS_JSON.as_bytes());
    keccak256(&b)
}

/// Cache/dedup key for this exact prompt, features and model.
pub fn request_key(features: &RedactedFeatures, model: &str) -> Result<RequestKey, RedactionError> {
    RequestKey::new(features, model, prompt_digest())
}

/// The serialized request. Constructed only by [`build_request`].
#[derive(Clone, PartialEq, Eq)]
pub struct AdvisoryRequest {
    body: Vec<u8>,
    key: RequestKey,
}

impl AdvisoryRequest {
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn key(&self) -> &RequestKey {
        &self.key
    }
}

impl std::fmt::Debug for AdvisoryRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdvisoryRequest")
            .field("model", &self.key.model)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Build `{"model":..,"state":..,"questions":..}` (compact, this key order).
pub fn build_request(
    features: &RedactedFeatures,
    model: &str,
) -> Result<AdvisoryRequest, RedactionError> {
    let key = request_key(features, model)?; // validates the model id charset/length
    let model_json = serde_json::to_string(model).map_err(|_| RedactionError::BadModel)?;
    let body = format!(
        r#"{{"model":{model_json},"state":{},"questions":{QUESTIONS_JSON}}}"#,
        features.json()
    );
    // Defense in depth over the whole body, not only the state.
    if let Some(why) = forbidden_pattern(&body) {
        return Err(RedactionError::Forbidden(why));
    }
    Ok(AdvisoryRequest {
        body: body.into_bytes(),
        key,
    })
}
