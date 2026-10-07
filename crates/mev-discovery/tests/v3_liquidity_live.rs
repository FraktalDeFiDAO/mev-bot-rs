//! Live read-only check (ignored): real V3-style `Swap`/`Mint`/`Burn` logs applied in order to a pool's
//! state at block N-1 must reproduce the chain's liquidity and price at block N (Uniswap V3, Pancake V3
//! and Algebra v1 pools). Needs ARBITRUM_HTTP_URL / ARBITRUM_WS_URL and MEV_IT_POOLS (a file, one pool
//! address per line, first word of each line).

use alloy::{
    eips::BlockId,
    primitives::{Address, U256, address},
    providers::Provider,
    rpc::types::Filter,
    sol_types::SolEvent,
};
use mev_bindings::{IAlgebraPool, IPancakeV3Pool, IUniswapV3Pool};
use mev_dex::{PoolState, sqrt_price_from_x96};
use mev_state::PoolStore;
use mev_types::{PoolInfo, PoolKind};

alloy::sol! {
    #[sol(rpc)]
    interface IPoolMeta2 {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
}

const UNI_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");
const PANCAKE_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const CAMELOT_ALGEBRA_FACTORY: Address = address!("1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B");

async fn chain_state(
    rpc: &mev_rpc::Rpc,
    pool: Address,
    kind: PoolKind,
    at: u64,
) -> Option<(f64, f64)> {
    let b = BlockId::number(at);
    let liq = IUniswapV3Pool::new(pool, &rpc.http)
        .liquidity()
        .block(b)
        .call()
        .await
        .ok()?;
    let price = match kind {
        PoolKind::AlgebraV3 => {
            IAlgebraPool::new(pool, &rpc.http)
                .globalState()
                .block(b)
                .call()
                .await
                .ok()?
                .price
        }
        PoolKind::PancakeV3 { .. } => {
            IPancakeV3Pool::new(pool, &rpc.http)
                .slot0()
                .block(b)
                .call()
                .await
                .ok()?
                .sqrtPriceX96
        }
        _ => {
            IUniswapV3Pool::new(pool, &rpc.http)
                .slot0()
                .block(b)
                .call()
                .await
                .ok()?
                .sqrtPriceX96
        }
    };
    Some((sqrt_price_from_x96(f64::from(price)), liq as f64))
}

#[tokio::test]
#[ignore = "network"]
async fn real_swap_mint_burn_logs_reproduce_the_chains_liquidity() {
    let http = std::env::var("ARBITRUM_HTTP_URL").unwrap();
    let ws = std::env::var("ARBITRUM_WS_URL").unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let list = std::fs::read_to_string(std::env::var("MEV_IT_POOLS").unwrap()).unwrap();
    let mut store = PoolStore::default();
    let mut kinds = std::collections::HashMap::new();
    for addr in list
        .lines()
        .filter_map(|l| l.split_whitespace().next()?.parse::<Address>().ok())
    {
        let p = IPoolMeta2::new(addr, &rpc.http);
        let (Ok(f), Ok(t0), Ok(t1)) = (
            p.factory().call().await,
            p.token0().call().await,
            p.token1().call().await,
        ) else {
            continue;
        };
        let fee = p
            .fee()
            .call()
            .await
            .ok()
            .map(|f| u32::try_from(f).unwrap())
            .unwrap_or(0);
        let kind = if f == UNI_FACTORY {
            PoolKind::UniV3 { fee }
        } else if f == PANCAKE_FACTORY {
            PoolKind::PancakeV3 { fee }
        } else if f == CAMELOT_ALGEBRA_FACTORY {
            PoolKind::AlgebraV3
        } else {
            continue;
        };
        kinds.insert(addr, kind);
        let info = PoolInfo {
            address: addr,
            dex: "t".into(),
            kind,
            token0: t0,
            token1: t1,
        };
        store.insert(mev_dex::Pool::new(
            info,
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 0.0,
            },
        ));
    }
    let pools: Vec<Address> = kinds.keys().copied().collect();
    println!("{} V3-style pools", pools.len());
    let head = rpc.http.get_block_number().await.unwrap();
    let topics = vec![
        IUniswapV3Pool::Swap::SIGNATURE_HASH,
        IPancakeV3Pool::Swap::SIGNATURE_HASH,
        IUniswapV3Pool::Mint::SIGNATURE_HASH,
        IUniswapV3Pool::Burn::SIGNATURE_HASH,
    ];
    let mut by_pool_block: std::collections::BTreeMap<(Address, u64), Vec<alloy::rpc::types::Log>> =
        Default::default();
    let mut to = head;
    while to > head.saturating_sub(3_000) {
        let from = to.saturating_sub(499);
        let f = Filter::new()
            .address(pools.clone())
            .event_signature(topics.clone())
            .from_block(from)
            .to_block(to);
        to = from.saturating_sub(1);
        let Ok(logs) = rpc.http.get_logs(&f).await else {
            continue;
        };
        for l in logs {
            if let Some(b) = l.block_number {
                by_pool_block.entry((l.address(), b)).or_default().push(l);
            }
        }
    }
    let (mut compared, mut with_liq_event, mut bad) = (0, 0, 0);
    for ((pool, block), mut logs) in by_pool_block {
        let has_liq = logs.iter().any(|l| {
            l.topic0() == Some(&IUniswapV3Pool::Mint::SIGNATURE_HASH)
                || l.topic0() == Some(&IUniswapV3Pool::Burn::SIGNATURE_HASH)
        });
        if !has_liq || compared >= 40 {
            continue;
        }
        let kind = kinds[&pool];
        let (Some((p0, l0)), Some((p1, l1))) = (
            chain_state(&rpc, pool, kind, block - 1).await,
            chain_state(&rpc, pool, kind, block).await,
        ) else {
            continue;
        };
        store.set_state(
            &pool,
            PoolState::V3 {
                sqrt_price: p0,
                liquidity: l0,
            },
        );
        logs.sort_by_key(|l| l.log_index);
        for l in &logs {
            store.apply_log(l);
        }
        let PoolState::V3 {
            sqrt_price,
            liquidity,
        } = store.get(&pool).unwrap().state
        else {
            panic!()
        };
        let ok = (liquidity - l1).abs() <= 1e-9 * l1.max(1.0)
            && (sqrt_price - p1).abs() <= 1e-12 * p1.max(1e-30);
        compared += 1;
        with_liq_event += 1;
        if !ok {
            bad += 1;
            println!(
                "MISMATCH {pool} block {block} {kind:?}: liq model {liquidity:.6e} chain {l1:.6e} (before {l0:.6e}); price model {sqrt_price:.9e} chain {p1:.9e}; logs {}",
                logs.len()
            );
        }
    }
    println!("compared {compared} (pool, block) pairs with a Mint/Burn; mismatches {bad}");
    assert!(
        with_liq_event >= 5,
        "found Mint/Burn blocks: {with_liq_event}"
    );
    assert_eq!(
        bad, 0,
        "the model must reproduce the chain after every block"
    );
    let _ = U256::ZERO;
}
