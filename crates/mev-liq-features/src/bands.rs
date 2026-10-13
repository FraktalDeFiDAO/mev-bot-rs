//! Closed feature enums and the exact-integer band functions that derive them.
//!
//! Every enum has an `unknown` variant and a fixed wire string (`as_str`). All thresholds are
//! compared with `U256`/`U512` integer math; there is no floating point anywhere in this crate.
//! Bands are lower-inclusive, upper-exclusive.
//!
//! None of these values is liquidation-eligibility evidence. Eligibility is decided only by the
//! deterministic Aave path (`BorrowerSnapshot::eligible` + REVM); a band such as `below_1.00`
//! is a coarse, rounded hint for ranking discretionary checks.

use alloy::primitives::{U256, U512};
use mev_liq_state::WAD;

macro_rules! wire_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }
        impl $name {
            /// Every variant, for exhaustive tests.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// Fixed wire string.
            pub const fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }
    };
}

wire_enum!(Chain { ArbitrumOne => "arbitrum_one", Unknown => "unknown" });
wire_enum!(Protocol { AaveV3 => "aave_v3", Unknown => "unknown" });
wire_enum!(
    /// Health factor band from the exact WAD value of a same-block `getUserAccountData`.
    HealthFactorBand {
        NoDebt => "no_debt",
        Below1 => "below_1.00",
        B100To102 => "1.00_to_1.02",
        B102To105 => "1.02_to_1.05",
        B105To110 => "1.05_to_1.10",
        B110To125 => "1.10_to_1.25",
        B125To200 => "1.25_to_2.00",
        Above200 => "above_2.00",
        Unknown => "unknown",
    }
);
wire_enum!(
    /// Total debt in USD (Aave base currency), banded.
    DebtUsdBand {
        Zero => "zero",
        Under1k => "under_1k",
        K1To10 => "1k_to_10k",
        K10To100 => "10k_to_100k",
        K100To500 => "100k_to_500k",
        K500ToM1 => "500k_to_1m",
        Above1m => "above_1m",
        Unknown => "unknown",
    }
);
wire_enum!(
    /// Coarse economic class of the dominant collateral or debt side.
    AssetClass {
        Stablecoin => "stablecoin",
        VolatileEthCorrelated => "volatile_eth_correlated",
        VolatileBtcCorrelated => "volatile_btc_correlated",
        OtherVolatile => "other_volatile",
        Mixed => "mixed",
        Unknown => "unknown",
    }
);
wire_enum!(OracleVolatilityBand {
    Low => "low",
    Medium => "medium",
    High => "high",
    Extreme => "extreme",
    Unknown => "unknown",
});
wire_enum!(HfTrend { Falling => "falling", Stable => "stable", Rising => "rising", Unknown => "unknown" });
wire_enum!(LiquidityToDebtBand { Low => "low", Medium => "medium", High => "high", Unknown => "unknown" });
wire_enum!(RouteSuccessBand { Low => "low", Medium => "medium", High => "high", Unknown => "unknown" });
wire_enum!(RecheckAgeBand {
    Under30s => "under_30s",
    S30To2m => "30s_to_2m",
    M2To10 => "2m_to_10m",
    M10To1h => "10m_to_1h",
    Stale => "stale",
    Unknown => "unknown",
});
wire_enum!(
    /// `VerifiedCanonical` is reachable only for a canonical, valid, digest-intact, fresh snapshot.
    SnapshotQuality {
        VerifiedCanonical => "verified_canonical",
        Provisional => "provisional",
        Stale => "stale",
        Invalid => "invalid",
    }
);

pub(crate) fn wide(x: U256) -> U512 {
    let l = x.into_limbs();
    U512::from_limbs([l[0], l[1], l[2], l[3], 0, 0, 0, 0])
}

pub(crate) fn narrow(x: U512) -> Option<U256> {
    let l = x.into_limbs();
    (l[4..].iter().all(|v| *v == 0)).then(|| U256::from_limbs([l[0], l[1], l[2], l[3]]))
}

/// `floor(hf_wad * 10000 / 1e18)` computed in 512-bit arithmetic: exact for every `U256`, never
/// overflows (the result of `U256::MAX` is ~1.16e63). Informational only; bands compare the WAD
/// value against exact thresholds instead, and this is **not** eligibility evidence.
pub fn hf_bps(hf_wad: U256) -> U256 {
    let v = wide(hf_wad) * U512::from(10_000u32) / wide(WAD);
    // v <= U256::MAX * 1e4 / 1e18 < 2^256, so narrowing cannot fail.
    narrow(v).unwrap_or(U256::MAX)
}

fn wad_pct(pct: u32) -> U256 {
    // WAD is divisible by 100, so this is exact.
    WAD / U256::from(100u8) * U256::from(pct)
}

/// Health factor band. `debt == 0` => `NoDebt` regardless of `hf` (Aave reports `U256::MAX`).
pub fn health_factor_band(hf_wad: U256, total_debt_base: U256) -> HealthFactorBand {
    use HealthFactorBand::*;
    if total_debt_base.is_zero() {
        return NoDebt;
    }
    let edges = [
        (wad_pct(100), Below1),
        (wad_pct(102), B100To102),
        (wad_pct(105), B102To105),
        (wad_pct(110), B105To110),
        (wad_pct(125), B110To125),
        (wad_pct(200), B125To200),
    ];
    edges
        .iter()
        .find(|(edge, _)| hf_wad < *edge)
        .map_or(Above200, |(_, band)| *band)
}

/// Debt band. `unit` is the oracle base-currency unit (USD * 10^8 on Aave V3 Arbitrum). A zero
/// unit or an edge overflow yields `Unknown`.
pub fn debt_usd_band(total_debt_base: U256, unit: U256) -> DebtUsdBand {
    use DebtUsdBand::*;
    if unit.is_zero() {
        return Unknown;
    }
    if total_debt_base.is_zero() {
        return Zero;
    }
    let usd = |n: u64| unit.checked_mul(U256::from(n));
    let edges = [
        (1_000u64, Under1k),
        (10_000, K1To10),
        (100_000, K10To100),
        (500_000, K100To500),
        (1_000_000, K500ToM1),
    ];
    for (n, band) in edges {
        match usd(n) {
            Some(edge) if total_debt_base < edge => return band,
            Some(_) => {}
            None => return Unknown,
        }
    }
    Above1m
}

/// Volatility from the max-min range in basis points of the minimum.
pub fn volatility_band_from_range_bps(range_bps: U256) -> OracleVolatilityBand {
    use OracleVolatilityBand::*;
    if range_bps < U256::from(25u8) {
        Low
    } else if range_bps < U256::from(100u8) {
        Medium
    } else if range_bps < U256::from(300u16) {
        High
    } else {
        Extreme
    }
}

/// `liquidity / debt` in bps: <1x low, <5x medium, otherwise high. Zero debt => unknown.
pub fn liquidity_band(liquidity: U256, debt: U256) -> LiquidityToDebtBand {
    if debt.is_zero() {
        return LiquidityToDebtBand::Unknown;
    }
    let ratio_bps = wide(liquidity) * U512::from(10_000u32) / wide(debt);
    if ratio_bps < U512::from(10_000u32) {
        LiquidityToDebtBand::Low
    } else if ratio_bps < U512::from(50_000u32) {
        LiquidityToDebtBand::Medium
    } else {
        LiquidityToDebtBand::High
    }
}

/// Age band in nanoseconds; `>= 1h` is `Stale`.
pub fn recheck_age_band(age_ns: u64) -> RecheckAgeBand {
    const S: u64 = 1_000_000_000;
    match age_ns {
        a if a < 30 * S => RecheckAgeBand::Under30s,
        a if a < 120 * S => RecheckAgeBand::S30To2m,
        a if a < 600 * S => RecheckAgeBand::M2To10,
        a if a < 3_600 * S => RecheckAgeBand::M10To1h,
        _ => RecheckAgeBand::Stale,
    }
}
