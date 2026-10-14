//! The redaction boundary: `RedactedFeatures` is the only type later HTTP code may serialize.
//!
//! Two layers. (1) By construction `FeatureV1` holds only closed enums with fixed strings and is
//! not `Serialize`, so it cannot carry a borrower/pool address, amount, URL, key or route.
//! (2) `RedactedFeatures::new` additionally requires `verified_canonical` and scans the emitted
//! JSON for address-/hash-shaped hex runs, URLs and known secret markers, so a future field that
//! leaks fails closed.

use alloy::primitives::{B256, keccak256};
use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::bands::SnapshotQuality;
use crate::features::FeatureV1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RedactionError {
    #[error("only verified_canonical features may leave the process (got {0})")]
    NotVerifiedCanonical(&'static str),
    #[error("forbidden content in feature payload: {0}")]
    Forbidden(&'static str),
    #[error("model id must be 1..=64 chars of [A-Za-z0-9._-]")]
    BadModel,
}

/// First forbidden pattern found in `text`, if any: a run of >= 40 hex digits (addresses, hashes,
/// keys), a `0x` prefix, a URL, or a known secret/credential marker.
pub fn forbidden_pattern(text: &str) -> Option<&'static str> {
    let mut run = 0usize;
    for b in text.bytes() {
        if b.is_ascii_hexdigit() {
            run += 1;
            if run >= 40 {
                return Some("hex run >= 40 (address/hash/key)");
            }
        } else {
            run = 0;
        }
    }
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[(&str, &str)] = &[
        ("0x", "0x prefix"),
        ("://", "url"),
        ("bearer", "bearer token"),
        ("authorization", "authorization header"),
        ("private_key", "private key"),
        ("privatekey", "private key"),
        ("mnemonic", "mnemonic"),
        ("secret", "secret"),
        ("api_key", "api key"),
        ("apikey", "api key"),
        ("sk-", "api key prefix"),
        ("rpc_url", "rpc url"),
        ("-----begin", "pem block"),
    ];
    MARKERS
        .iter()
        .find(|(m, _)| lower.contains(m))
        .map(|(_, why)| *why)
}

/// Verified-canonical, scanned feature set. Immutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedFeatures {
    features: FeatureV1,
    json: String,
}

impl RedactedFeatures {
    pub fn new(features: FeatureV1) -> Result<Self, RedactionError> {
        if features.snapshot_quality != SnapshotQuality::VerifiedCanonical {
            return Err(RedactionError::NotVerifiedCanonical(
                features.snapshot_quality.as_str(),
            ));
        }
        let json = features.canonical_json();
        if let Some(why) = forbidden_pattern(&json) {
            return Err(RedactionError::Forbidden(why));
        }
        Ok(Self { features, json })
    }

    pub fn features(&self) -> &FeatureV1 {
        &self.features
    }

    /// The canonical JSON object (the `state` of a Jev request).
    pub fn json(&self) -> &str {
        &self.json
    }

    pub fn digest(&self) -> B256 {
        self.features.digest()
    }
}

impl Serialize for RedactedFeatures {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let fields = self.features.fields();
        let mut m = ser.serialize_map(Some(fields.len()))?;
        for (k, v) in fields {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// Cache-dedup key for a hosted-advisor request: identical features asked of the same model with
/// the same instructions yield the same key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestKey {
    pub feature_hash: B256,
    pub model: String,
    pub prompt_hash: B256,
}

impl RequestKey {
    pub fn new(
        features: &RedactedFeatures,
        model: &str,
        prompt_hash: B256,
    ) -> Result<Self, RedactionError> {
        let ok = (1..=64).contains(&model.len())
            && model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !ok {
            return Err(RedactionError::BadModel);
        }
        Ok(Self {
            feature_hash: features.digest(),
            model: model.to_string(),
            prompt_hash,
        })
    }

    /// One 32-byte id for the whole key (length-prefixed model, domain-separated).
    pub fn id(&self) -> B256 {
        let mut b = Vec::with_capacity(160);
        b.extend_from_slice(b"mev-liq-features/request-key/v1\0");
        b.extend_from_slice(self.feature_hash.as_slice());
        b.extend_from_slice(&(self.model.len() as u32).to_be_bytes());
        b.extend_from_slice(self.model.as_bytes());
        b.extend_from_slice(self.prompt_hash.as_slice());
        keccak256(&b)
    }
}
