//! Live read-only differential test (ignored): the integer swap math for hookless Uniswap V4 pools must equal
//! the V4 Quoter to the unit at one pinned block, including flipped-orientation pools. Discovers pools with the
//! bot's own V4 discovery (cached registry in a temp file; the first run scans the manager's history).
//!   ARBITRUM_HTTP_URL=... ARBITRUM_WS_URL=... cargo test -p mev-discovery --test exact_v4_live -- --ignored --nocapture

use alloy::{
    primitives::{
        Address, Bytes, U256, address,
        aliases::{I24, U24},
    },
    sol,
};
use mev_discovery::{load_pools_at, ticks::load_ticks_at, v4};
use mev_types::PoolKind;

sol! {
    struct PoolKeyQ { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct QuoteExactSingleParams { PoolKeyQ poolKey; bool zeroForOne; uint128 exactAmount; bytes hookData; }
    #[sol(rpc)]
    interface IV4Quoter {
        function quoteExactInputSingle(QuoteExactSingleParams memory params) external returns (uint256 amountOut, uint256 gasEstimate);
    }
}

const V4_QUOTER: Address = address!("3972C00f7ed4885e145823eb7C655375d275A1C5");

#[tokio::test]
#[ignore = "network"]
async fn v4_integer_math_equals_the_quoter() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let cfg =
        mev_config::ChainConfig::load(std::path::Path::new("../../config/arbitrum.toml")).unwrap();
    let v4cfg = cfg.uniswap_v4.clone().expect("[uniswap_v4]");
    let registry_path = std::path::PathBuf::from("../../data/.exact-v4-test-registry.json");
    let infos = v4::discover(&cfg, &v4cfg, &registry_path).await;
    assert!(!infos.is_empty(), "no active V4 pools discovered");
    let registry = v4::Registry::load(&registry_path);
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let quoter = IV4Quoter::new(V4_QUOTER, &rpc.http);
    let (mut compared, mut flipped_seen, mut pools_used, mut mismatches) = (0, 0, 0, 0);
    for info in infos.into_iter().take(40) {
        let PoolKind::UniV4 { id, flipped, .. } = info.kind else {
            continue;
        };
        let Some(key) = registry.pools.iter().find(|p| p.id == id) else {
            continue;
        };
        let Ok(mut pools) = load_pools_at(&rpc, vec![info.clone()], block).await else {
            continue;
        };
        let Some(mut pool) = pools.pop() else {
            continue;
        };
        let Ok(maps) = load_ticks_at(&rpc, std::slice::from_ref(&pool), 4000, block).await else {
            continue;
        };
        let Some(map) = maps.into_values().next() else {
            continue;
        };
        pool.ticks = Some(std::sync::Arc::new(map));
        let mut used = false;
        for token_in in [info.token0, info.token1] {
            let native_zfo = (token_in == info.token0) != flipped;
            for k in 3..=24u32 {
                let amount = U256::from(10u8).pow(U256::from(k));
                let Some(mine) = pool.exact_amount_out(token_in, amount) else {
                    continue;
                };
                let Ok(exact_amount) = u128::try_from(amount) else {
                    continue;
                };
                let q = quoter
                    .quoteExactInputSingle(QuoteExactSingleParams {
                        poolKey: PoolKeyQ {
                            currency0: key.currency0,
                            currency1: key.currency1,
                            fee: U24::from(key.fee),
                            tickSpacing: I24::try_from(key.tick_spacing as i32).unwrap(),
                            hooks: Address::ZERO,
                        },
                        zeroForOne: native_zfo,
                        exactAmount: exact_amount,
                        hookData: Bytes::new(),
                    })
                    .block(block)
                    .call()
                    .await;
                let Ok(q) = q else { continue };
                if mine != q.amountOut {
                    println!(
                        "MISMATCH pool {id} flipped {flipped} zfo {native_zfo} amount {amount} mine {mine} quoter {} exact {:?} fees {}/{}",
                        q.amountOut, pool.exact, pool.fee0, pool.fee1
                    );
                    mismatches += 1;
                }
                compared += 1;
                used = true;
            }
        }
        if used {
            pools_used += 1;
            flipped_seen += usize::from(flipped);
        }
    }
    println!(
        "block {number}: {compared} V4 sizes equal to the unit across {pools_used} pools ({flipped_seen} flipped)"
    );
    assert_eq!(mismatches, 0, "integer math differs from the quoter");
    assert!(compared >= 10, "too few comparable sizes ({compared})");
}
