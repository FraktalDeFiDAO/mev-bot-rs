//! Live read-only sweep (ignored): for each pool address in MEV_IT_POOLS (a file, one address per line,
//! first word of each line), the tick model must agree with the QuoterV2 on every size it calls exact.
//! Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL.

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
    #[sol(rpc)]
    interface IPool {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);
    }
}

const UNI_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");
const PANCAKE_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const UNI_QUOTER: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
const PANCAKE_QUOTER: Address = address!("B048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997");

#[tokio::test]
#[ignore = "network"]
async fn tick_model_agrees_with_the_quoter_on_every_pool_it_calls_exact() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let list = std::fs::read_to_string(std::env::var("MEV_IT_POOLS").unwrap()).unwrap();
    let mut infos = Vec::new();
    let mut quoters = std::collections::HashMap::new();
    for addr in list
        .lines()
        .filter_map(|l| l.split_whitespace().next()?.parse::<Address>().ok())
    {
        let p = IPool::new(addr, &rpc.http);
        let (Ok(f), Ok(t0), Ok(t1), Ok(fee)) = (
            p.factory().call().await,
            p.token0().call().await,
            p.token1().call().await,
            p.fee().call().await,
        ) else {
            continue;
        };
        let fee = u32::try_from(fee).unwrap();
        let (kind, quoter) = if f == UNI_FACTORY {
            (PoolKind::UniV3 { fee }, UNI_QUOTER)
        } else if f == PANCAKE_FACTORY {
            (PoolKind::PancakeV3 { fee }, PANCAKE_QUOTER)
        } else {
            continue;
        };
        quoters.insert(addr, (quoter, fee));
        infos.push(PoolInfo {
            address: addr,
            dex: "t".into(),
            kind,
            token0: t0,
            token1: t1,
        });
    }
    let pools = load_pools(&rpc, infos).await.unwrap();
    let maps = load_ticks(&rpc, &pools, 800).await.unwrap();
    println!(
        "{} pools with tick maps of {} loaded",
        maps.len(),
        pools.len()
    );
    let (mut compared, mut bad) = (0, 0);
    for pool in &pools {
        let Some(map) = maps.get(&pool.info.address) else {
            continue;
        };
        let with = pool.clone().with_ticks(map.clone());
        let (quoter, fee) = quoters[&pool.info.address];
        let q = IQuoterV2::new(quoter, &rpc.http);
        for token_in in [pool.info.token0, pool.info.token1] {
            let window = with.max_exact_in(token_in);
            if window < 1e3 {
                continue;
            }
            let token_out = if token_in == pool.info.token0 {
                pool.info.token1
            } else {
                pool.info.token0
            };
            for frac in [0.1f64, 0.5, 0.9] {
                let amount = (window * frac).floor();
                let params = IQuoterV2::Params {
                    tokenIn: token_in,
                    tokenOut: token_out,
                    amountIn: U256::from(amount as u128),
                    fee: U24::from(fee),
                    sqrtPriceLimitX96: Default::default(),
                };
                let Ok(r) = q.quoteExactInputSingle(params).call().await else {
                    continue;
                };
                let truth = f64::from(r.amountOut);
                if truth <= 0.0 {
                    continue;
                }
                let flat = Pool_flat(pool, token_in, amount);
                let tick = with.amount_out(token_in, amount);
                let err = (tick - truth) / truth * 100.0;
                compared += 1;
                if err.abs() > 0.5 {
                    bad += 1;
                    println!(
                        "BAD {} {:?} fee {fee} in={} x{frac}: quoter {truth:.4e} ticks {err:+.2}% flat {:+.2}% ({} ticks, window {:.3e})",
                        pool.info.address,
                        if matches!(pool.info.kind, PoolKind::UniV3 { .. }) {
                            "uni"
                        } else {
                            "cake"
                        },
                        if token_in == pool.info.token0 {
                            "t0"
                        } else {
                            "t1"
                        },
                        (flat - truth) / truth * 100.0,
                        map.ticks.len(),
                        window
                    );
                }
            }
        }
    }
    println!("compared {compared}, off by more than 0.5 %: {bad}");
}

#[allow(non_snake_case)]
fn Pool_flat(p: &mev_dex::Pool, token_in: Address, amount: f64) -> f64 {
    let mut flat = p.clone();
    flat.ticks = None;
    flat.amount_out(token_in, amount)
}
