//! Live read-only integration tests (ignored by default; scripts/test-integration.sh).

use std::path::Path;

use alloy::{primitives::Address, sol_types::SolCall};
use mev_lending::{AdapterRpc, LiquidationAdapter};
use mev_lending_compound_v3::{CompoundV3Adapter, IComet, STOREFRONT};

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
async fn reads_all_comets_storefronts_and_builds_valid_plans() {
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap();
    let c = cfg.compound_v3.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = CompoundV3Adapter::new(c.clone(), rpc(), dir.path().join("idx.json"))
        .await
        .unwrap();
    assert_eq!(a.targets().len(), c.comets.len());
    let storefronts: Vec<_> = a
        .accounts()
        .into_iter()
        .filter(|x| x.sub_id == STOREFRONT)
        .collect();
    assert!(
        storefronts.len() >= 20,
        "4 Comets list ~29 collateral assets"
    );

    let hs = a.health(&storefronts, true).await.unwrap();
    assert_eq!(hs.len(), storefronts.len(), "every storefront decodes");
    for h in &hs {
        assert!(h.debt_usd >= 0.0 && h.debt_usd.is_finite());
    }
    // Plan any storefront the adapter flags; the encoded call must be a valid buyCollateral.
    let exec = Address::with_last_byte(0xee);
    for h in hs.iter().filter(|h| h.liquidatable()) {
        if let Some(p) = a.plan(h, exec).await.unwrap() {
            let call = p.calls.last().unwrap();
            let d = IComet::buyCollateralCall::abi_decode(&call.data).unwrap();
            assert_eq!(d.recipient, exec);
            assert_eq!(d.baseAmount, p.flash_amount);
            assert_eq!(call.approve_amount, p.flash_amount);
        }
    }
}
