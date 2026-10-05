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

/// Backrun prediction on real positions: sweep the bot's cached Comet accounts (fills the
/// snapshot), then for every collateral price feed predict at today's answer (calibration: nothing
/// newly crosses) and at -40 % (borrowers on volatile collateral cross, with well-formed plans).
#[tokio::test]
#[ignore = "network"]
async fn predicts_absorbs_from_a_price_report_on_real_positions() {
    alloy::sol! {
        #[sol(rpc)]
        interface IAgg { function latestAnswer() external view returns (int256); }
    }
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/arbitrum/compound_v3_borrowers.json");
    if !cache.exists() {
        eprintln!("skipping: no {}", cache.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let idx = dir.path().join("compound_v3_borrowers.json");
    std::fs::copy(&cache, &idx).unwrap();
    let c = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap()
    .compound_v3
    .unwrap();
    let r = rpc();
    let a = CompoundV3Adapter::new(c, r.clone(), idx).await.unwrap();
    let accounts = a.accounts();
    a.health(&accounts, false).await.unwrap();
    eprintln!("snapshot: {:?}", a.snapshot_summary());
    let exec = Address::repeat_byte(0xee);
    let (mut same, mut crash) = (0, 0);
    for feed in a.price_feeds() {
        let Ok(now) = IAgg::new(feed, &r.fast).latestAnswer().call().await else {
            continue;
        };
        let now: f64 = now.to_string().parse().unwrap();
        same += a.predict(feed, now, exec).len();
        let plans = a.predict(feed, now * 0.6, exec);
        for p in &plans {
            assert!(p.flash_amount > alloy::primitives::U256::ZERO && p.bonus_usd > 0.0);
            assert_eq!(p.calls.len(), 2, "absorb + buyCollateral");
            let absorb = IComet::absorbCall::abi_decode(&p.calls[0].data).unwrap();
            assert_eq!(
                (absorb.absorber, absorb.accounts[0]),
                (exec, p.account.account)
            );
        }
        crash += plans.len();
        // Restore the snapshot's answer so the next feed starts from today's state.
        a.predict(feed, now, exec);
    }
    eprintln!(
        "{} accounts, predicted at today's price {same}, at -40% {crash}",
        accounts.len()
    );
    assert_eq!(
        same, 0,
        "calibration: nothing newly crosses at today's prices"
    );
    let (_, answers, known, links) = a.snapshot_summary();
    eprintln!("answers {answers}, links with a measured elasticity {known}/{links}");
    assert!(
        answers > 0,
        "aggregator answers must be readable for the baseline"
    );
    assert!(known > 0, "plain proxies are classified from one sweep");
}
