//! Live read-only integration tests against Arbitrum (ignored by default; see
//! scripts/test-integration.sh). They catch ABI/decoding drift that mocks can't.

use std::path::Path;

use alloy::{primitives::Address, providers::Provider, rpc::types::Filter, sol_types::SolEvent};
use mev_bindings::IAavePool;
use mev_lending::{AccountRef, AdapterRpc, LiquidationAdapter};
use mev_lending_aave_v3::AaveV3Adapter;

fn rpc() -> AdapterRpc {
    let call = std::env::var("MEV_IT_RPC")
        .unwrap_or_else(|_| "https://arbitrum-one-rpc.publicnode.com".into());
    let logs =
        std::env::var("MEV_IT_LOG_RPC").unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".into());
    AdapterRpc {
        fast: mev_rpc::http_provider(&call).unwrap(),
        scan: mev_rpc::http_provider(&logs).unwrap(),
        sweep: mev_rpc::http_provider(&call).unwrap(),
    }
}

fn cfg() -> mev_config::ChainConfig {
    mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "network"]
async fn adapter_reads_real_reserves_feeds_and_a_real_borrower() {
    let c = cfg().aave.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let r = rpc();
    let a = AaveV3Adapter::new(c.clone(), r.clone(), dir.path().join("idx.json"))
        .await
        .unwrap();
    assert!(a.assets().len() >= 10, "Arbitrum Aave has ~20 reserves");
    assert!(
        a.price_feeds().len() >= 5,
        "price sources resolve to aggregators"
    );
    assert_eq!(a.targets(), vec![c.pool]);

    // A recent borrower from Borrow events.
    let head = r.scan.get_block_number().await.unwrap();
    let f = Filter::new()
        .address(c.pool)
        .event_signature(IAavePool::Borrow::SIGNATURE_HASH)
        .from_block(head - 20_000)
        .to_block(head);
    let logs = r.scan.get_logs(&f).await.unwrap();
    let user = Address::from_word(
        *logs
            .last()
            .expect("borrows in last 20k blocks")
            .topics()
            .get(2)
            .unwrap(),
    );
    let h = a
        .health(
            &[AccountRef {
                market: c.pool,
                account: user,
                sub_id: 0,
            }],
            true,
        )
        .await
        .unwrap();
    assert_eq!(h.len(), 1);
    assert!(h[0].health_factor > 0.0);
    if h[0].debt_usd > 0.0 {
        assert!(h[0].collateral_usd > 0.0);
    }
    if !h[0].liquidatable() {
        assert!(
            a.plan(&h[0], Address::ZERO).await.unwrap().is_none(),
            "healthy accounts are never planned"
        );
    }
}

/// Backrun prediction on real positions: sweep the bot's cached borrowers (fills the prediction
/// snapshot), then predict at today's ETH price (calibration: ~nobody liquidatable, as on-chain)
/// and at a hypothetical -40% (plenty, with well-formed plans). Needs `data/arbitrum`'s index.
#[tokio::test]
#[ignore = "network"]
async fn predicts_liquidations_from_a_price_report_on_real_positions() {
    alloy::sol! {
        #[sol(rpc)]
        interface IAgg { function latestAnswer() external view returns (int256); }
    }
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/arbitrum/aave_v3_borrowers.json");
    if !cache.exists() {
        eprintln!("skipping: no {}", cache.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let idx = dir.path().join("aave_v3_borrowers.json");
    std::fs::copy(&cache, &idx).unwrap();
    let c = cfg().aave.unwrap();
    let r = rpc();
    let a = AaveV3Adapter::new(c, r.clone(), idx).await.unwrap();
    let accounts = a.accounts();
    let hs = a.health(&accounts, false).await.unwrap();
    let liquidatable_now = hs
        .iter()
        .filter(|h| h.health_factor < 0.9995 && h.debt_usd > 0.0)
        .count();

    // ETH/USD aggregator (WETH's Aave source resolves to it).
    let eth_usd: Address = "0xa5E1a36938769cbd5a26f5e19D8FCB379f597c83"
        .parse()
        .unwrap();
    assert!(a.price_feeds().contains(&eth_usd));
    let now: f64 = IAgg::new(eth_usd, &r.fast)
        .latestAnswer()
        .call()
        .await
        .unwrap()
        .to_string()
        .parse()
        .unwrap();

    // Same price as the snapshot: nothing newly crosses (calibration reproduces on-chain HFs).
    let same = a.predict(eth_usd, now);
    assert!(
        same.is_empty(),
        "calibration: {} predicted at today's price ({} already liquidatable on-chain)",
        same.len(),
        liquidatable_now
    );
    // ETH -40% (prices persist, so this is relative to the answer just reported).
    let crash = a.predict(eth_usd, now * 0.6);
    eprintln!(
        "{} accounts, {} at-risk swept, liquidatable now {}, predicted at today's price {}, at -40% {}",
        accounts.len(),
        hs.iter()
            .filter(|h| h.health_factor < 1.10 && h.debt_usd > 0.0)
            .count(),
        liquidatable_now,
        same.len(),
        crash.len()
    );
    assert!(
        crash.len() > same.len(),
        "an ETH crash must liquidate ETH-collateral borrowers"
    );
    for p in &crash {
        assert!(p.bonus_usd > 0.0 && p.flash_amount > alloy::primitives::U256::ZERO);
        assert_eq!(p.calls.len(), 1);
    }
}
