//! Live read-only check of every configured Compound v2 fork (ignored by default; network).

use std::path::Path;

use alloy::primitives::address;
use mev_lending::{AccountRef, AdapterRpc, LiquidationAdapter};
use mev_lending_moonwell::MoonwellAdapter;

fn rpc(url: &str) -> AdapterRpc {
    let p = mev_rpc::http_provider(url).unwrap();
    AdapterRpc {
        fast: p.clone(),
        scan: p.clone(),
        sweep: p,
    }
}

#[tokio::test]
#[ignore = "network"]
async fn every_configured_fork_starts_and_reads_health() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let chains = [
        ("arbitrum.toml", "https://arb1.arbitrum.io/rpc"),
        ("base.toml", "https://base-rpc.publicnode.com"),
    ];
    let dir = std::env::temp_dir().join("forks_live");
    for (file, url) in chains {
        let cfg = mev_config::ChainConfig::load(&root.join(file)).unwrap();
        for f in cfg.compound_v2.iter().filter(|f| f.enabled) {
            let name = f.name.clone().unwrap();
            let a = MoonwellAdapter::new(f.clone(), rpc(url), dir.join(format!("{name}.json")))
                .await
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(a.id(), name.as_str());
            assert!(!a.targets().is_empty(), "{name}: no markets");
            let nobody = AccountRef {
                market: f.comptroller,
                account: address!("000000000000000000000000000000000000dEaD"),
                sub_id: 0,
            };
            let h = a.health(&[nobody], true).await.unwrap();
            assert_eq!(h.len(), 1, "{name}: getAccountLiquidity did not answer");
            assert!(h[0].health_factor.is_infinite(), "{name}: no debt, no risk");
        }
    }
}
