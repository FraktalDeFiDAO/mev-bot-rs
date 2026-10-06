//! Live read-only check against Arbitrum (ignored by default): Uniswap V4 pools found by our
//! discovery and read from the PoolManager's storage must price swaps like the official V4Quoter.
//! Needs ARBITRUM_HTTP_URL and ARBITRUM_WS_URL (a keyed endpoint).

use std::path::Path;

use alloy::{
    primitives::{Address, Bytes, U256, address, aliases::U24},
    providers::Provider,
    sol,
};
use mev_discovery::{load_pools, v4};
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
async fn our_v4_model_prices_swaps_like_the_official_quoter() {
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
    // Optionally start from a saved registry (MEV_IT_V4_REGISTRY) so only recent blocks are scanned.
    let registry = match std::env::var("MEV_IT_V4_REGISTRY") {
        Ok(seed) => {
            let copy = dir.path().join("v4.json");
            std::fs::copy(seed, &copy).unwrap();
            copy
        }
        Err(_) => dir.path().join("v4.json"),
    };
    let infos = v4::discover(&cfg, &v4cfg, &registry).await;
    assert!(
        infos.len() >= 20,
        "tracks the active hookless pools: {}",
        infos.len()
    );
    let pools = load_pools(&rpc, infos).await.unwrap();
    assert!(
        pools.len() >= 15,
        "most tracked pools load: {}",
        pools.len()
    );

    let quoter = IV4Quoter::new(QUOTER, &rpc.http);
    let mut checked = 0;
    // The busiest shapes: a native ETH pool, an ERC20 pool and a stable pool (whatever is tracked).
    for pool in pools
        .iter()
        .filter(|p| matches!(p.info.kind, PoolKind::UniV4 { .. }))
    {
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
        // Pool currencies in the pool's own order (native side = address 0).
        let (a, b) = if flipped {
            (pool.info.token1, pool.info.token0)
        } else {
            (pool.info.token0, pool.info.token1)
        };
        let (c0, c1) = if native { (Address::ZERO, b) } else { (a, b) };
        // Half of what the model prices exactly (inside the current tick bucket); beyond it a swap
        // may cross ticks, which the model deliberately doesn't claim to know.
        let token_in = pool.info.token0;
        let (r_in, _) = pool.reserves(token_in);
        let amount = (pool.max_exact_in(token_in) * 0.5).floor();
        if amount < 1e3 {
            continue;
        }
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
                tickSpacing: alloy::primitives::aliases::I24::try_from(tick_spacing as i32)
                    .unwrap(),
                hooks: Address::ZERO,
            },
            zeroForOne: zero_for_one,
            exactAmount: amount as u128,
            hookData: Bytes::new(),
        };
        let Ok(q) = quoter.quoteExactInputSingle(params).call().await else {
            continue;
        };
        let model = pool.amount_out(token_in, amount);
        let chain = f64::from(q.amountOut);
        if chain <= 0.0 || r_in < 1e3 {
            continue;
        }
        let err = ((model - chain) / chain).abs();
        println!(
            "pool fee {fee} spacing {tick_spacing} native {native} flipped {flipped}: model {model:.0} quoter {chain:.0} err {:.4} %",
            err * 100.0
        );
        assert!(
            err < 0.01,
            "model off by {:.3} % for fee {fee} spacing {tick_spacing}",
            err * 100.0
        );
        checked += 1;
    }
    assert!(checked >= 8, "compared {checked} pools");
    let _ = (U256::ZERO, rpc.http.get_block_number().await);
}
