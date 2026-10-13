//! Versioned, redacted, as-of feature contract for the Jev liquidation advisor (JEV-LIQ-007).
//!
//! A pure leaf crate: it consumes `mev-liq-state` types plus data handed in by the caller and
//! returns `FeatureV1`. No I/O, no network, no clock. Bands are rounded, advisory hints and are
//! **never** eligibility, profit or safety evidence (spec section 0, invariants 2 and 8).

mod bands;
mod features;
mod redact;
mod registry;

pub use bands::{
    AssetClass, Chain, DebtUsdBand, HealthFactorBand, HfTrend, LiquidityToDebtBand,
    OracleVolatilityBand, Protocol, RecheckAgeBand, RouteSuccessBand, SnapshotQuality,
    debt_usd_band, health_factor_band, hf_bps, liquidity_band, recheck_age_band,
    volatility_band_from_range_bps,
};
pub use features::{
    AsOf, FeatureError, FeatureInput, FeatureV1, LIQUIDITY_MAX_AGE_NS, MAX_SNAPSHOT_AGE_NS,
    ORACLE_MAX_PRICE, ORACLE_MAX_STALENESS_NS, ORACLE_MIN_POINTS, ORACLE_WINDOW_NS, PricePoint,
    RouteLiquidity, SCHEMA_VERSION, SIM_MAX_SAMPLES, SIM_MIN_SAMPLES, SimulationOutcome,
    TREND_MAX_INTERVAL_NS, TREND_MIN_INTERVAL_NS, TREND_STABLE_BPS, build_features,
};
pub use redact::{RedactedFeatures, RedactionError, RequestKey, forbidden_pattern};
pub use registry::{ClassRegistry, RegistryError};
