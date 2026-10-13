use alloy::primitives::{Address, B256, I256, U256};
use mev_liq_features::*;
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainPoint, ChainRef, ChainSource, HistoryPoint, ReserveBalance,
    SnapshotRepository, SnapshotStatus, WAD,
};
use proptest::prelude::*;

const NS: u64 = 1_000_000_000;
const WETH: &str = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1";
const USDC: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";

fn usd8(n: u64) -> U256 {
    U256::from(n) * U256::from(100_000_000u64)
}

fn wad_x100(n: u64) -> U256 {
    WAD / U256::from(100u8) * U256::from(n)
}

fn as_of() -> AsOf {
    AsOf {
        block_number: 1_000,
        event_time_ns: 2_000 * NS,
        monotonic_ns: 5_000 * NS,
    }
}

fn hash(block: u64) -> B256 {
    B256::left_padding_from(&block.to_be_bytes())
}

fn snapshot(block: u64, observed_s: u64, source: ChainSource, hf: U256) -> BorrowerSnapshot {
    BorrowerSnapshot {
        key: AccountKey {
            chain_id: 42_161,
            pool: Address::repeat_byte(0xaa),
            borrower: Address::repeat_byte(0x11),
        },
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: hash(block),
            source,
            observed_at_ns: observed_s * NS,
        },
        health_factor_wad: hf,
        total_debt_base: usd8(150_000),
        total_collateral_base: usd8(300_000),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::from(8_000u16),
        ltv: U256::from(7_500u16),
        reserves: vec![
            ReserveBalance {
                asset: WETH.parse().unwrap(),
                collateral_raw: U256::from(10u64).pow(U256::from(20u8)),
                debt_raw: U256::ZERO,
            },
            ReserveBalance {
                asset: USDC.parse().unwrap(),
                collateral_raw: U256::ZERO,
                debt_raw: U256::from(150_000_000_000u64),
            },
        ],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(observed_s * NS),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

fn price(block: u64, t_s: u64, p: i64) -> PricePoint {
    PricePoint {
        block_number: block,
        observed_at_ns: t_s * NS,
        price: I256::try_from(p).unwrap(),
    }
}

/// An input that reproduces the spec section 6 example exactly.
fn spec_input(reg: &ClassRegistry) -> FeatureInput<'_> {
    let s = snapshot(1_000, 1_990, ChainSource::Canonical, wad_x100(101));
    FeatureInput {
        protocol: Protocol::AaveV3,
        snapshot: s,
        previous: Some(HistoryPoint {
            block_number: 900,
            block_hash: hash(900),
            observed_at_ns: 1_960 * NS,
            health_factor_wad: wad_x100(105),
            total_debt_base: usd8(150_000),
        }),
        base_currency_unit: usd8(1),
        // 200 bps range => "high".
        oracle_history: vec![
            price(940, 1_900, 100_000_000),
            price(960, 1_920, 102_000_000),
            price(980, 1_940, 101_000_000),
            price(990, 1_950, 101_500_000),
        ],
        registry: reg,
        route_liquidity: Some(RouteLiquidity {
            liquidity_base: usd8(300_000),
            observed_at_ns: 1_980 * NS,
        }),
        simulations: (0..8)
            .map(|i| SimulationOutcome {
                recorded_at_ns: (1_000 + i) * NS,
                succeeded: i == 3,
            })
            .collect(),
        last_check_monotonic_ns: Some(4_990 * NS),
    }
}

fn reg() -> ClassRegistry {
    ClassRegistry::pinned().unwrap()
}

#[test]
fn spec_example_is_reproduced_byte_for_byte() {
    let r = reg();
    let f = build_features(&spec_input(&r), &as_of()).unwrap();
    assert_eq!(
        f.canonical_json(),
        concat!(
            r#"{"schema_version":"copper_liq_features_v1","chain":"arbitrum_one","protocol":"aave_v3","#,
            r#""health_factor_band":"1.00_to_1.02","debt_usd_band":"100k_to_500k","#,
            r#""collateral_class":"volatile_eth_correlated","debt_class":"stablecoin","#,
            r#""oracle_volatility_band":"high","hf_trend":"falling","liquidity_to_debt_band":"medium","#,
            r#""historical_route_success_band":"low","last_account_recheck_age_band":"under_30s","#,
            r#""snapshot_quality":"verified_canonical"}"#
        )
    );
}

// ---------- fixtures: band edges ----------

#[derive(serde::Deserialize)]
struct Fixture {
    hf: Vec<HfRow>,
    debt: Vec<DebtRow>,
}
#[derive(serde::Deserialize)]
struct HfRow {
    name: String,
    hf_wad: String,
    band: String,
}
#[derive(serde::Deserialize)]
struct DebtRow {
    name: String,
    debt_base: String,
    band: String,
}

fn fixture() -> Fixture {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/edge_cases.toml"
    );
    toml::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn fixture_hf_and_debt_bands_match_exactly() {
    let f = fixture();
    for r in &f.hf {
        let hf = U256::from_str_radix(&r.hf_wad, 10).unwrap();
        let b = health_factor_band(hf, U256::from(1u8));
        assert_eq!(b.as_str(), r.band, "{}", r.name);
    }
    for r in &f.debt {
        let d = U256::from_str_radix(&r.debt_base, 10).unwrap();
        assert_eq!(debt_usd_band(d, usd8(1)).as_str(), r.band, "{}", r.name);
    }
}

#[test]
fn hf_band_edges_in_wad_one_wei_either_side() {
    let one = U256::from(1u8);
    let d = one;
    for (pct, below, at) in [
        (100, HealthFactorBand::Below1, HealthFactorBand::B100To102),
        (
            102,
            HealthFactorBand::B100To102,
            HealthFactorBand::B102To105,
        ),
        (
            105,
            HealthFactorBand::B102To105,
            HealthFactorBand::B105To110,
        ),
        (
            110,
            HealthFactorBand::B105To110,
            HealthFactorBand::B110To125,
        ),
        (
            125,
            HealthFactorBand::B110To125,
            HealthFactorBand::B125To200,
        ),
        (200, HealthFactorBand::B125To200, HealthFactorBand::Above200),
    ] {
        let edge = wad_x100(pct);
        assert_eq!(health_factor_band(edge - one, d), below, "{pct} - 1 wei");
        assert_eq!(health_factor_band(edge, d), at, "{pct}");
        assert_eq!(health_factor_band(edge + one, d), at, "{pct} + 1 wei");
    }
    // Zero debt: Aave reports U256::MAX; never a numeric band. Zero HF with debt is below 1.
    assert_eq!(
        health_factor_band(U256::MAX, U256::ZERO),
        HealthFactorBand::NoDebt
    );
    assert_eq!(
        health_factor_band(U256::ZERO, U256::ZERO),
        HealthFactorBand::NoDebt
    );
    assert_eq!(health_factor_band(U256::ZERO, d), HealthFactorBand::Below1);
    assert_eq!(health_factor_band(U256::MAX, d), HealthFactorBand::Above200);
}

#[test]
fn hf_bps_floors_exactly_and_never_overflows() {
    let e14 = U256::from(10u64).pow(U256::from(14u8));
    assert_eq!(hf_bps(U256::ZERO), U256::ZERO);
    assert_eq!(hf_bps(e14 - U256::from(1u8)), U256::ZERO);
    assert_eq!(hf_bps(e14), U256::from(1u8));
    assert_eq!(hf_bps(WAD - U256::from(1u8)), U256::from(9_999u32));
    assert_eq!(hf_bps(WAD), U256::from(10_000u32));
    assert_eq!(hf_bps(WAD + U256::from(1u8)), U256::from(10_000u32));
    // Independent oracle: U256::MAX / 1e14 (floor(x*1e4/1e18) == floor(x/1e14)).
    assert_eq!(hf_bps(U256::MAX), U256::MAX / e14);
}

proptest! {
    #[test]
    fn hf_bps_matches_independent_division(limbs in any::<[u64; 4]>()) {
        let x = U256::from_limbs(limbs);
        let e14 = U256::from(10u64).pow(U256::from(14u8));
        prop_assert_eq!(hf_bps(x), x / e14);
    }
}

#[test]
fn debt_band_edges_for_6_8_and_18_decimal_base_units() {
    for dec in [6u32, 8, 18] {
        let unit = U256::from(10u8).pow(U256::from(dec));
        let at = |usd: u64| unit * U256::from(usd);
        let one = U256::from(1u8);
        assert_eq!(debt_usd_band(U256::ZERO, unit), DebtUsdBand::Zero);
        assert_eq!(debt_usd_band(one, unit), DebtUsdBand::Under1k);
        for (usd, below, above) in [
            (1_000, DebtUsdBand::Under1k, DebtUsdBand::K1To10),
            (10_000, DebtUsdBand::K1To10, DebtUsdBand::K10To100),
            (100_000, DebtUsdBand::K10To100, DebtUsdBand::K100To500),
            (500_000, DebtUsdBand::K100To500, DebtUsdBand::K500ToM1),
            (1_000_000, DebtUsdBand::K500ToM1, DebtUsdBand::Above1m),
        ] {
            assert_eq!(
                debt_usd_band(at(usd) - one, unit),
                below,
                "dec {dec} usd {usd} - 1"
            );
            assert_eq!(debt_usd_band(at(usd), unit), above, "dec {dec} usd {usd}");
        }
    }
    // Zero unit and an overflowing unit are unknown, never a guess.
    assert_eq!(
        debt_usd_band(U256::from(5u8), U256::ZERO),
        DebtUsdBand::Unknown
    );
    assert_eq!(
        debt_usd_band(U256::from(5u8), U256::MAX),
        DebtUsdBand::Unknown
    );
    assert_eq!(debt_usd_band(U256::ZERO, U256::MAX), DebtUsdBand::Zero);
    assert_eq!(debt_usd_band(U256::MAX, usd8(1)), DebtUsdBand::Above1m);
}

#[test]
fn liquidity_and_recheck_age_edges() {
    let d = U256::from(1_000u32);
    assert_eq!(
        liquidity_band(U256::from(999u32), d),
        LiquidityToDebtBand::Low
    );
    assert_eq!(
        liquidity_band(U256::from(1_000u32), d),
        LiquidityToDebtBand::Medium
    );
    assert_eq!(
        liquidity_band(U256::from(4_999u32), d),
        LiquidityToDebtBand::Medium
    );
    assert_eq!(
        liquidity_band(U256::from(5_000u32), d),
        LiquidityToDebtBand::High
    );
    assert_eq!(liquidity_band(U256::MAX, d), LiquidityToDebtBand::High);
    assert_eq!(liquidity_band(d, U256::ZERO), LiquidityToDebtBand::Unknown);
    for (age_s, band) in [
        (0u64, RecheckAgeBand::Under30s),
        (29, RecheckAgeBand::Under30s),
        (30, RecheckAgeBand::S30To2m),
        (119, RecheckAgeBand::S30To2m),
        (120, RecheckAgeBand::M2To10),
        (599, RecheckAgeBand::M2To10),
        (600, RecheckAgeBand::M10To1h),
        (3_599, RecheckAgeBand::M10To1h),
        (3_600, RecheckAgeBand::Stale),
    ] {
        assert_eq!(recheck_age_band(age_s * NS), band, "{age_s}s");
    }
    assert_eq!(recheck_age_band(u64::MAX), RecheckAgeBand::Stale);
    assert_eq!(recheck_age_band(30 * NS - 1), RecheckAgeBand::Under30s);
}

#[test]
fn volatility_band_edges() {
    let b = |n: u32| volatility_band_from_range_bps(U256::from(n));
    assert_eq!(b(0), OracleVolatilityBand::Low);
    assert_eq!(b(24), OracleVolatilityBand::Low);
    assert_eq!(b(25), OracleVolatilityBand::Medium);
    assert_eq!(b(99), OracleVolatilityBand::Medium);
    assert_eq!(b(100), OracleVolatilityBand::High);
    assert_eq!(b(299), OracleVolatilityBand::High);
    assert_eq!(b(300), OracleVolatilityBand::Extreme);
}

// ---------- oracle history through build_features ----------

fn vol_with(history: Vec<PricePoint>) -> OracleVolatilityBand {
    let r = reg();
    let mut i = spec_input(&r);
    i.oracle_history = history;
    build_features(&i, &as_of()).unwrap().oracle_volatility_band
}

#[test]
fn oracle_range_edges_use_exact_integer_bps() {
    // min 1_000_000; max 1_002_499 => range 24.99 bps => floor 24 => low; 1_002_500 => 25 => medium.
    let pts = |max: i64| {
        vec![
            price(940, 1_900, 1_000_000),
            price(960, 1_920, max),
            price(980, 1_940, 1_000_000),
        ]
    };
    assert_eq!(vol_with(pts(1_002_499)), OracleVolatilityBand::Low);
    assert_eq!(vol_with(pts(1_002_500)), OracleVolatilityBand::Medium);
    assert_eq!(vol_with(pts(1_030_000)), OracleVolatilityBand::Extreme);
}

#[test]
fn bad_stale_or_sparse_oracle_data_is_unknown() {
    let good = |p| {
        vec![
            price(940, 1_900, 100),
            price(960, 1_920, p),
            price(980, 1_940, 100),
        ]
    };
    assert_eq!(
        vol_with(good(101)),
        OracleVolatilityBand::High,
        "1% = 100 bps"
    );
    // Negative, zero, out-of-range price anywhere in the window poisons it.
    assert_eq!(vol_with(good(-5)), OracleVolatilityBand::Unknown);
    assert_eq!(vol_with(good(0)), OracleVolatilityBand::Unknown);
    let mut huge = good(100);
    huge[1].price = I256::from_raw(ORACLE_MAX_PRICE + U256::from(1u8));
    assert_eq!(vol_with(huge), OracleVolatilityBand::Unknown);
    let mut max = good(100);
    max[1].price = I256::MAX;
    assert_eq!(vol_with(max), OracleVolatilityBand::Unknown);
    // Too few points.
    assert_eq!(
        vol_with(good(101)[..2].to_vec()),
        OracleVolatilityBand::Unknown
    );
    assert_eq!(vol_with(vec![]), OracleVolatilityBand::Unknown);
    // Stale: newest point older than the staleness bound (as-of 2000s; bound 1800s).
    let stale = vec![
        price(100, 150, 100),
        price(110, 160, 101),
        price(120, 170, 100),
    ];
    assert_eq!(vol_with(stale), OracleVolatilityBand::Unknown);
    // Same block, conflicting prices.
    let conflict = vec![
        price(940, 1_900, 100),
        price(940, 1_901, 120),
        price(980, 1_940, 100),
    ];
    assert_eq!(vol_with(conflict), OracleVolatilityBand::Unknown);
    // Outside the 1h window are ignored (-> too few points).
    let old = vec![price(1, 10, 100), price(2, 20, 500), price(3, 30, 100)];
    assert_eq!(vol_with(old), OracleVolatilityBand::Unknown);
}

// ---------- trend ----------

fn trend_with(prev: Option<HistoryPoint>, cur_hf: U256, source: ChainSource) -> HfTrend {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot = snapshot(1_000, 1_990, source, cur_hf);
    i.previous = prev;
    build_features(&i, &as_of()).unwrap().hf_trend
}

fn prev_pt(hf: U256, block: u64, t_s: u64) -> HistoryPoint {
    HistoryPoint {
        block_number: block,
        block_hash: hash(block),
        observed_at_ns: t_s * NS,
        health_factor_wad: hf,
        total_debt_base: usd8(1),
    }
}

#[test]
fn trend_edges_and_interval_validation() {
    let p = wad_x100(110);
    let down = p * U256::from(9_950u32) / U256::from(10_000u32); // exactly -50 bps
    let up = p * U256::from(10_050u32) / U256::from(10_000u32);
    let one = U256::from(1u8);
    let canon = ChainSource::Canonical;
    let prev = prev_pt(p, 900, 1_960);
    assert_eq!(trend_with(Some(prev), down, canon), HfTrend::Stable);
    assert_eq!(trend_with(Some(prev), down - one, canon), HfTrend::Falling);
    assert_eq!(trend_with(Some(prev), up, canon), HfTrend::Stable);
    assert_eq!(trend_with(Some(prev), up + one, canon), HfTrend::Rising);
    // No previous, provisional current, same/later block, bad interval => unknown.
    assert_eq!(trend_with(None, p, canon), HfTrend::Unknown);
    assert_eq!(
        trend_with(Some(prev), p, ChainSource::Provisional),
        HfTrend::Unknown
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 1_000, 1_960)), p, canon),
        HfTrend::Unknown
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 1_001, 1_960)), p, canon),
        HfTrend::Unknown
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 900, 1_989)), p, canon),
        HfTrend::Unknown,
        "10s ok? 1s too short"
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 900, 1_988)), p, canon),
        HfTrend::Stable,
        "2s is the minimum"
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 900, 1_390)), p, canon),
        HfTrend::Stable,
        "600s is the maximum"
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 900, 1_389)), p, canon),
        HfTrend::Unknown
    );
    assert_eq!(
        trend_with(Some(prev_pt(p, 900, 1_991)), p, canon),
        HfTrend::Unknown,
        "previous after current"
    );
    // Zero-debt previous point has no meaningful HF.
    let mut nodebt = prev;
    nodebt.total_debt_base = U256::ZERO;
    assert_eq!(trend_with(Some(nodebt), p, canon), HfTrend::Unknown);
}

// ---------- classes, liquidity, simulations, recheck age ----------

#[test]
fn unknown_and_mixed_class_fallbacks() {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot.reserves.push(ReserveBalance {
        asset: Address::repeat_byte(0x77), // not in the registry
        collateral_raw: U256::from(1u8),
        debt_raw: U256::ZERO,
    });
    i.snapshot = i.snapshot.seal();
    let f = build_features(&i, &as_of()).unwrap();
    assert_eq!(
        f.collateral_class,
        AssetClass::Unknown,
        "any unlisted asset => unknown"
    );
    assert_eq!(f.debt_class, AssetClass::Stablecoin);

    let mut m = spec_input(&r);
    m.snapshot.reserves.push(ReserveBalance {
        asset: "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"
            .parse()
            .unwrap(), // WBTC
        collateral_raw: U256::from(1u8),
        debt_raw: U256::ZERO,
    });
    m.snapshot = m.snapshot.seal();
    assert_eq!(
        build_features(&m, &as_of()).unwrap().collateral_class,
        AssetClass::Mixed
    );

    // Reserve list not populated yet (rechecker leaves it empty) => unknown, not a guess.
    let mut e = spec_input(&r);
    e.snapshot.reserves.clear();
    e.snapshot = e.snapshot.seal();
    let f = build_features(&e, &as_of()).unwrap();
    assert_eq!(
        (f.collateral_class, f.debt_class),
        (AssetClass::Unknown, AssetClass::Unknown)
    );

    // Zero balances do not count toward a class.
    let mut z = spec_input(&r);
    z.snapshot.reserves[1].debt_raw = U256::ZERO;
    z.snapshot = z.snapshot.seal();
    assert_eq!(
        build_features(&z, &as_of()).unwrap().debt_class,
        AssetClass::Unknown
    );

    // Another chain: the registry is chain-scoped.
    let mut c = spec_input(&r);
    c.snapshot.key.chain_id = 1;
    c.snapshot = c.snapshot.seal();
    let f = build_features(&c, &as_of()).unwrap();
    assert_eq!(
        (f.chain, f.collateral_class),
        (Chain::Unknown, AssetClass::Unknown)
    );
}

#[test]
fn stale_or_missing_route_liquidity_is_unknown() {
    let r = reg();
    let mut i = spec_input(&r);
    let liq = |obs_s: u64| {
        Some(RouteLiquidity {
            liquidity_base: usd8(300_000),
            observed_at_ns: obs_s * NS,
        })
    };
    i.route_liquidity = liq(1_940); // exactly 60s old: still fresh
    assert_eq!(
        build_features(&i, &as_of()).unwrap().liquidity_to_debt_band,
        LiquidityToDebtBand::Medium
    );
    i.route_liquidity = liq(1_939); // 61s
    assert_eq!(
        build_features(&i, &as_of()).unwrap().liquidity_to_debt_band,
        LiquidityToDebtBand::Unknown
    );
    i.route_liquidity = liq(2_001); // observed after the as-of point
    assert_eq!(
        build_features(&i, &as_of()).unwrap().liquidity_to_debt_band,
        LiquidityToDebtBand::Unknown
    );
    i.route_liquidity = None;
    assert_eq!(
        build_features(&i, &as_of()).unwrap().liquidity_to_debt_band,
        LiquidityToDebtBand::Unknown
    );
}

fn sims(n: usize, ok: usize, at_s: u64) -> Vec<SimulationOutcome> {
    (0..n)
        .map(|i| SimulationOutcome {
            recorded_at_ns: (at_s + i as u64) * NS,
            succeeded: i < ok,
        })
        .collect()
}

#[test]
fn simulation_history_bands_use_event_time_only() {
    let r = reg();
    let run = |s: Vec<SimulationOutcome>| {
        let mut i = spec_input(&r);
        i.simulations = s;
        build_features(&i, &as_of())
            .unwrap()
            .historical_route_success_band
    };
    assert_eq!(
        run(sims(4, 4, 100)),
        RouteSuccessBand::Unknown,
        "below the minimum sample"
    );
    assert_eq!(run(sims(8, 1, 100)), RouteSuccessBand::Low);
    assert_eq!(
        run(sims(8, 2, 100)),
        RouteSuccessBand::Medium,
        "25% is medium"
    );
    assert_eq!(run(sims(8, 6, 100)), RouteSuccessBand::High, "75% is high");
    assert_eq!(run(sims(8, 5, 100)), RouteSuccessBand::Medium);
    // Outcomes after the as-of time (2000s) do not exist yet.
    assert_eq!(run(sims(8, 8, 3_000)), RouteSuccessBand::Unknown);
    let mut mixed = sims(5, 0, 100);
    mixed.extend(sims(20, 20, 3_000)); // future successes must not turn the band High
    assert_eq!(run(mixed), RouteSuccessBand::Low);
}

#[test]
fn recheck_age_uses_monotonic_clock_and_labels_anomalies() {
    let r = reg();
    let run = |last: Option<u64>| {
        let mut i = spec_input(&r);
        i.last_check_monotonic_ns = last;
        build_features(&i, &as_of())
            .unwrap()
            .last_account_recheck_age_band
    };
    assert_eq!(run(Some(4_990 * NS)), RecheckAgeBand::Under30s);
    assert_eq!(run(Some(5_000 * NS)), RecheckAgeBand::Under30s, "age zero");
    assert_eq!(run(Some(4_500 * NS)), RecheckAgeBand::M2To10);
    assert_eq!(run(Some(1_000 * NS)), RecheckAgeBand::Stale);
    assert_eq!(run(Some(0)), RecheckAgeBand::Stale);
    assert_eq!(
        run(Some(5_001 * NS)),
        RecheckAgeBand::Unknown,
        "last check in the future"
    );
    assert_eq!(run(None), RecheckAgeBand::Unknown);
}

// ---------- snapshot quality / provenance ----------

fn quality_of(s: BorrowerSnapshot) -> SnapshotQuality {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot = s;
    build_features(&i, &as_of()).unwrap().snapshot_quality
}

#[test]
fn only_valid_fresh_digest_intact_canonical_is_verified() {
    let hf = wad_x100(101);
    assert_eq!(
        quality_of(snapshot(1_000, 1_990, ChainSource::Canonical, hf)),
        SnapshotQuality::VerifiedCanonical
    );
    assert_eq!(
        quality_of(snapshot(1_000, 1_990, ChainSource::Provisional, hf)),
        SnapshotQuality::Provisional
    );
    // 120s old is fresh; 121s is stale.
    assert_eq!(
        quality_of(snapshot(1_000, 1_880, ChainSource::Canonical, hf)),
        SnapshotQuality::VerifiedCanonical
    );
    assert_eq!(
        quality_of(snapshot(1_000, 1_879, ChainSource::Canonical, hf)),
        SnapshotQuality::Stale
    );
    let mut inv = snapshot(1_000, 1_990, ChainSource::Canonical, hf);
    inv.status = SnapshotStatus::Invalidated;
    assert_eq!(quality_of(inv), SnapshotQuality::Invalid);
    let mut tampered = snapshot(1_000, 1_990, ChainSource::Canonical, hf);
    tampered.health_factor_wad = wad_x100(150); // digest no longer matches
    assert_eq!(quality_of(tampered), SnapshotQuality::Invalid);
}

#[test]
fn invalid_snapshot_hides_account_derived_bands() {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot.status = SnapshotStatus::Invalidated;
    let f = build_features(&i, &as_of()).unwrap();
    assert_eq!(f.health_factor_band, HealthFactorBand::Unknown);
    assert_eq!(f.debt_usd_band, DebtUsdBand::Unknown);
    assert_eq!(f.hf_trend, HfTrend::Unknown);
    assert!(RedactedFeatures::new(f).is_err());
}

#[test]
fn a_snapshot_from_after_the_as_of_point_is_an_error_not_a_feature() {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot = snapshot(1_001, 1_990, ChainSource::Canonical, wad_x100(101));
    assert_eq!(
        build_features(&i, &as_of()),
        Err(FeatureError::SnapshotBlockAfterAsOf {
            snapshot_block: 1_001,
            as_of_block: 1_000
        })
    );
    i.snapshot = snapshot(1_000, 2_001, ChainSource::Canonical, wad_x100(101));
    assert!(matches!(
        build_features(&i, &as_of()),
        Err(FeatureError::SnapshotTimeAfterAsOf { .. })
    ));
}

proptest! {
    /// Provisional can never be represented as canonical, whatever the other fields are.
    #[test]
    fn provisional_is_never_verified_canonical(
        hf in any::<[u64; 4]>(), age_s in 0u64..400, invalid in any::<bool>(),
    ) {
        let r = reg();
        let mut i = spec_input(&r);
        let mut s = snapshot(1_000, 2_000 - age_s, ChainSource::Provisional, U256::from_limbs(hf));
        if invalid { s.status = SnapshotStatus::Invalidated; }
        i.snapshot = s;
        let f = build_features(&i, &as_of()).unwrap();
        prop_assert_ne!(f.snapshot_quality, SnapshotQuality::VerifiedCanonical);
        prop_assert!(RedactedFeatures::new(f).is_err());
    }
}

// ---------- determinism, digest ----------

proptest! {
    #[test]
    fn input_order_does_not_change_json_or_digest(
        oracle in Just(spec_input(&reg()).oracle_history).prop_shuffle(),
        sim in Just(sims(10, 4, 500)).prop_shuffle(),
    ) {
        let r = reg();
        let base = {
            let mut i = spec_input(&r);
            i.simulations = sims(10, 4, 500);
            build_features(&i, &as_of()).unwrap()
        };
        let mut i = spec_input(&r);
        i.oracle_history = oracle;
        i.simulations = sim;
        let f = build_features(&i, &as_of()).unwrap();
        prop_assert_eq!(f.canonical_json(), base.canonical_json());
        prop_assert_eq!(f.digest(), base.digest());
    }
}

#[test]
fn same_input_gives_identical_bytes_and_digest() {
    let r = reg();
    let a = build_features(&spec_input(&r), &as_of()).unwrap();
    let b = build_features(&spec_input(&r), &as_of()).unwrap();
    assert_eq!(a.canonical_json().as_bytes(), b.canonical_json().as_bytes());
    assert_eq!(a.digest(), b.digest());
}

/// Every field of the digest: changing exactly that field to any other value changes the digest;
/// restoring it restores the digest.
#[test]
fn digest_changes_iff_a_feature_changes() {
    let r = reg();
    let base = build_features(&spec_input(&r), &as_of()).unwrap();
    let d0 = base.digest();
    let mut seen = std::collections::HashSet::new();
    seen.insert(d0);
    macro_rules! vary {
        ($field:ident, $ty:ty) => {
            for v in <$ty>::ALL {
                let mut f = base;
                f.$field = *v;
                if *v == base.$field {
                    assert_eq!(f.digest(), d0, stringify!($field));
                } else {
                    assert_ne!(f.digest(), d0, "{} = {}", stringify!($field), v.as_str());
                    assert!(
                        seen.insert(f.digest()),
                        "digest collision on {}",
                        stringify!($field)
                    );
                }
            }
        };
    }
    vary!(chain, Chain);
    vary!(protocol, Protocol);
    vary!(health_factor_band, HealthFactorBand);
    vary!(debt_usd_band, DebtUsdBand);
    vary!(collateral_class, AssetClass);
    vary!(debt_class, AssetClass);
    vary!(oracle_volatility_band, OracleVolatilityBand);
    vary!(hf_trend, HfTrend);
    vary!(liquidity_to_debt_band, LiquidityToDebtBand);
    vary!(historical_route_success_band, RouteSuccessBand);
    vary!(last_account_recheck_age_band, RecheckAgeBand);
    vary!(snapshot_quality, SnapshotQuality);
}

#[test]
fn request_key_dedups_and_separates() {
    let r = reg();
    let f = RedactedFeatures::new(build_features(&spec_input(&r), &as_of()).unwrap()).unwrap();
    let p1 = alloy::primitives::keccak256(b"prompt one");
    let p2 = alloy::primitives::keccak256(b"prompt two");
    let k = RequestKey::new(&f, "jev-1.13.0", p1).unwrap();
    assert_eq!(k, RequestKey::new(&f, "jev-1.13.0", p1).unwrap());
    assert_eq!(k.id(), RequestKey::new(&f, "jev-1.13.0", p1).unwrap().id());
    assert_ne!(k.id(), RequestKey::new(&f, "jev-1.13.1", p1).unwrap().id());
    assert_ne!(k.id(), RequestKey::new(&f, "jev-1.13.0", p2).unwrap().id());
    let mut other = *f.features();
    other.hf_trend = HfTrend::Rising;
    let o = RedactedFeatures::new(other).unwrap();
    assert_ne!(k.id(), RequestKey::new(&o, "jev-1.13.0", p1).unwrap().id());
    for bad in ["", "has space", "a/b", "model\n", &"x".repeat(65), "jév"] {
        assert_eq!(
            RequestKey::new(&f, bad, p1),
            Err(RedactionError::BadModel),
            "{bad:?}"
        );
    }
}

// ---------- as-of / leakage with the real repository ----------

fn promote(repo: &SnapshotRepository, s: BorrowerSnapshot) {
    repo.checkpoint_canonical(ChainPoint {
        block_number: s.chain.block_number,
        block_hash: s.chain.block_hash,
    })
    .unwrap();
    let rev = repo.fetch_by_key(&s.key).unwrap().map_or(0, |c| c.revision);
    repo.upsert_cas(&s, rev).unwrap();
}

#[test]
fn data_recorded_after_the_as_of_point_cannot_change_features() {
    let d = tempfile::tempdir().unwrap();
    let repo = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    let r = reg();
    promote(
        &repo,
        snapshot(900, 1_960, ChainSource::Canonical, wad_x100(105)),
    );
    promote(
        &repo,
        snapshot(1_000, 1_990, ChainSource::Canonical, wad_x100(101)),
    );

    let build = |repo: &SnapshotRepository, oracle: &[PricePoint], sim: &[SimulationOutcome]| {
        let cur = snapshot(1_000, 1_990, ChainSource::Canonical, wad_x100(101));
        let mut i = spec_input(&r);
        i.previous = repo
            .previous_canonical(&cur.key, cur.chain.block_number)
            .unwrap();
        i.snapshot = cur;
        i.oracle_history = oracle.to_vec();
        i.simulations = sim.to_vec();
        build_features(&i, &as_of()).unwrap()
    };
    let oracle = spec_input(&r).oracle_history;
    let sim = sims(8, 1, 100);
    let before = build(&repo, &oracle, &sim);
    assert_eq!(before.hf_trend, HfTrend::Falling);

    // Append the future: a later canonical snapshot that crashed HF, more oracle ticks (violent
    // moves) and a string of successful liquidations recorded after the as-of time.
    promote(
        &repo,
        snapshot(1_100, 2_100, ChainSource::Canonical, wad_x100(90)),
    );
    promote(
        &repo,
        snapshot(1_200, 2_200, ChainSource::Canonical, wad_x100(80)),
    );
    let mut sim2 = sim.clone();
    sim2.extend(sims(30, 30, 5_000));
    let after_future_only = build(&repo, &oracle, &sim2);
    assert_eq!(before, after_future_only);
    assert_eq!(before.canonical_json(), after_future_only.canonical_json());
    assert_eq!(before.digest(), after_future_only.digest());

    // Future oracle ticks (block and time after as-of) are ignored too.
    let mut oracle3 = oracle.clone();
    oracle3.push(price(1_050, 2_050, 50_000_000));
    oracle3.push(price(1_001, 2_001, 500_000_000));
    assert_eq!(build(&repo, &oracle3, &sim2), before);
}

#[test]
fn previous_canonical_cutoff_feeds_only_strictly_earlier_history() {
    let d = tempfile::tempdir().unwrap();
    let repo = SnapshotRepository::open(&d.path().join("s.db")).unwrap();
    for (b, t, hf) in [
        (900u64, 1_960u64, 105u64),
        (1_000, 1_990, 101),
        (1_100, 2_100, 90),
    ] {
        promote(&repo, snapshot(b, t, ChainSource::Canonical, wad_x100(hf)));
    }
    let key = snapshot(1_000, 1, ChainSource::Canonical, WAD).key;
    for cutoff in [901u64, 1_000, 1_001, 1_100, 1_101] {
        let p = repo.previous_canonical(&key, cutoff).unwrap().unwrap();
        assert!(p.block_number < cutoff);
    }
    assert!(repo.previous_canonical(&key, 900).unwrap().is_none());
}

// ---------- registry ----------

#[test]
fn pinned_registry_parses_and_classifies() {
    let r = reg();
    assert_eq!(r.version(), 1);
    assert!(r.len() >= 10);
    assert_eq!(
        r.classify(42_161, &WETH.parse().unwrap()),
        AssetClass::VolatileEthCorrelated
    );
    assert_eq!(
        r.classify(42_161, &USDC.parse().unwrap()),
        AssetClass::Stablecoin
    );
    assert_eq!(
        r.classify(42_161, &Address::repeat_byte(9)),
        AssetClass::Unknown
    );
    assert_eq!(r.classify(1, &WETH.parse().unwrap()), AssetClass::Unknown);
}

#[test]
fn registry_rejects_bad_input() {
    let head = "registry_version = 1\nchain_id = 42161\n";
    let one = |a: &str, c: &str| format!("{head}[[asset]]\naddress = \"{a}\"\nclass = \"{c}\"\n");
    let a = WETH;
    assert!(ClassRegistry::from_toml_str(&one(a, "stablecoin")).is_ok());
    assert!(matches!(
        ClassRegistry::from_toml_str(&one(a, "mixed")),
        Err(RegistryError::BadClass(_))
    ));
    assert!(matches!(
        ClassRegistry::from_toml_str(&one(a, "unknown")),
        Err(RegistryError::BadClass(_))
    ));
    assert!(matches!(
        ClassRegistry::from_toml_str(&one("0x12", "stablecoin")),
        Err(RegistryError::BadAddress(_))
    ));
    let dup = format!(
        "{}[[asset]]\naddress = \"{a}\"\nclass = \"stablecoin\"\n",
        one(a, "stablecoin")
    );
    assert!(matches!(
        ClassRegistry::from_toml_str(&dup),
        Err(RegistryError::Duplicate(_))
    ));
    assert!(matches!(
        ClassRegistry::from_toml_str("nope"),
        Err(RegistryError::Parse(_))
    ));
}

// ---------- redaction ----------

#[test]
fn scanner_detects_leaks_and_passes_every_wire_string() {
    let hex40 = "ab".repeat(20);
    let hex64 = "Cd".repeat(32);
    for bad in [
        hex40.clone(),
        hex64.clone(),
        format!("0x{hex40}"),
        "0xdead".to_string(),
        "https://arb-mainnet.example/v2/KEY".to_string(),
        "wss://host".to_string(),
        "Authorization: Bearer abc".to_string(),
        "PRIVATE_KEY=1".to_string(),
        "my secret".to_string(),
        "sk-live-1234".to_string(),
        "{\"rpc_url\":1}".to_string(),
    ] {
        assert!(forbidden_pattern(&bad).is_some(), "{bad}");
    }
    assert!(
        forbidden_pattern(&"ab".repeat(19)).is_none(),
        "39 hex digits is not an address"
    );
    macro_rules! all_ok {
        ($($ty:ty),+) => { $( for v in <$ty>::ALL { assert!(forbidden_pattern(v.as_str()).is_none(), "{}", v.as_str()); } )+ };
    }
    all_ok!(
        Chain,
        Protocol,
        HealthFactorBand,
        DebtUsdBand,
        AssetClass,
        OracleVolatilityBand,
        HfTrend,
        LiquidityToDebtBand,
        RouteSuccessBand,
        RecheckAgeBand,
        SnapshotQuality
    );
    assert!(forbidden_pattern(SCHEMA_VERSION).is_none());
}

fn arb_u256() -> impl Strategy<Value = U256> {
    any::<[u64; 4]>().prop_map(U256::from_limbs)
}

fn arb_addr() -> impl Strategy<Value = Address> {
    any::<[u8; 20]>().prop_map(Address::from)
}

proptest! {
    /// Arbitrary inputs (random borrower, pool, assets, hashes, amounts): whatever features come
    /// out, the serialized form never contains an address/hash-shaped string or a secret marker,
    /// and the serde output equals the canonical JSON.
    #[test]
    fn serialized_features_never_contain_identifiers(
        borrower in arb_addr(), pool in arb_addr(), asset in arb_addr(),
        hf in arb_u256(), debt in arb_u256(), raw in arb_u256(),
        block_hash in any::<[u8; 32]>(), provenance in ".{0,40}",
        prices in prop::collection::vec((0u64..2_000, 0u64..2_100, any::<i64>()), 0..8),
    ) {
        let r = reg();
        let mut i = spec_input(&r);
        let mut s = snapshot(1_000, 1_990, ChainSource::Canonical, hf);
        s.key = AccountKey { chain_id: 42_161, pool, borrower };
        s.total_debt_base = debt;
        s.chain.block_hash = B256::from(block_hash);
        s.provenance = format!("http://rpc.example/key {provenance} {borrower} {pool}");
        s.reserves = vec![ReserveBalance { asset, collateral_raw: raw, debt_raw: raw }];
        i.snapshot = s.seal();
        i.oracle_history = prices.into_iter().map(|(b, t, p)| price(b, t, p)).collect();
        let f = build_features(&i, &as_of()).unwrap();
        let json = f.canonical_json();
        prop_assert!(forbidden_pattern(&json).is_none(), "{json}");
        prop_assert!(!json.contains(&borrower.to_string().to_lowercase()));
        if let Ok(red) = RedactedFeatures::new(f) {
            let via_serde = serde_json::to_string(&red).unwrap();
            prop_assert_eq!(&via_serde, red.json());
            prop_assert!(forbidden_pattern(&via_serde).is_none());
            let v: serde_json::Value = serde_json::from_str(&via_serde).unwrap();
            let obj = v.as_object().unwrap();
            prop_assert_eq!(obj.len(), 13);
            prop_assert!(obj.values().all(|x| x.is_string()), "no numbers or floats");
        }
    }
}

#[test]
fn redacted_features_refuse_non_verified_quality() {
    let r = reg();
    let mut i = spec_input(&r);
    i.snapshot = snapshot(1_000, 1_990, ChainSource::Provisional, wad_x100(101));
    let f = build_features(&i, &as_of()).unwrap();
    assert_eq!(
        RedactedFeatures::new(f),
        Err(RedactionError::NotVerifiedCanonical("provisional"))
    );
    let ok = build_features(&spec_input(&r), &as_of()).unwrap();
    let red = RedactedFeatures::new(ok).unwrap();
    assert_eq!(red.json(), ok.canonical_json());
    assert_eq!(red.digest(), ok.digest());
}
