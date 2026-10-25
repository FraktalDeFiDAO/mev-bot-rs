//! Live read-only check of every configured Aave v3 fork (ignored by default; network).

use std::path::Path;

use alloy::primitives::address;
use mev_lending::{AccountRef, AdapterRpc, LiquidationAdapter};
use mev_lending_aave_v3::AaveV3Adapter;

#[tokio::test]
#[ignore = "network"]
async fn every_configured_fork_is_pinned_starts_and_reads_health() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    for (file, url) in [
        ("arbitrum.toml", "https://arb1.arbitrum.io/rpc"),
        ("base.toml", "https://base-rpc.publicnode.com"),
    ] {
        let p = mev_rpc::http_provider(url).unwrap();
        let rpc = AdapterRpc {
            fast: p.clone(),
            scan: p.clone(),
            sweep: p.clone(),
        };
        let cfg = mev_config::ChainConfig::load(&root.join(file)).unwrap();
        for f in cfg.aave_forks.iter().filter(|f| f.enabled) {
            let name = f.name.clone().unwrap();
            mev_lending_aave_v3::deployment::verify_deployment(&p, f)
                .await
                .unwrap_or_else(|e| panic!("{name}: pin: {e}"));
            let a = AaveV3Adapter::new(
                f.clone(),
                rpc.clone(),
                std::env::temp_dir().join(format!("{name}_live.json")),
            )
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(a.id(), name.as_str());
            let nobody = AccountRef {
                market: f.pool,
                account: address!("000000000000000000000000000000000000dEaD"),
                sub_id: 0,
            };
            let h = a.health(&[nobody], true).await.unwrap();
            assert_eq!(h.len(), 1, "{name}: no answer");
        }
    }
}
