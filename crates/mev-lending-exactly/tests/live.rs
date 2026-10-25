//! Live read-only check against Exactly on Base (ignored by default; network).

use std::path::Path;

use alloy::primitives::address;
use mev_lending::{AccountRef, AdapterRpc, LiquidationAdapter};
use mev_lending_exactly::ExactlyAdapter;

#[tokio::test]
#[ignore = "network"]
async fn starts_and_reads_health_of_recently_liquidated_borrowers() {
    let cfg = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/base.toml"),
    )
    .unwrap();
    let x = cfg.exactly.unwrap();
    let p = mev_rpc::http_provider("https://base-rpc.publicnode.com").unwrap();
    let rpc = AdapterRpc {
        fast: p.clone(),
        scan: p.clone(),
        sweep: p,
    };
    let a = ExactlyAdapter::new(
        x.clone(),
        rpc,
        std::env::temp_dir().join("exactly_live.json"),
    )
    .await
    .unwrap();
    assert!(a.targets().len() >= 5, "listed markets");
    assert!(
        !a.price_feeds().is_empty(),
        "Chainlink aggregators resolved"
    );
    // Borrowers liquidated on 2026-10-24 (block 53,032,829) and one that never borrowed.
    let who = [
        address!("c7943b1ad29a394fe775b73726025eab4cd799d4"),
        address!("7df456ee958433c5dd585679fdb59933016ff92b"),
        address!("000000000000000000000000000000000000dEaD"),
    ];
    let refs: Vec<AccountRef> = who
        .iter()
        .map(|a| AccountRef {
            market: x.auditor,
            account: *a,
            sub_id: 0,
        })
        .collect();
    let h = a.health(&refs, true).await.unwrap();
    assert_eq!(h.len(), 3, "one answer per account");
    assert!(h[2].health_factor.is_infinite() && h[2].debt_usd == 0.0);
    for x in &h[..2] {
        if x.debt_usd > 0.0 {
            assert!(x.health_factor.is_finite() && x.collateral_usd > 0.0);
        }
    }
}
