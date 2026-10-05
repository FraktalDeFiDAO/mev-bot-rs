//! Live read-only integration tests (ignored by default; scripts/test-integration.sh).

use std::path::Path;

use alloy::{
    primitives::{Address, U256, b256},
    providers::Provider,
    rpc::types::Filter,
    sol_types::{SolCall, SolEvent},
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

/// Backrun prediction on real positions: sweep the bot's cached Morpho borrowers (fills the
/// snapshot), then for every predictable aggregator predict at today's answer (calibration: nothing
/// newly crosses) and at ±40 % (base feeds push collateral down on −40 %, quote feeds on +40 %).
#[tokio::test]
#[ignore = "network"]
async fn predicts_liquidations_from_a_price_report_on_real_positions() {
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/arbitrum/morpho_borrowers.json");
    if !cache.exists() {
        eprintln!("skipping: no {}", cache.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let idx = dir.path().join("morpho_borrowers.json");
    std::fs::copy(&cache, &idx).unwrap();
    let c = mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap()
    .morpho
    .unwrap();
    let a = MorphoAdapter::new(c, rpc(), idx).await.unwrap();
    let accounts = a.accounts();
    a.health(&accounts, false).await.unwrap();
    eprintln!(
        "snapshot (feeds, markets, at-risk, answers): {:?}",
        a.snapshot_summary()
    );
    let exec = Address::repeat_byte(0xee);
    let (mut same, mut moved, mut unpriced, mut readable, mut exposed) = (0, 0, 0, 0, 0);
    let watched: std::collections::HashSet<Address> = a.price_feeds().into_iter().collect();
    let predictable = a.predictable_feeds();
    eprintln!(
        "predictable {} of which watched {}",
        predictable.len(),
        predictable.iter().filter(|f| watched.contains(*f)).count()
    );
    for feed in predictable {
        // The sweep's baseline (read once, by direct eth_call); none = never predicted.
        let Some(now) = a.last_answer(feed) else {
            continue;
        };
        readable += 1;
        exposed += a.exposed_positions(feed);
        same += a.predict(feed, now, exec).len();
        for shock in [0.6, 1.4] {
            let plans = a.predict(feed, now * shock, exec);
            for p in &plans {
                // bonus_usd is 0 when the USD oracle doesn't price the loan token, and can be
                // slightly negative for dust (rounded-up repayment); the engine skips both.
                assert!(p.flash_amount > U256::ZERO);
                unpriced += usize::from(p.bonus_usd <= 0.0);
                let call = IMorpho::liquidateCall::abi_decode(&p.calls[0].data).unwrap();
                assert_eq!(call.borrower, p.account.account);
            }
            moved += plans.len();
            a.predict(feed, now, exec); // restore today's state for the next shock / feed
        }
    }
    eprintln!(
        "{} accounts; readable feeds {readable}, exposed positions {exposed}; predicted at today's prices {same}, at ±40 % {moved} ({unpriced} worthless/unpriced)",
        accounts.len()
    );
    assert_eq!(
        same, 0,
        "calibration: nothing newly crosses at today's prices"
    );
}
