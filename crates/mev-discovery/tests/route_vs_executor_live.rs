//! Live diagnostic (ignored, read-only): for routes the bot logged as profitable but the executor reverted,
//! compare at ONE pinned block (a) the integer model's chained output with (b) what the deployed executor
//! actually ends with (the `balance` in its NotProfitable revert, or its profit on success).
//!   MEV_IT_ROUTES=data/.routes.json ARBITRUM_HTTP_URL=... ARBITRUM_WS_URL=... \
//!     cargo test -p mev-discovery --test route_vs_executor_live -- --ignored --nocapture

use alloy::{
    primitives::{Address, U256, address},
    providers::Provider,
    rpc::types::TransactionRequest,
    sol,
};
use mev_discovery::{load_pools_at, ticks::load_ticks_at};
use mev_types::{Hop, Opportunity, PoolInfo, PoolKind};

sol! {
    #[sol(rpc)]
    interface IP {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
}
const UNI: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");
const PANCAKE: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const SUSHI: Address = address!("1af415a1EbA07a4986a52B6f2e7dE7003D82231e");
const EXECUTOR: Address = address!("17357E33012e8c05E654a7D1CC7D9dedd6B63acf");
const OWNER: Address = address!("e7322F17Ed69B9236f5103f8F62B302Cbf984447");

#[tokio::test]
#[ignore = "network"]
async fn model_versus_executor_on_logged_routes() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let routes: Vec<serde_json::Value> = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("MEV_IT_ROUTES").unwrap()).unwrap(),
    )
    .unwrap();
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    println!("pinned block {number}");
    for r in routes {
        let amount: U256 = r["amount"].as_str().unwrap().parse().unwrap();
        let mut hops = Vec::new();
        let mut infos = Vec::new();
        let mut ok = true;
        for h in r["route"].as_array().unwrap() {
            let pool: Address = h["pool"].as_str().unwrap().parse().unwrap();
            let (tin, tout): (Address, Address) = (
                h["in"].as_str().unwrap().parse().unwrap(),
                h["out"].as_str().unwrap().parse().unwrap(),
            );
            let p = IP::new(pool, &rpc.http);
            let (Ok(f), Ok(t0), Ok(t1), Ok(fee)) = (
                p.factory().call().await,
                p.token0().call().await,
                p.token1().call().await,
                p.fee().call().await,
            ) else {
                ok = false;
                break;
            };
            let fee = u32::try_from(fee).unwrap();
            let kind = if f == PANCAKE {
                PoolKind::PancakeV3 { fee }
            } else if f == UNI || f == SUSHI {
                PoolKind::UniV3 { fee }
            } else {
                ok = false;
                break;
            };
            infos.push(PoolInfo {
                address: pool,
                dex: format!("{f}"),
                kind,
                token0: t0,
                token1: t1,
            });
            hops.push(Hop {
                kind: 2,
                pool,
                token_in: tin,
                token_out: tout,
                zero_for_one: tin == t0,
                fee_bps: 0,
                spacing: 0,
            });
        }
        if !ok {
            println!("skip (non Uniswap-family pool)");
            continue;
        }
        // Model: chain the integer walk hop by hop at the pinned block.
        let mut pools = load_pools_at(&rpc, infos, block).await.unwrap();
        let maps = load_ticks_at(&rpc, &pools, 4000, block).await.unwrap();
        for p in &mut pools {
            p.ticks = maps.get(&p.info.address).cloned().map(std::sync::Arc::new);
        }
        let mut cur = amount;
        let mut per_hop = Vec::new();
        for (h, p) in hops.iter().zip(&pools) {
            match p.exact_amount_out(h.token_in, cur) {
                Some(o) => {
                    cur = o;
                    per_hop.push(o.to_string());
                }
                None => {
                    per_hop.push("n/a".into());
                    cur = U256::ZERO;
                    break;
                }
            }
        }
        let opp = Opportunity {
            loan_token: hops[0].token_in,
            amount_in: amount,
            hops,
            est_profit: 0.0,
            est_profit_usd: 0.0,
            buy_pool: Address::ZERO,
            sell_pool: Address::ZERO,
            exact: false,
            versions: vec![],
        };
        // Executor at the same block.
        let req = TransactionRequest::default()
            .from(OWNER)
            .to(EXECUTOR)
            .input(mev_bindings::arb_calldata(&opp, U256::ZERO, true).into());
        let res = rpc.http.call(req).block(block).await;
        let exec = match res {
            Ok(b) => format!("OK profit-bytes {}", b.len()),
            Err(e) => {
                let s = e.to_string();
                match s.find("0x843e30ec") {
                    Some(i) => {
                        let h = &s[i + 10..];
                        let w =
                            |n: usize| U256::from_str_radix(&h[n * 64..n * 64 + 64], 16).unwrap();
                        format!("NotProfitable balance={} required={}", w(0), w(1))
                    }
                    None => s.chars().take(110).collect(),
                }
            }
        };
        println!(
            "amount {amount} | model chain {per_hop:?} (final {cur}, {:+}) | executor: {exec}",
            i128::try_from(cur).unwrap_or(0) - i128::try_from(amount).unwrap_or(0)
        );
    }
}
