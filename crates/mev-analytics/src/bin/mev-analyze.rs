//! Diagnostics: on-chain event rate for the watched pools, and how close each pair is to an arb.
//!   cargo run -p mev-analytics --bin mev-analyze -- [config/arbitrum.toml] [blocks=1200]

use std::collections::HashMap;

use alloy::{providers::Provider, rpc::types::Filter, sol_types::SolEvent};
use anyhow::Result;
use mev_bindings::{IUniswapV2Pair, IUniswapV3Pool};
use mev_config::{ChainConfig, Secrets};
use mev_rpc::Rpc;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().collect();
    let cfg = ChainConfig::load(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("config/arbitrum.toml")
            .as_ref(),
    )?;
    let blocks: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1200);
    let secrets = Secrets::from_env(&cfg.name.to_uppercase())?;
    let rpc = Rpc::connect(&secrets.http_url, &secrets.ws_url).await?;
    let usd = cfg.token(&cfg.usd_token)?;
    let native = cfg.token(&cfg.native_token)?;

    let pairs = mev_discovery::candidate_pairs(&rpc, &cfg).await?;
    let infos = mev_discovery::discover_pools(&rpc, &cfg, &pairs).await?;
    let pools = mev_discovery::load_pools(&rpc, infos).await?;
    let decimals =
        mev_discovery::token_decimals(&rpc, &cfg.tokens.values().copied().collect::<Vec<_>>())
            .await?;
    let store = mev_discovery::build_store(
        pools,
        cfg.min_pool_liquidity_usd,
        usd,
        decimals[&usd],
        native,
    );

    // 1. Event rate: what the chain actually emitted for our pools.
    let head = rpc.http.get_block_number().await?;
    let from = head.saturating_sub(blocks);
    let t_head = rpc
        .http
        .get_block_by_number(head.into())
        .await?
        .map(|b| b.header.timestamp)
        .unwrap_or(0);
    let t_from = rpc
        .http
        .get_block_by_number(from.into())
        .await?
        .map(|b| b.header.timestamp)
        .unwrap_or(0);
    let mut per_pool: HashMap<_, u64> = HashMap::new();
    let mut total = 0u64;
    let step: u64 = std::env::var("LOG_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let mut start = from;
    while start <= head {
        let end = (start + step - 1).min(head);
        let filter = Filter::new()
            .address(store.addresses())
            .event_signature(vec![
                IUniswapV2Pair::Sync::SIGNATURE_HASH,
                IUniswapV3Pool::Swap::SIGNATURE_HASH,
            ])
            .from_block(start)
            .to_block(end);
        for log in rpc.http.get_logs(&filter).await? {
            *per_pool.entry(log.address()).or_default() += 1;
            total += 1;
        }
        start = end + 1;
    }
    let mins = (t_head.saturating_sub(t_from)) as f64 / 60.0;
    println!(
        "== on-chain events for {} watched pools over {blocks} blocks ({mins:.1} min)",
        store.len()
    );
    println!(
        "total {total}  ->  {:.1} events/min",
        total as f64 / mins.max(0.01)
    );
    let mut busiest: Vec<_> = per_pool.into_iter().collect();
    busiest.sort_by_key(|b| std::cmp::Reverse(b.1));
    for (addr, n) in busiest.iter().take(8) {
        let p = store.get(addr).unwrap();
        println!(
            "  {:>5}  {:<11} {}/{}  {}",
            n,
            p.info.dex,
            cfg.symbol(p.info.token0),
            cfg.symbol(p.info.token1),
            addr
        );
    }

    // 2. How far each pair is from a profitable round trip (marginal, after pool fees).
    println!(
        "\n== best marginal round-trip edge per pair (bps, after pool fees; > 0 = arb exists)"
    );
    let spreads = mev_analytics::pair_spreads(&store);
    for s in &spreads {
        let (b, se) = (store.get(&s.buy).unwrap(), store.get(&s.sell).unwrap());
        println!(
            "  {:>8.2}  {:<7}/{:<7} pools={}  buy {} -> sell {}",
            s.best_edge_bps,
            cfg.symbol(s.token0),
            cfg.symbol(s.token1),
            s.pools,
            b.info.dex,
            se.info.dex
        );
    }
    let single: usize = store.len() - spreads.iter().map(|s| s.pools).sum::<usize>();
    println!(
        "  ({} pools are the only pool for their pair after filtering -> never searchable)",
        single
    );
    Ok(())
}
