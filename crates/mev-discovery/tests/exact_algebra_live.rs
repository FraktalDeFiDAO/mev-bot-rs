//! Live read-only differential test (ignored) for Camelot V3 (Algebra): the integer walk at the pool's maximum
//! possible fee must never exceed the real Quoter output (it is a lower bound), and at the fee the pool actually
//! charges (`globalState`) it should equal the Quoter whenever the Quoter's own fee matches.
//!   ARBITRUM_HTTP_URL=... ARBITRUM_WS_URL=... cargo test -p mev-discovery --test exact_algebra_live -- --ignored --nocapture

use alloy::{
    primitives::{Address, U256, address},
    sol,
};
use mev_dex::ExactState;
use mev_discovery::{load_pools_at, ticks::load_ticks_at};
use mev_types::{PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IAlgFactory { function poolByPair(address a, address b) external view returns (address); }
    #[sol(rpc)]
    interface IAlgPool {
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
    #[sol(rpc)]
    interface IAlgQuoter {
        function quoteExactInputSingle(address tokenIn, address tokenOut, uint256 amountIn, uint160 limitSqrtPrice) external returns (uint256 amountOut, uint16 fee);
    }
}

const FACTORY: Address = address!("1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B");
const QUOTER: Address = address!("0Fc73040b26E9bC8514fA028D998E73A254Fa76E");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const OTHERS: [Address; 3] = [
    address!("af88d065e77c8cC2239327C5EDb3A432268e5831"), // USDC
    address!("912CE59144191C1204E64559FE8253a0e49E6548"), // ARB
    address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"), // WBTC
];

#[tokio::test]
#[ignore = "network"]
async fn algebra_lower_bound_never_exceeds_the_quoter() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let factory = IAlgFactory::new(FACTORY, &rpc.http);
    assert!(
        !rpc.http.get_code_at(QUOTER).await.unwrap().is_empty(),
        "quoter address has no code"
    );
    use alloy::providers::Provider;
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let quoter = IAlgQuoter::new(QUOTER, &rpc.http);
    let (mut compared, mut equal_at_current_fee, mut tight) = (0, 0, 0);
    for other in OTHERS {
        let addr = factory.poolByPair(WETH, other).call().await.unwrap();
        if addr == Address::ZERO {
            continue;
        }
        let p = IAlgPool::new(addr, &rpc.http);
        let (t0, t1) = (
            p.token0().call().await.unwrap(),
            p.token1().call().await.unwrap(),
        );
        let info = PoolInfo {
            address: addr,
            dex: "camelot_v3".into(),
            kind: PoolKind::AlgebraV3,
            token0: t0,
            token1: t1,
        };
        let mut pools = load_pools_at(&rpc, vec![info], block).await.unwrap();
        let Some(mut pool) = pools.pop() else {
            continue;
        };
        let Some(ExactState::Algebra {
            max_fee_zero_for_one,
            max_fee_one_for_zero,
            ..
        }) = pool.exact.clone()
        else {
            panic!("no Algebra exact state for {addr}");
        };
        let maps = load_ticks_at(&rpc, std::slice::from_ref(&pool), 6000, block)
            .await
            .unwrap();
        pool.ticks = Some(std::sync::Arc::new(
            maps.into_values().next().expect("tick map"),
        ));
        println!(
            "pool {addr} max fees {max_fee_zero_for_one}/{max_fee_one_for_zero}, actual {}/{}",
            pool.fee0, pool.fee1
        );
        for token_in in [t0, t1] {
            let token_out = if token_in == t0 { t1 } else { t0 };
            for k in 3..=22u32 {
                let amount = U256::from(10u8).pow(U256::from(k));
                let Some((bound, is_exact)) = pool.exact_amount_out_bound(token_in, amount) else {
                    continue;
                };
                assert!(!is_exact, "Algebra is only ever a bound");
                let Ok(q) = quoter
                    .quoteExactInputSingle(
                        token_in,
                        token_out,
                        amount,
                        alloy::primitives::aliases::U160::ZERO,
                    )
                    .block(block)
                    .call()
                    .await
                else {
                    continue;
                };
                assert!(
                    bound <= q.amountOut,
                    "bound {bound} exceeds quoter {} (pool {addr}, amount {amount})",
                    q.amountOut
                );
                compared += 1;
                if (q.amountOut - bound) * U256::from(1000u16) < q.amountOut {
                    tight += 1; // within 0.1 %
                }
                // At the fee the quoter itself used, the integer walk should be exact.
                let fee_used = u32::from(q.fee);
                let mut walk_pool = pool.clone();
                walk_pool.exact = Some(ExactState::Algebra {
                    sqrt_price: match &pool.exact {
                        Some(ExactState::Algebra { sqrt_price, .. }) => *sqrt_price,
                        _ => unreachable!(),
                    },
                    tick: match &pool.exact {
                        Some(ExactState::Algebra { tick, .. }) => *tick,
                        _ => unreachable!(),
                    },
                    liquidity: match &pool.exact {
                        Some(ExactState::Algebra { liquidity, .. }) => *liquidity,
                        _ => unreachable!(),
                    },
                    max_fee_zero_for_one: fee_used,
                    max_fee_one_for_zero: fee_used,
                });
                if let Some((at_fee, _)) = walk_pool.exact_amount_out_bound(token_in, amount) {
                    assert_eq!(
                        at_fee, q.amountOut,
                        "at the quoter's own fee {fee_used}: pool {addr}, amount {amount}"
                    );
                    equal_at_current_fee += 1;
                }
            }
        }
    }
    println!(
        "block {number}: {compared} sizes; lower bound held on all ({tight} within 0.1%); {equal_at_current_fee} equal to the unit at the quoter's own fee"
    );
    assert!(compared >= 6, "too few comparable sizes ({compared})");
}
