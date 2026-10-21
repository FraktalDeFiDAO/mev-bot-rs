//! Live read-only differential test (ignored): Aerodrome V2 (Solidly-style) volatile pools, loaded the way the
//! bot loads them, must quote exactly what the pool's own `getAmountOut` returns at one pinned block. Needs
//! BASE_HTTP_URL / BASE_WS_URL (public endpoints work, e.g. https://base-rpc.publicnode.com).

use alloy::{
    primitives::{Address, U256, address},
    sol,
};
use mev_discovery::load_pools_at;
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IFactory { function getPool(address a, address b, bool stable) external view returns (address); }
    #[sol(rpc)]
    interface IPoolQ {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    }
}

const FACTORY: Address = address!("420DD381b31aEf6683db6B902084cB0FFECe40Da");
const WETH: Address = address!("4200000000000000000000000000000000000006");
const OTHERS: [Address; 3] = [
    address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
    address!("940181a94A35A4569E4529A3CDfB74e38FD98631"), // AERO
    address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), // cbETH
];

#[tokio::test]
#[ignore = "network"]
async fn aerodrome_integer_math_equals_the_pool() {
    let http = std::env::var("BASE_HTTP_URL").unwrap();
    let ws = std::env::var("BASE_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let factory = IFactory::new(FACTORY, &rpc.http);
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let (mut compared, mut pools_seen) = (0, 0);
    for other in OTHERS {
        let pool_addr = factory.getPool(WETH, other, false).call().await.unwrap();
        if pool_addr == Address::ZERO {
            continue;
        }
        let q = IPoolQ::new(pool_addr, &rpc.http);
        let (t0, t1) = (
            q.token0().call().await.unwrap(),
            q.token1().call().await.unwrap(),
        );
        let info = PoolInfo {
            address: pool_addr,
            dex: "aerodrome".into(),
            kind: PoolKind::SolidlyV2 { factory: FACTORY },
            token0: t0,
            token1: t1,
        };
        let pools = load_pools_at(&rpc, vec![info], block).await.unwrap();
        let pool = pools.first().expect("a volatile pool loads");
        pools_seen += 1;
        for token_in in [t0, t1] {
            for k in 3..=22u32 {
                let amount = U256::from(10u8).pow(U256::from(k)) + U256::from(k * 7919);
                let Some(mine) = pool.exact_amount_out(token_in, amount) else {
                    continue;
                };
                let theirs = q
                    .getAmountOut(amount, token_in)
                    .block(block)
                    .call()
                    .await
                    .unwrap();
                assert_eq!(
                    mine, theirs,
                    "block {number}, pool {pool_addr}, in {token_in}, amount {amount}"
                );
                compared += 1;
            }
        }
    }
    println!("block {number}: {pools_seen} pools, {compared} Aerodrome sizes equal to the unit");
    assert!(
        pools_seen >= 2 && compared >= 40,
        "too few comparisons ({pools_seen} pools, {compared})"
    );
}
