//! Re-validation of exported feature JSON. The export log is written by Process A, but Process B
//! never trusts it: before anything goes on the wire the JSON must parse back into a
//! [`RedactedFeatures`] whose canonical rendering is byte-identical to the stored text and whose
//! digest equals the digest recorded with the row.

use alloy::primitives::B256;
use mev_liq_features::{
    AssetClass, Chain, DebtUsdBand, FeatureV1, HealthFactorBand, HfTrend, LiquidityToDebtBand,
    OracleVolatilityBand, Protocol, RecheckAgeBand, RedactedFeatures, RouteSuccessBand,
    SCHEMA_VERSION, SnapshotQuality,
};
use mev_liq_state::MAX_FEATURE_JSON_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeatureReject {
    #[error("feature json larger than {MAX_FEATURE_JSON_BYTES} bytes")]
    TooLarge,
    #[error("feature json is not an object of string values")]
    NotFlatStringObject,
    #[error("missing or unknown value for `{0}`")]
    BadField(&'static str),
    #[error("feature schema version is not `{SCHEMA_VERSION}`")]
    SchemaVersion,
    #[error("redaction check failed: {0}")]
    Redaction(String),
    #[error("feature json is not in canonical form")]
    NotCanonical,
    #[error("feature digest does not match the exported digest")]
    DigestMismatch,
}

fn pick<T: Copy>(all: &[T], as_str: impl Fn(T) -> &'static str, v: &str) -> Option<T> {
    all.iter().copied().find(|t| as_str(*t) == v)
}

/// Parse and re-validate `json`; `expected_digest` is the digest stored with the queue row.
pub fn parse_redacted(
    json: &str,
    expected_digest: B256,
) -> Result<RedactedFeatures, FeatureReject> {
    if json.len() > MAX_FEATURE_JSON_BYTES {
        return Err(FeatureReject::TooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| FeatureReject::NotFlatStringObject)?;
    let obj = value
        .as_object()
        .ok_or(FeatureReject::NotFlatStringObject)?;
    if obj.values().any(|v| !v.is_string()) {
        return Err(FeatureReject::NotFlatStringObject);
    }
    let get = |k: &'static str| -> Result<&str, FeatureReject> {
        obj.get(k)
            .and_then(|v| v.as_str())
            .ok_or(FeatureReject::BadField(k))
    };
    if get("schema_version")? != SCHEMA_VERSION {
        return Err(FeatureReject::SchemaVersion);
    }
    macro_rules! field {
        ($key:literal, $ty:ty) => {
            pick(<$ty>::ALL, <$ty>::as_str, get($key)?).ok_or(FeatureReject::BadField($key))?
        };
    }
    let features = FeatureV1 {
        chain: field!("chain", Chain),
        protocol: field!("protocol", Protocol),
        health_factor_band: field!("health_factor_band", HealthFactorBand),
        debt_usd_band: field!("debt_usd_band", DebtUsdBand),
        collateral_class: field!("collateral_class", AssetClass),
        debt_class: field!("debt_class", AssetClass),
        oracle_volatility_band: field!("oracle_volatility_band", OracleVolatilityBand),
        hf_trend: field!("hf_trend", HfTrend),
        liquidity_to_debt_band: field!("liquidity_to_debt_band", LiquidityToDebtBand),
        historical_route_success_band: field!("historical_route_success_band", RouteSuccessBand),
        last_account_recheck_age_band: field!("last_account_recheck_age_band", RecheckAgeBand),
        snapshot_quality: field!("snapshot_quality", SnapshotQuality),
    };
    let redacted =
        RedactedFeatures::new(features).map_err(|e| FeatureReject::Redaction(e.to_string()))?;
    // Re-rendering must reproduce the stored text exactly: this rejects extra keys, duplicate keys,
    // reordering and whitespace, so what is sent is what Process A canonicalised.
    if redacted.json() != json {
        return Err(FeatureReject::NotCanonical);
    }
    if redacted.digest() != expected_digest {
        return Err(FeatureReject::DigestMismatch);
    }
    Ok(redacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RedactedFeatures {
        RedactedFeatures::new(FeatureV1 {
            chain: Chain::ArbitrumOne,
            protocol: Protocol::AaveV3,
            health_factor_band: HealthFactorBand::B100To102,
            debt_usd_band: DebtUsdBand::K100To500,
            collateral_class: AssetClass::VolatileEthCorrelated,
            debt_class: AssetClass::Stablecoin,
            oracle_volatility_band: OracleVolatilityBand::High,
            hf_trend: HfTrend::Falling,
            liquidity_to_debt_band: LiquidityToDebtBand::Medium,
            historical_route_success_band: RouteSuccessBand::Low,
            last_account_recheck_age_band: RecheckAgeBand::Under30s,
            snapshot_quality: SnapshotQuality::VerifiedCanonical,
        })
        .unwrap()
    }

    #[test]
    fn round_trips_canonical_json() {
        let f = sample();
        let back = parse_redacted(f.json(), f.digest()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn rejects_every_kind_of_tampering() {
        let f = sample();
        let j = f.json();
        let d = f.digest();
        assert_eq!(
            parse_redacted(&j.replace("aave_v3", "compound"), d),
            Err(FeatureReject::BadField("protocol"))
        );
        assert_eq!(
            parse_redacted(&j.replace("copper_liq_features_v1", "v0"), d),
            Err(FeatureReject::SchemaVersion)
        );
        assert_eq!(
            parse_redacted(&j.replace("verified_canonical", "provisional"), d).unwrap_err(),
            FeatureReject::Redaction(
                "only verified_canonical features may leave the process (got provisional)".into()
            )
        );
        // Extra key, reordering and whitespace are not canonical.
        let extra = j.replacen('{', "{\"extra\":\"x\",", 1);
        assert_eq!(parse_redacted(&extra, d), Err(FeatureReject::NotCanonical));
        let spaced = j.replacen(',', ", ", 1);
        assert_eq!(parse_redacted(&spaced, d), Err(FeatureReject::NotCanonical));
        let dup = j.replacen('{', "{\"chain\":\"unknown\",", 1);
        assert!(parse_redacted(&dup, d).is_err());
        assert_eq!(
            parse_redacted(j, B256::repeat_byte(1)),
            Err(FeatureReject::DigestMismatch)
        );
        assert_eq!(
            parse_redacted("[1]", d),
            Err(FeatureReject::NotFlatStringObject)
        );
        assert_eq!(
            parse_redacted("{\"a\":1}", d),
            Err(FeatureReject::NotFlatStringObject)
        );
        assert_eq!(
            parse_redacted(&" ".repeat(MAX_FEATURE_JSON_BYTES + 1), d),
            Err(FeatureReject::TooLarge)
        );
    }
}
