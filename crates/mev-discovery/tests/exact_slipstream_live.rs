//! Live read-only differential test (ignored): Aerodrome Slipstream pools, loaded the way the bot loads them
//! (state, dynamic fee and initialized ticks at one pinned block), must price swaps exactly like Slipstream's
//! own QuoterV2, including swaps that cross initialized ticks. Needs BASE_HTTP_URL / BASE_WS_URL.

use alloy::{
    primitives::{
        Address, U256, address,
        aliases::{I24, U160},
    },
    sol,
};
use mev_discovery::{load_pools_at, ticks::load_ticks_at};
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IFactory { function getPool(address a, address b, int24 tickSpacing) external view returns (address); }
    #[sol(rpc)]
    interface IPoolQ {
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
    #[sol(rpc)]
    interface IQuoterV2 {
        struct Params { address tokenIn; address tokenOut; uint256 amountIn; int24 tickSpacing; uint160 sqrtPriceLimitX96; }
        function quoteExactInputSingle(Params params) external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }
}

const FACTORY: Address = address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A");
const QUOTER: Address = address!("254cF9E1E6e233aa1AC962CB9B05b2cfeAaE15b0");
const WETH: Address = address!("4200000000000000000000000000000000000006");
const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

#[tokio::test]
#[ignore = "network"]
async fn slipstream_integer_math_equals_the_quoter() {
    let http = std::env::var("BASE_HTTP_URL").unwrap();
    let ws = std::env::var("BASE_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let factory = IFactory::new(FACTORY, &rpc.http);
    let quoter = IQuoterV2::new(QUOTER, &rpc.http);
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let (mut pools_seen, mut compared, mut crossed) = (0, 0, 0);
    for tick_spacing in [1u32, 10, 50, 100, 200] {
        let spacing = I24::try_from(tick_spacing).unwrap();
        let addr = factory.getPool(WETH, USDC, spacing).call().await.unwrap();
        if addr == Address::ZERO {
            continue;
        }
        let q = IPoolQ::new(addr, &rpc.http);
        let (t0, t1) = (
            q.token0().call().await.unwrap(),
            q.token1().call().await.unwrap(),
        );
        let info = PoolInfo {
            address: addr,
            dex: "aerodrome_slipstream".into(),
            kind: PoolKind::SlipstreamV3 {
                fee: 0,
                tick_spacing,
            },
            token0: t0,
            token1: t1,
        };
        let mut pools = load_pools_at(&rpc, vec![info], block).await.unwrap();
        let maps = load_ticks_at(&rpc, &pools, 4000 * tick_spacing as i32, block)
            .await
            .unwrap();
        let mut pool = pools.remove(0);
        let PoolKind::SlipstreamV3 { fee, .. } = pool.info.kind else {
            unreachable!()
        };
        assert!(fee > 0, "the load fills in the dynamic fee");
        pool.ticks = Some(std::sync::Arc::new(
            maps.into_values().next().expect("tick map"),
        ));
        pools_seen += 1;
        for token_in in [t0, t1] {
            let token_out = if token_in == t0 { t1 } else { t0 };
            for k in 4..=24u32 {
                let amount = U256::from(10u8).pow(U256::from(k)) + U256::from(k * 7919);
                let Some(mine) = pool.exact_amount_out(token_in, amount) else {
                    continue; // beyond the loaded tick window: the model declines
                };
                let Ok(r) = quoter
                    .quoteExactInputSingle(IQuoterV2::Params {
                        tokenIn: token_in,
                        tokenOut: token_out,
                        amountIn: amount,
                        tickSpacing: spacing,
                        sqrtPriceLimitX96: U160::ZERO,
                    })
                    .block(block)
                    .call()
                    .await
                else {
                    continue; // the quoter reverts on dust / exhausted liquidity
                };
                assert_eq!(
                    mine, r.amountOut,
                    "block {number}, pool {addr} (spacing {tick_spacing}, fee {fee}), in {token_in}, amount {amount}"
                );
                compared += 1;
                if r.initializedTicksCrossed > 1 {
                    crossed += 1;
                }
            }
        }
    }
    println!(
        "block {number}: {pools_seen} pools, {compared} sizes equal to the quoter ({crossed} crossing ticks)"
    );
    assert!(
        pools_seen >= 3 && compared >= 30 && crossed >= 3,
        "too few comparisons"
    );
}
