//! Live read-only check (ignored by default): for tracked Uniswap V4 pools, the tick-aware model must
//! price swaps that cross ticks like the official V4Quoter, where the one-bucket model cannot.
//! Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL and MEV_IT_V4_REGISTRY (a saved registry).

use std::path::Path;

use alloy::{
    primitives::{
        Address, Bytes, address,
        aliases::{I24, U24},
    },
    sol,
};
use mev_discovery::{load_pools, ticks::load_ticks, v4};
use mev_types::PoolKind;

sol! {
    #[sol(rpc)]
    interface IV4Quoter {
        struct PoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
        struct QuoteExactSingleParams { PoolKey poolKey; bool zeroForOne; uint128 exactAmount; bytes hookData; }
        function quoteExactInputSingle(QuoteExactSingleParams memory params) external returns (uint256 amountOut, uint256 gasEstimate);
    }
}

const QUOTER: Address = address!("3972C00f7ed4885e145823eb7c655375d275A1C5");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

#[tokio::test]
#[ignore = "network"]
async fn v4_tick_model_prices_swaps_across_ticks_like_the_quoter() {
    let http = std::env::var("ARBITRUM_HTTP_URL").expect("ARBITRUM_HTTP_URL");
    let ws = std::env::var("ARBITRUM_WS_URL").expect("ARBITRUM_WS_URL");
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap();
    let v4cfg = cfg.uniswap_v4.clone().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("v4.json");
    std::fs::copy(
        std::env::var("MEV_IT_V4_REGISTRY").expect("MEV_IT_V4_REGISTRY"),
        &registry,
    )
    .unwrap();
    let infos = v4::discover(&cfg, &v4cfg, &registry).await;
    let pools = load_pools(&rpc, infos).await.unwrap();
    let maps = load_ticks(&rpc, &pools, 800).await.unwrap();
    let quoter = IV4Quoter::new(QUOTER, &rpc.http);
    let (mut compared, mut beyond_bucket, mut better) = (0, 0, 0);
    for pool in pools
        .iter()
        .filter(|p| matches!(p.info.kind, PoolKind::UniV4 { .. }))
    {
        let Some(map) = maps.get(&pool.info.address) else {
            continue;
        };
        let with = pool.clone().with_ticks(map.clone());
        let PoolKind::UniV4 {
            fee,
            tick_spacing,
            native,
            flipped,
            ..
        } = pool.info.kind
        else {
            continue;
        };
        let (a, b) = if flipped {
            (pool.info.token1, pool.info.token0)
        } else {
            (pool.info.token0, pool.info.token1)
        };
        let (c0, c1) = if native { (Address::ZERO, b) } else { (a, b) };
        let bucket = pool.max_exact_in(pool.info.token0);
        let window = with.max_exact_in(pool.info.token0);
        if bucket < 1e3 {
            continue;
        }
        for mult in [0.5f64, 3.0, 15.0] {
            let amount = (bucket * mult).floor();
            if amount > window * 0.9 {
                continue; // beyond what the loaded window knows
            }
            let token_in = pool.info.token0;
            let zero_for_one = if native && token_in == WETH {
                true
            } else {
                token_in == c0
            };
            let params = IV4Quoter::QuoteExactSingleParams {
                poolKey: IV4Quoter::PoolKey {
                    currency0: c0,
                    currency1: c1,
                    fee: U24::from(fee),
                    tickSpacing: I24::try_from(tick_spacing as i32).unwrap(),
                    hooks: Address::ZERO,
                },
                zeroForOne: zero_for_one,
                exactAmount: amount as u128,
                hookData: Bytes::new(),
            };
            let Ok(q) = quoter.quoteExactInputSingle(params).call().await else {
                continue;
            };
            let truth = f64::from(q.amountOut);
            if truth <= 0.0 {
                continue;
            }
            let err = |x: f64| (x - truth).abs() / truth * 100.0;
            let (flat, tick) = (
                pool.amount_out(token_in, amount),
                with.amount_out(token_in, amount),
            );
            println!(
                "fee {fee} sp {tick_spacing} native {native} flipped {flipped} x{mult}: quoter {truth:.4e} flat {:.3}% ticks {:.4}% ({} ticks)",
                err(flat),
                err(tick),
                map.ticks.len()
            );
            assert!(err(tick) < 0.5, "tick model off by {:.3} %", err(tick));
            compared += 1;
            if mult > 1.0 {
                beyond_bucket += 1;
                if err(tick) < err(flat) {
                    better += 1;
                }
            }
        }
    }
    println!(
        "compared {compared}, beyond the bucket {beyond_bucket}, tick model closer in {better}"
    );
    assert!(compared >= 6, "compared {compared}");
}
