//! Live read-only check of the trigger indexer against Arbitrum mainnet (ignored by default).
//! Run: scripts/dev.sh cargo test -p mev-lending-aave-v3 --test triggers_live -- --ignored --nocapture

use std::{path::Path, sync::Arc, time::Duration};

use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::Filter,
};
use mev_bindings::IAavePool;
use mev_lending::{AdapterRpc, LiquidationAdapter};
use mev_lending_aave_v3::{
    AaveV3Adapter,
    events::{self, PoolEvent},
    triggers::{AaveTriggerIndexer, TriggerConfig},
};
use mev_liq_state::*;

const CHAIN: u64 = 42161;
/// A real 1,600 block window (Arbitrum One) holding Supply, Withdraw, Repay, Borrow,
/// ReserveDataUpdated and collateral-toggle logs of the Aave Pool.
const START: u64 = 514_400_000;
const WINDOW: u64 = 1_600;

fn providers() -> (DynProvider, DynProvider) {
    let call = std::env::var("MEV_IT_RPC")
        .unwrap_or_else(|_| "https://arbitrum-one-rpc.publicnode.com".into());
    let logs =
        std::env::var("MEV_IT_LOG_RPC").unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".into());
    (
        mev_rpc::http_provider(&logs).unwrap(),
        mev_rpc::http_provider(&call).unwrap(),
    )
}

fn aave() -> mev_config::AaveConfig {
    mev_config::ChainConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
    )
    .unwrap()
    .aave
    .unwrap()
}

fn reasons(repo: &SnapshotRepository, who: Address) -> Vec<DirtyReason> {
    repo.dirty_entries()
        .unwrap()
        .into_iter()
        .find(|e| e.key.borrower == who)
        .map(|e| e.reasons)
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "network"]
async fn real_window_yields_the_expected_dirty_set_and_reverse_index() {
    let cfg = aave();
    let (scan, fast) = providers();
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::new(SnapshotRepository::open(&dir.path().join("live.db")).unwrap());

    // Ground truth: decode the same window independently of the indexer.
    let filter = Filter::new()
        .address(vec![cfg.pool, cfg.oracle])
        .event_signature(events::topics())
        .from_block(START)
        .to_block(START + WINDOW - 1);
    let logs = scan.get_logs(&filter).await.unwrap();
    let evs: Vec<PoolEvent> = logs
        .iter()
        .filter_map(|l| events::decode(l, cfg.pool, cfg.oracle).unwrap())
        .collect();
    let count = |f: fn(&PoolEvent) -> bool| evs.iter().filter(|e| f(e)).count();
    let (borrows, repays, supplies, withdraws, rdu) = (
        count(|e| matches!(e, PoolEvent::Borrow { .. })),
        count(|e| matches!(e, PoolEvent::Repay { .. })),
        count(|e| matches!(e, PoolEvent::Supply { .. })),
        count(|e| matches!(e, PoolEvent::Withdraw { .. })),
        count(|e| matches!(e, PoolEvent::ReserveDataUpdated { .. })),
    );
    println!(
        "window {START}..{}: {} logs, borrow {borrows} repay {repays} supply {supplies} withdraw {withdraws} reserveDataUpdated {rdu}",
        START + WINDOW - 1,
        evs.len()
    );
    assert!(borrows > 0 && repays > 0 && supplies > 0 && withdraws > 0 && rdu > 0);

    // Start the cursor at the window and let exactly one range request run.
    let tcfg = TriggerConfig {
        step: WINDOW,
        max_requests: 1,
        throttle: Duration::ZERO,
        reconcile_batch: 30,
        ..TriggerConfig::for_pool(&cfg, CHAIN)
    };
    let before = scan
        .get_block_by_number((START - 1).into())
        .await
        .unwrap()
        .unwrap();
    repo.init_scan_cursor(
        &tcfg.name,
        ScanCursor {
            next_block: START,
            tip: Some(ChainPoint {
                block_number: START - 1,
                block_hash: before.header.hash,
            }),
            back_block: START, // no history scan in this test
        },
    )
    .unwrap();
    let mut idx = AaveTriggerIndexer::new(
        cfg.clone(),
        tcfg.clone(),
        scan.clone(),
        fast.clone(),
        repo.clone(),
    )
    .await
    .unwrap();
    let rep = idx.step().await.unwrap();
    println!("report: {rep:#?}");

    assert_eq!(
        rep.events,
        evs.len(),
        "indexer saw exactly the independently decoded events"
    );
    assert_eq!(rep.new_gaps, 0);
    assert!(
        !rep.upgrade_flagged,
        "pinned deployment is unchanged: {:?}",
        rep.warnings
    );
    let c = repo.scan_cursor(&tcfg.name).unwrap().unwrap();
    assert_eq!(c.next_block, START + WINDOW);
    assert_eq!(c.tip.unwrap().block_number, START + WINDOW - 1);

    // Every borrower that borrowed in the window is registered, discovered and dirty.
    let borrowers: std::collections::BTreeSet<Address> = evs
        .iter()
        .filter_map(|e| match e {
            PoolEvent::Borrow { on_behalf_of, .. } => Some(*on_behalf_of),
            _ => None,
        })
        .collect();
    for b in &borrowers {
        assert!(
            repo.borrower(CHAIN, cfg.pool, *b).unwrap().is_some(),
            "{b} registered"
        );
        let r = reasons(&repo, *b);
        assert!(
            r.contains(&DirtyReason::Discovered) && r.contains(&DirtyReason::BorrowerEvent),
            "{b}: {r:?}"
        );
    }
    // Repay/Supply/Withdraw users that are known borrowers are dirty with the event reason.
    let mut known_touched = 0;
    for e in &evs {
        let user = match e {
            PoolEvent::Repay { user, .. } | PoolEvent::Withdraw { user, .. } => Some(*user),
            PoolEvent::Supply { on_behalf_of, .. } => Some(*on_behalf_of),
            _ => None,
        };
        if let Some(u) = user
            && repo.borrower(CHAIN, cfg.pool, u).unwrap().is_some()
        {
            known_touched += 1;
            assert!(reasons(&repo, u).contains(&DirtyReason::BorrowerEvent));
        }
    }
    println!("known borrowers touched by repay/supply/withdraw: {known_touched}");
    // ReserveDataUpdated reached the borrower set (unknown composition counts as affected).
    assert!(
        borrowers
            .iter()
            .any(|b| reasons(&repo, *b).contains(&DirtyReason::ReserveConfig))
    );

    // Reconciliation read on-chain config: dependency sets equal getUserConfiguration now.
    let pool = IAavePool::new(cfg.pool, &fast);
    let mut checked = 0;
    for b in repo.active_borrowers(CHAIN, cfg.pool).unwrap() {
        let rec = repo.borrower(CHAIN, cfg.pool, b).unwrap().unwrap();
        if !rec.deps_reconciled || rec.deps_stale {
            continue;
        }
        let mask = pool.getUserConfiguration(b).call().await.unwrap();
        let expect = mev_lending_aave_v3::triggers::deps_from_user_config(mask, idx.reserves());
        let mut got = repo.borrower_dependencies(CHAIN, cfg.pool, b).unwrap();
        got.sort();
        let mut want = expect;
        want.sort();
        assert_eq!(
            got, want,
            "reverse index of {b} matches the on-chain bitmap"
        );
        checked += 1;
    }
    println!(
        "reconciled {} (deactivated {}), active borrowers verified against chain: {checked}",
        rep.reconciled, rep.deactivated
    );
    assert!(rep.reconciled > 0, "reconciliation ran: {:?}", rep.warnings);
}

#[tokio::test]
#[ignore = "network"]
async fn adapter_feed_match_marks_exposed_borrowers() {
    let cfg = aave();
    let (scan, fast) = providers();
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::new(SnapshotRepository::open(&dir.path().join("live2.db")).unwrap());
    let mut adapter = AaveV3Adapter::new(
        cfg.clone(),
        AdapterRpc {
            fast: fast.clone(),
            scan: scan.clone(),
            sweep: fast.clone(),
        },
        dir.path().join("idx.json"),
    )
    .await
    .unwrap();
    adapter
        .attach_triggers(repo.clone(), TriggerConfig::for_pool(&cfg, CHAIN))
        .await
        .unwrap();
    let feed = adapter.price_feeds()[0];
    let assets = adapter.price_update_assets(Some(feed), &[]);
    assert!(!assets.is_empty(), "a real price feed maps to reserves");
    repo.register_borrowers(CHAIN, cfg.pool, &[Address::repeat_byte(0x42)], None, 1)
        .unwrap();
    let marked = adapter.on_price_update_tx(Some(feed), &[]).unwrap();
    assert!(marked >= 1);
    assert!(reasons(&repo, Address::repeat_byte(0x42)).contains(&DirtyReason::PriceMove));
    println!("feed {feed} -> reserves {assets:?}, marked {marked}");
}
