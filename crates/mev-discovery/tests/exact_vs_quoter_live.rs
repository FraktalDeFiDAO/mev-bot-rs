//! Live read-only differential test (ignored): the integer swap math must equal the Uniswap V3 QuoterV2
//! to the unit, at one pinned block, across sizes that cross ticks. Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL
//! (public endpoints work) and optionally MEV_IT_POOL (a V3 pool address; default WETH/USDC 0.05%).
//!   ARBITRUM_HTTP_URL=https://arb1.arbitrum.io/rpc ARBITRUM_WS_URL=wss://arb1.arbitrum.io/ws \
//!     cargo test -p mev-discovery --test exact_vs_quoter_live -- --ignored --nocapture

use alloy::{
    primitives::{Address, U256, address, aliases::U24},
    sol,
};
use mev_discovery::{load_pools_at, ticks::load_ticks_at};
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IQuoterV2 {
        struct Params { address tokenIn; address tokenOut; uint256 amountIn; uint24 fee; uint160 sqrtPriceLimitX96; }
        function quoteExactInputSingle(Params memory params) external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }
    #[sol(rpc)]
    interface IPool {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
}

const UNI_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");
const UNI_QUOTER: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
const PANCAKE_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const PANCAKE_QUOTER: Address = address!("B048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997");

#[tokio::test]
#[ignore = "network"]
async fn integer_swap_math_equals_the_quoter() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let pool: Address = std::env::var("MEV_IT_POOL")
        .map(|s| s.parse().unwrap())
        .unwrap_or(address!("C6962004f452bE9203591991D15f6b388e09E8D0"));
    let p = IPool::new(pool, &rpc.http);
    let factory = p.factory().call().await.unwrap();
    assert!(
        factory == UNI_FACTORY || factory == PANCAKE_FACTORY,
        "pick a Uniswap or Pancake V3 pool"
    );
    let (t0, t1) = (
        p.token0().call().await.unwrap(),
        p.token1().call().await.unwrap(),
    );
    let fee = u32::try_from(p.fee().call().await.unwrap()).unwrap();
    let (kind, quoter_addr) = if factory == PANCAKE_FACTORY {
        (PoolKind::PancakeV3 { fee }, PANCAKE_QUOTER)
    } else {
        (PoolKind::UniV3 { fee }, UNI_QUOTER)
    };
    let info = PoolInfo {
        address: pool,
        dex: "t".into(),
        kind,
        token0: t0,
        token1: t1,
    };

    // One block for everything: pool state, ticks and every quote.
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let mut pools = load_pools_at(&rpc, vec![info], block).await.unwrap();
    let maps = load_ticks_at(&rpc, &pools, 4000, block).await.unwrap();
    let mut pool_state = pools.remove(0);
    pool_state.ticks = Some(std::sync::Arc::new(
        maps.into_values().next().expect("tick map"),
    ));
    assert!(pool_state.exact.is_some(), "exact state must be loaded");

    let quoter = IQuoterV2::new(quoter_addr, &rpc.http);
    let (mut compared, mut crossed) = (0, 0);
    for token_in in [t0, t1] {
        let token_out = if token_in == t0 { t1 } else { t0 };
        for k in 4..=24u32 {
            let amount = U256::from(10u8).pow(U256::from(k));
            let Some(mine) = pool_state.exact_amount_out(token_in, amount) else {
                continue; // beyond the loaded window: the model declines, which is the point
            };
            let q = quoter
                .quoteExactInputSingle(IQuoterV2::Params {
                    tokenIn: token_in,
                    tokenOut: token_out,
                    amountIn: amount,
                    fee: U24::from(fee),
                    sqrtPriceLimitX96: alloy::primitives::aliases::U160::ZERO,
                })
                .block(block)
                .call()
                .await;
            let Ok(q) = q else { continue }; // quoter reverts on dust/overflow sizes
            assert_eq!(
                mine, q.amountOut,
                "block {number}, token_in {token_in}, amount {amount}"
            );
            compared += 1;
            if q.initializedTicksCrossed > 0 {
                crossed += 1;
            }
        }
    }
    println!(
        "block {number}: {compared} sizes equal to the unit ({crossed} crossed initialized ticks)"
    );
    assert!(compared >= 6, "too few comparable sizes ({compared})");
}
