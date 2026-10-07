//! Live read-only check (ignored by default): pricing across ticks with the loaded tick map must
//! match the on-chain QuoterV2 more closely than the single-bucket model does.
//! Needs ARBITRUM_HTTP_URL and ARBITRUM_WS_URL.

use alloy::{
    primitives::{Address, U256, address, aliases::U24},
    sol,
};
use mev_discovery::{load_pools, ticks::load_ticks};
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IQuoterV2 {
        struct Params { address tokenIn; address tokenOut; uint256 amountIn; uint24 fee; uint160 sqrtPriceLimitX96; }
        function quoteExactInputSingle(Params memory params) external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }
}

const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
const CBBTC: Address = address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf");
const UNI_QUOTER: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
const PANCAKE_QUOTER: Address = address!("B048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997");

#[tokio::test]
#[ignore = "network"]
async fn tick_aware_pricing_matches_the_quoter_better_than_the_flat_model() {
    let http = std::env::var("ARBITRUM_HTTP_URL").expect("ARBITRUM_HTTP_URL");
    let ws = std::env::var("ARBITRUM_WS_URL").expect("ARBITRUM_WS_URL");
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let mk = |address, t0, t1, kind| PoolInfo {
        address,
        dex: "t".into(),
        kind,
        token0: t0,
        token1: t1,
    };
    // (pool, quoter, fee, token0, token1)
    let cases = [
        (
            address!("C6962004f452bE9203591991D15f6b388e09E8D0"),
            UNI_QUOTER,
            500u32,
            PoolKind::UniV3 { fee: 500 },
        ),
        (
            address!("a4a9958c41cd390bf6b821f108f04187c9b313e6"),
            PANCAKE_QUOTER,
            100,
            PoolKind::PancakeV3 { fee: 100 },
        ),
    ];
    let infos: Vec<_> = cases
        .iter()
        .map(|(a, _, _, k)| {
            let (t0, t1) = if *a == cases[0].0 {
                (WETH, USDC)
            } else {
                (WETH, CBBTC)
            };
            mk(*a, t0, t1, *k)
        })
        .collect();
    let pools = load_pools(&rpc, infos).await.unwrap();
    assert_eq!(pools.len(), 2, "both pools load");
    let maps = load_ticks(&rpc, &pools, 800).await.unwrap();
    for (pool, (addr, quoter, fee, _)) in pools.iter().zip(&cases) {
        let map = maps.get(addr).expect("tick map loaded");
        println!(
            "pool {addr}: {} initialized ticks in [{}, {}]",
            map.ticks.len(),
            map.lo,
            map.hi
        );
        let with = pool.clone().with_ticks(map.clone());
        let q = IQuoterV2::new(*quoter, &rpc.http);
        // Amounts of token0 (WETH) from small to well past the current bucket.
        for eth in [0.01f64, 0.5, 5.0, 40.0] {
            let amount = eth * 1e18;
            let params = IQuoterV2::Params {
                tokenIn: pool.info.token0,
                tokenOut: pool.info.token1,
                amountIn: U256::from(amount as u128),
                fee: U24::from(*fee),
                sqrtPriceLimitX96: Default::default(),
            };
            let Ok(r) = q.quoteExactInputSingle(params).call().await else {
                println!("  {eth:>6} WETH: quoter reverted (beyond pool depth)");
                continue;
            };
            let truth = f64::from(r.amountOut);
            let (flat, tick) = (
                pool.amount_out(pool.info.token0, amount),
                with.amount_out(pool.info.token0, amount),
            );
            let err = |x: f64| (x - truth).abs() / truth * 100.0;
            println!(
                "  {eth:>6} WETH: quoter {truth:.4e}  flat {flat:.4e} ({:.3}%)  ticks {tick:.4e} ({:.3}%)  crossed {}",
                err(flat),
                err(tick),
                r.initializedTicksCrossed
            );
            if (amount as u128) <= with.max_exact_in(pool.info.token0) as u128 {
                assert!(
                    err(tick) < 0.5,
                    "tick model within 0.5% of the quoter inside the loaded window"
                );
            }
        }
    }
}
