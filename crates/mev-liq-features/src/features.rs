//! `FeatureV1`, the as-of `FeatureInput` and the pure `build_features`.
//!
//! Everything is derived from data passed in; nothing is read from a clock, disk or network.
//! Each input carries its own event time and anything newer than the `AsOf` point is ignored (or,
//! for the current snapshot itself, rejected), so a replay at time T is byte-identical no matter
//! what was recorded later. Input ordering never matters: series are sorted before use.

use alloy::primitives::{B256, I256, U256, U512, keccak256};
use mev_liq_state::{BorrowerSnapshot, ChainSource, HistoryPoint, SnapshotStatus};

use crate::bands::*;
use crate::registry::ClassRegistry;

/// Schema identifier carried in every feature set and in the digest.
pub const SCHEMA_VERSION: &str = "copper_liq_features_v1";

const NS: u64 = 1_000_000_000;
/// A canonical snapshot older than this (at the as-of time) is labeled `stale`.
pub const MAX_SNAPSHOT_AGE_NS: u64 = 120 * NS;
/// Oracle volatility window ending at the as-of time.
pub const ORACLE_WINDOW_NS: u64 = 3_600 * NS;
/// Newest in-window oracle point must be at most this old, else volatility is `unknown`.
pub const ORACLE_MAX_STALENESS_NS: u64 = 1_800 * NS;
/// Fewer in-window oracle points than this => `unknown`.
pub const ORACLE_MIN_POINTS: usize = 3;
/// Prices above this (raw, any decimals) are treated as out of range.
pub const ORACLE_MAX_PRICE: U256 = U256::from_limbs([0, 0, 1 << 56, 0]); // ~2^184
/// `hf_trend` comparison interval bounds (observation time between the two canonical snapshots).
pub const TREND_MIN_INTERVAL_NS: u64 = 2 * NS;
pub const TREND_MAX_INTERVAL_NS: u64 = 600 * NS;
/// `hf_trend` is `stable` within +-this many bps of the previous HF.
pub const TREND_STABLE_BPS: u32 = 50;
/// Route liquidity older than this is stale => `unknown`.
pub const LIQUIDITY_MAX_AGE_NS: u64 = 60 * NS;
/// Previous simulation outcomes considered (most recent first) and the minimum sample.
pub const SIM_MAX_SAMPLES: usize = 20;
pub const SIM_MIN_SAMPLES: usize = 5;

/// The event-time point features are computed for. `monotonic_ns` is the scheduler's monotonic
/// clock (only used against `FeatureInput::last_check_monotonic_ns`, never mixed with wall time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsOf {
    pub block_number: u64,
    pub event_time_ns: u64,
    pub monotonic_ns: u64,
}

/// One oracle price observation for the reference asset (signed, as Chainlink returns it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricePoint {
    pub block_number: u64,
    pub observed_at_ns: u64,
    pub price: I256,
}

/// Current validated route liquidity expressed in the same base-currency units as the debt.
/// Carries no route structure, pool or token identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteLiquidity {
    pub liquidity_base: U256,
    pub observed_at_ns: u64,
}

/// A prior simulation result for this opportunity class, stamped with its own event time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulationOutcome {
    pub recorded_at_ns: u64,
    pub succeeded: bool,
}

/// Everything needed to compute features as of an event time.
#[derive(Debug, Clone)]
pub struct FeatureInput<'a> {
    pub protocol: Protocol,
    pub snapshot: BorrowerSnapshot,
    /// Previous canonical point (from `SnapshotRepository::previous_canonical`).
    pub previous: Option<HistoryPoint>,
    /// Oracle base-currency unit (USD * 10^8 on Aave V3 Arbitrum). Zero => debt band unknown.
    pub base_currency_unit: U256,
    /// Reference-asset price history; order and future entries are irrelevant.
    pub oracle_history: Vec<PricePoint>,
    pub registry: &'a ClassRegistry,
    pub route_liquidity: Option<RouteLiquidity>,
    pub simulations: Vec<SimulationOutcome>,
    /// Scheduler monotonic time of the account's last successful recheck.
    pub last_check_monotonic_ns: Option<u64>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FeatureError {
    #[error("snapshot block {snapshot_block} is after the as-of block {as_of_block}")]
    SnapshotBlockAfterAsOf {
        snapshot_block: u64,
        as_of_block: u64,
    },
    #[error("snapshot observed at {observed_ns}ns is after the as-of time {as_of_ns}ns")]
    SnapshotTimeAfterAsOf { observed_ns: u64, as_of_ns: u64 },
}

/// The closed, versioned feature set. Deliberately not `Serialize`: the only serializable form is
/// [`crate::RedactedFeatures`]. Its fields are enums with fixed strings, so no address, amount,
/// URL, key or route can be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FeatureV1 {
    pub chain: Chain,
    pub protocol: Protocol,
    pub health_factor_band: HealthFactorBand,
    pub debt_usd_band: DebtUsdBand,
    pub collateral_class: AssetClass,
    pub debt_class: AssetClass,
    pub oracle_volatility_band: OracleVolatilityBand,
    pub hf_trend: HfTrend,
    pub liquidity_to_debt_band: LiquidityToDebtBand,
    pub historical_route_success_band: RouteSuccessBand,
    pub last_account_recheck_age_band: RecheckAgeBand,
    pub snapshot_quality: SnapshotQuality,
}

impl FeatureV1 {
    /// `(wire key, wire value)` in canonical order (the spec section 6 example order).
    pub fn fields(&self) -> [(&'static str, &'static str); 13] {
        [
            ("schema_version", SCHEMA_VERSION),
            ("chain", self.chain.as_str()),
            ("protocol", self.protocol.as_str()),
            ("health_factor_band", self.health_factor_band.as_str()),
            ("debt_usd_band", self.debt_usd_band.as_str()),
            ("collateral_class", self.collateral_class.as_str()),
            ("debt_class", self.debt_class.as_str()),
            (
                "oracle_volatility_band",
                self.oracle_volatility_band.as_str(),
            ),
            ("hf_trend", self.hf_trend.as_str()),
            (
                "liquidity_to_debt_band",
                self.liquidity_to_debt_band.as_str(),
            ),
            (
                "historical_route_success_band",
                self.historical_route_success_band.as_str(),
            ),
            (
                "last_account_recheck_age_band",
                self.last_account_recheck_age_band.as_str(),
            ),
            ("snapshot_quality", self.snapshot_quality.as_str()),
        ]
    }

    /// Canonical JSON: fixed field order, compact, string values only (no floats, no numbers).
    /// Values are fixed ASCII literals without characters needing escapes.
    pub fn canonical_json(&self) -> String {
        let mut out = String::with_capacity(400);
        out.push('{');
        for (i, (k, v)) in self.fields().iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(k);
            out.push_str("\":\"");
            out.push_str(v);
            out.push('"');
        }
        out.push('}');
        out
    }

    /// `keccak256("mev-liq-features/digest/v1\0" || schema_version || \0 || canonical_json)`.
    /// Changes if and only if a digested feature (or the schema version) changes.
    pub fn digest(&self) -> B256 {
        let mut b = Vec::with_capacity(480);
        b.extend_from_slice(b"mev-liq-features/digest/v1\0");
        b.extend_from_slice(SCHEMA_VERSION.as_bytes());
        b.push(0);
        b.extend_from_slice(self.canonical_json().as_bytes());
        keccak256(&b)
    }
}

fn chain_of(chain_id: u64) -> Chain {
    if chain_id == 42_161 {
        Chain::ArbitrumOne
    } else {
        Chain::Unknown
    }
}

fn snapshot_quality(s: &BorrowerSnapshot, as_of: &AsOf) -> SnapshotQuality {
    if s.status != SnapshotStatus::Valid || !s.digest_matches() {
        return SnapshotQuality::Invalid;
    }
    match s.chain.source {
        ChainSource::Provisional => SnapshotQuality::Provisional,
        ChainSource::Canonical => {
            if as_of.event_time_ns - s.chain.observed_at_ns > MAX_SNAPSHOT_AGE_NS {
                SnapshotQuality::Stale
            } else {
                SnapshotQuality::VerifiedCanonical
            }
        }
    }
}

fn class_of<'a>(
    registry: &ClassRegistry,
    chain_id: u64,
    reserves: impl Iterator<Item = &'a mev_liq_state::ReserveBalance>,
    side: impl Fn(&mev_liq_state::ReserveBalance) -> U256,
) -> AssetClass {
    let mut seen: Option<AssetClass> = None;
    for r in reserves.filter(|r| !side(r).is_zero()) {
        let c = registry.classify(chain_id, &r.asset);
        if c == AssetClass::Unknown {
            return AssetClass::Unknown;
        }
        match seen {
            None => seen = Some(c),
            Some(p) if p != c => seen = Some(AssetClass::Mixed),
            Some(_) => {}
        }
    }
    seen.unwrap_or(AssetClass::Unknown)
}

fn volatility(history: &[PricePoint], as_of: &AsOf) -> OracleVolatilityBand {
    let start = as_of.event_time_ns.saturating_sub(ORACLE_WINDOW_NS);
    let mut pts: Vec<PricePoint> = history
        .iter()
        .filter(|p| {
            p.block_number <= as_of.block_number
                && p.observed_at_ns <= as_of.event_time_ns
                && p.observed_at_ns >= start
        })
        .copied()
        .collect();
    pts.sort_by_key(|p| (p.block_number, p.observed_at_ns, p.price));
    if pts.len() < ORACLE_MIN_POINTS {
        return OracleVolatilityBand::Unknown;
    }
    // Two different prices at the same block, or any invalid price, poison the window.
    if pts
        .windows(2)
        .any(|w| w[0].block_number == w[1].block_number && w[0].price != w[1].price)
    {
        return OracleVolatilityBand::Unknown;
    }
    let newest = pts.iter().map(|p| p.observed_at_ns).max().unwrap_or(0);
    if as_of.event_time_ns - newest > ORACLE_MAX_STALENESS_NS {
        return OracleVolatilityBand::Unknown;
    }
    let mut min = U256::MAX;
    let mut max = U256::ZERO;
    for p in &pts {
        if p.price.is_negative() || p.price.is_zero() {
            return OracleVolatilityBand::Unknown;
        }
        let v = p.price.into_raw();
        if v > ORACLE_MAX_PRICE {
            return OracleVolatilityBand::Unknown;
        }
        min = min.min(v);
        max = max.max(v);
    }
    let range_bps = (wide(max - min) * U512::from(10_000u32)) / wide(min);
    match narrow(range_bps) {
        Some(r) => volatility_band_from_range_bps(r),
        None => OracleVolatilityBand::Unknown,
    }
}

fn trend(cur: &BorrowerSnapshot, prev: Option<&HistoryPoint>, as_of: &AsOf) -> HfTrend {
    let Some(p) = prev else {
        return HfTrend::Unknown;
    };
    let c = &cur.chain;
    let valid_interval = c.source == ChainSource::Canonical
        && cur.status == SnapshotStatus::Valid
        && p.block_number < c.block_number
        && p.block_number <= as_of.block_number
        && p.observed_at_ns < c.observed_at_ns
        && p.observed_at_ns <= as_of.event_time_ns
        && {
            let dt = c.observed_at_ns - p.observed_at_ns;
            (TREND_MIN_INTERVAL_NS..=TREND_MAX_INTERVAL_NS).contains(&dt)
        };
    if !valid_interval || p.total_debt_base.is_zero() || cur.total_debt_base.is_zero() {
        return HfTrend::Unknown;
    }
    let cur_hf = wide(cur.health_factor_wad) * U512::from(10_000u32);
    let prev_hf = wide(p.health_factor_wad);
    let down = prev_hf * U512::from(10_000u32 - TREND_STABLE_BPS);
    let up = prev_hf * U512::from(10_000u32 + TREND_STABLE_BPS);
    if cur_hf < down {
        HfTrend::Falling
    } else if cur_hf > up {
        HfTrend::Rising
    } else {
        HfTrend::Stable
    }
}

fn liquidity(input: &FeatureInput<'_>, as_of: &AsOf) -> LiquidityToDebtBand {
    match input.route_liquidity {
        Some(l)
            if l.observed_at_ns <= as_of.event_time_ns
                && as_of.event_time_ns - l.observed_at_ns <= LIQUIDITY_MAX_AGE_NS =>
        {
            liquidity_band(l.liquidity_base, input.snapshot.total_debt_base)
        }
        _ => LiquidityToDebtBand::Unknown,
    }
}

fn route_success(sims: &[SimulationOutcome], as_of: &AsOf) -> RouteSuccessBand {
    let mut v: Vec<SimulationOutcome> = sims
        .iter()
        .filter(|s| s.recorded_at_ns <= as_of.event_time_ns)
        .copied()
        .collect();
    v.sort_by_key(|s| (std::cmp::Reverse(s.recorded_at_ns), s.succeeded));
    v.truncate(SIM_MAX_SAMPLES);
    if v.len() < SIM_MIN_SAMPLES {
        return RouteSuccessBand::Unknown;
    }
    let n = v.len();
    let ok = v.iter().filter(|s| s.succeeded).count();
    if ok * 4 < n {
        RouteSuccessBand::Low
    } else if ok * 4 >= n * 3 {
        RouteSuccessBand::High
    } else {
        RouteSuccessBand::Medium
    }
}

fn recheck_age(last: Option<u64>, as_of: &AsOf) -> RecheckAgeBand {
    match last {
        // A last-check later than "now" means the clocks are not comparable: say so.
        Some(l) if l <= as_of.monotonic_ns => recheck_age_band(as_of.monotonic_ns - l),
        _ => RecheckAgeBand::Unknown,
    }
}

/// Pure, deterministic feature construction as of `now`.
pub fn build_features(input: &FeatureInput<'_>, now: &AsOf) -> Result<FeatureV1, FeatureError> {
    let s = &input.snapshot;
    if s.chain.block_number > now.block_number {
        return Err(FeatureError::SnapshotBlockAfterAsOf {
            snapshot_block: s.chain.block_number,
            as_of_block: now.block_number,
        });
    }
    if s.chain.observed_at_ns > now.event_time_ns {
        return Err(FeatureError::SnapshotTimeAfterAsOf {
            observed_ns: s.chain.observed_at_ns,
            as_of_ns: now.event_time_ns,
        });
    }
    let quality = snapshot_quality(s, now);
    let usable = quality != SnapshotQuality::Invalid;
    let chain_id = s.key.chain_id;
    Ok(FeatureV1 {
        chain: chain_of(chain_id),
        protocol: input.protocol,
        health_factor_band: if usable {
            health_factor_band(s.health_factor_wad, s.total_debt_base)
        } else {
            HealthFactorBand::Unknown
        },
        debt_usd_band: if usable {
            debt_usd_band(s.total_debt_base, input.base_currency_unit)
        } else {
            DebtUsdBand::Unknown
        },
        collateral_class: class_of(input.registry, chain_id, s.reserves.iter(), |r| {
            r.collateral_raw
        }),
        debt_class: class_of(input.registry, chain_id, s.reserves.iter(), |r| r.debt_raw),
        oracle_volatility_band: volatility(&input.oracle_history, now),
        hf_trend: if usable {
            trend(s, input.previous.as_ref(), now)
        } else {
            HfTrend::Unknown
        },
        liquidity_to_debt_band: liquidity(input, now),
        historical_route_success_band: route_success(&input.simulations, now),
        last_account_recheck_age_band: recheck_age(input.last_check_monotonic_ns, now),
        snapshot_quality: quality,
    })
}
