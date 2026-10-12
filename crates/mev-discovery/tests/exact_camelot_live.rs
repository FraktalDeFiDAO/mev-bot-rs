//! Live read-only differential test (ignored): Camelot V2 integer output must equal the pair's own
//! `getAmountOut` at one pinned block. Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL (public endpoints work).

use alloy::{
    primitives::{Address, U256, address},
    sol,
};
use mev_discovery::load_pools_at;
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface ICamelotFactory { function getPair(address a, address b) external view returns (address); }
    #[sol(rpc)]
    interface ICamelotPairQ {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    }
}

const FACTORY: Address = address!("6EcCab422D763aC031210895C81787E87B43A652");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const OTHERS: [Address; 2] = [
    address!("af88d065e77c8cC2239327C5EDb3A432268e5831"), // USDC
    address!("912CE59144191C1204E64559FE8253a0e49E6548"), // ARB
];

#[tokio::test]
#[ignore = "network"]
async fn camelot_integer_math_equals_the_pair() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let factory = ICamelotFactory::new(FACTORY, &rpc.http);
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let mut compared = 0;
    for other in OTHERS {
        let pair = factory.getPair(WETH, other).call().await.unwrap();
        if pair == Address::ZERO {
            continue;
        }
        let q = ICamelotPairQ::new(pair, &rpc.http);
        let (t0, t1) = (
            q.token0().call().await.unwrap(),
            q.token1().call().await.unwrap(),
        );
        let info = PoolInfo {
            address: pair,
            dex: "camelot".into(),
            kind: PoolKind::CamelotV2,
            token0: t0,
            token1: t1,
        };
        let pools = load_pools_at(&rpc, vec![info], block).await.unwrap();
        let Some(pool) = pools.first() else { continue }; // stable pairs are skipped by the loader
        for token_in in [t0, t1] {
            for k in 3..=22u32 {
                let amount = U256::from(10u8).pow(U256::from(k));
                let Some(mine) = pool.exact_amount_out(token_in, amount) else {
                    continue;
                };
                let Ok(theirs) = q.getAmountOut(amount, token_in).block(block).call().await else {
                    continue;
                };
                assert_eq!(
                    mine, theirs,
                    "block {number}, pair {pair}, in {token_in}, amount {amount}"
                );
                compared += 1;
            }
        }
    }
    println!("block {number}: {compared} Camelot sizes equal to the unit");
    assert!(compared >= 10, "too few comparable sizes ({compared})");
}
