//! Live read-only integration tests (ignored by default; scripts/test-integration.sh).

use std::path::Path;

use alloy::{
    primitives::{Address, b256},
    providers::Provider,
    rpc::types::Filter,
    sol_types::SolEvent,
};
use mev_lending::{AdapterRpc, LiquidationAdapter};
use mev_lending_morpho::{IMorpho, MorphoAdapter};

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

#[tokio::test]
#[ignore = "network"]
async fn reads_a_real_wbtc_usdc_borrower() {
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap();
    let c = cfg.morpho.unwrap();
    let r = rpc();
    let dir = tempfile::tempdir().unwrap();
    let mut a = MorphoAdapter::new(c.clone(), r.clone(), dir.path().join("idx.json"))
        .await
        .unwrap();
    let market = b256!("e6392ff19d10454b099d692b58c361ef93e31af34ed1ef78232e07c78fe99169"); // WBTC/USDC
    let head = r.scan.get_block_number().await.unwrap();
    let f = Filter::new()
        .address(c.morpho)
        .event_signature(IMorpho::Borrow::SIGNATURE_HASH)
        .topic1(market)
        .from_block(head - 2_000_000)
        .to_block(head);
    let logs = r.scan.get_logs(&f).await.unwrap();
    let user = Address::from_word(
        *logs
            .last()
            .expect("WBTC/USDC borrows")
            .topics()
            .get(2)
            .unwrap(),
    );
    a.track(market, user);
    let accounts = a.accounts();
    assert_eq!(accounts.len(), 1);
    let h = a.health(&accounts, true).await.unwrap();
    // A borrower may have repaid since; if a position exists, values must be sane.
    if let Some(h) = h.first() {
        assert!(h.health_factor > 0.0 && h.debt_usd > 0.0 && h.collateral_usd > 0.0);
        if !h.liquidatable() {
            assert!(a.plan(h, Address::ZERO).await.unwrap().is_none());
        }
    }
    assert!(a.assets().len() >= 2, "market params loaded (WBTC, USDC)");
}
