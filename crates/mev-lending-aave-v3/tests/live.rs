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
