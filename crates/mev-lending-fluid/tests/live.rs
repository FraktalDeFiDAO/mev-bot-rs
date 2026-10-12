//! Live read-only integration tests (ignored by default; scripts/test-integration.sh).

use std::path::Path;

use mev_lending::{AdapterRpc, LiquidationAdapter};
use mev_lending_fluid::FluidAdapter;

fn rpc() -> AdapterRpc {
    let call = std::env::var("MEV_IT_RPC")
        .unwrap_or_else(|_| "https://arbitrum-one-rpc.publicnode.com".into());
    let p = mev_rpc::http_provider(&call).unwrap();
    AdapterRpc {
        fast: p.clone(),
        scan: p.clone(),
        sweep: p,
    }
}

#[tokio::test]
#[ignore = "network"]
async fn resolver_quotes_decode_for_configured_vaults() {
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap();
    let c = cfg.fluid.unwrap();
    let a = FluidAdapter::new(c.clone(), rpc()).await.unwrap();
    assert!(
        c.vaults.iter().all(|v| a.targets().contains(v)),
        "configured vaults are watched"
    );
    assert!(
        !c.auto_vaults || a.targets().len() > c.vaults.len(),
        "auto_vaults adds the resolver's T1 vaults"
    );
    assert!(!a.assets().is_empty(), "vault tokens resolved");
    assert!(
        !a.price_feeds().is_empty(),
        "vault tokens map to Chainlink aggregators"
    );
    let hs = a.health(&a.accounts(), true).await.unwrap();
    println!(
        "{} vaults watched, {} decode a liquidation quote",
        a.targets().len(),
        hs.len()
    );
    assert!(
        hs.len() >= c.vaults.len().min(1),
        "at least the configured vaults decode"
    );
    for h in hs {
        assert!(h.debt_usd >= 0.0 && h.collateral_usd >= 0.0);
    }
}
