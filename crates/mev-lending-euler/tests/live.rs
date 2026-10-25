//! Live read-only check against Euler V2 on Base and Arbitrum (ignored by default; network).

use std::path::Path;

use alloy::primitives::address;
use mev_lending::{AccountRef, AdapterRpc, LiquidationAdapter};
use mev_lending_euler::EulerAdapter;

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
async fn starts_on_both_chains_and_reads_a_real_borrower() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    for (file, url) in [
        ("base.toml", "https://base-rpc.publicnode.com"),
        ("arbitrum.toml", "https://arb1.arbitrum.io/rpc"),
    ] {
        let e = mev_config::ChainConfig::load(&root.join(file))
            .unwrap()
            .euler
            .unwrap();
        let a = EulerAdapter::new(
            e,
            rpc(url),
            std::env::temp_dir().join(format!("euler_{file}.json")),
        )
        .await
        .unwrap_or_else(|x| panic!("{file}: {x:#}"));
        assert!(a.targets().len() > 10, "{file}: controllers listed");
    }
    // Base: an AERO borrower of eAERO-1 (HF ~1.5 on 2026-10-25), and nobody.
    let e = mev_config::ChainConfig::load(&root.join("base.toml"))
        .unwrap()
        .euler
        .unwrap();
    let a = EulerAdapter::new(
        e,
        rpc("https://base-rpc.publicnode.com"),
        std::env::temp_dir().join("euler_b.json"),
    )
    .await
    .unwrap();
    let vault = address!("5fe2de3e565a6a501a4ec44aab8664b1d674ac25");
    let refs = [
        address!("65c2550c288a98ea1ac8d4679ac900899c5f13ef"),
        address!("000000000000000000000000000000000000dEaD"),
    ]
    .map(|account| AccountRef {
        market: vault,
        account,
        sub_id: 0,
    });
    let h = a.health(&refs, true).await.unwrap();
    assert_eq!(h.len(), 2, "one answer per account");
    assert!(h[1].health_factor.is_infinite() && h[1].debt_usd == 0.0);
    if h[0].debt_usd > 0.0 {
        assert!(h[0].health_factor.is_finite() && h[0].collateral_usd > 0.0);
    }
}
