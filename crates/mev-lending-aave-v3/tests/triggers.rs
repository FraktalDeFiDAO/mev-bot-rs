//! Event dependency indexing against real recorded Arbitrum logs (decode) and a scripted local
//! JSON-RPC chain (scan, reorg, gaps, crash-resume, rate limits, upgrade detection).

#[path = "../../mev-lending-index/tests/support/fake_node.rs"]
mod fake_node;

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use alloy::{
    primitives::{Address, B256, U256, keccak256},
    rpc::types::Log,
    sol,
    sol_types::SolCall,
};
use fake_node::{FakeNode, NodeState, chain, hash_of};
use mev_config::{AaveConfig, AaveManifest};
use mev_lending_aave_v3::{
    events::{self, PoolEvent},
    triggers::{
        AaveTriggerIndexer, Invalid, LogGuard, Mode, TriggerConfig, apply_event,
        deps_from_user_config, moved_enough,
    },
};
use mev_lending_index::RetryPolicy;
use mev_liq_state::*;
use serde_json::{Value, json};

const CHAIN: u64 = 42161;
const POOL: Address = alloy::primitives::address!("794a61358D6845594F94dc1DB02A252b5b4814aD");
const ORACLE: Address = alloy::primitives::address!("b56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7");

sol! {
    function getPool() external view returns (address);
    function getPriceOracle() external view returns (address);
}

fn fixtures() -> Vec<Value> {
    serde_json::from_slice(include_bytes!("fixtures/arbitrum_events.json")).unwrap()
}

fn fx(name: &str) -> Value {
    fixtures()
        .into_iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("fixture {name}"))
}

fn a(n: u8) -> Address {
    Address::repeat_byte(n)
}

/// A recorded log re-homed onto a scripted chain: same emitter, topic0 and data, new block
/// identity and (optionally) different indexed addresses.
fn rehome(name: &str, block: u64, fork: u8, idx: u64, topics: &[(usize, Address)]) -> Value {
    let mut l = fx(name)["log"].clone();
    l["blockNumber"] = json!(format!("0x{block:x}"));
    l["blockHash"] = json!(format!("{:#x}", hash_of(fork, block)));
    l["logIndex"] = json!(format!("0x{idx:x}"));
    for (i, who) in topics {
        l["topics"][*i] = json!(format!("{:#x}", who.into_word()));
    }
    l
}

fn rpc_log(v: &Value) -> Log {
    serde_json::from_value(v.clone()).unwrap()
}

// ---------------------------------------------------------------- decode (real fixtures)

#[test]
fn every_recorded_event_decodes_to_the_expected_variant_and_fields() {
    let hashes: std::collections::HashMap<_, _> = events::signature_hashes().into_iter().collect();
    let fx = fixtures();
    assert_eq!(fx.len(), 12, "one recorded log per event");
    for f in &fx {
        let name = f["name"].as_str().unwrap();
        let log = rpc_log(&f["log"]);
        let prov = &f["provenance"];
        // Provenance really is the log's own position.
        assert_eq!(prov["chain_id"], 42161);
        assert_eq!(prov["block"], log.block_number.unwrap());
        assert_eq!(
            prov["tx"],
            json!(format!("{:#x}", log.transaction_hash.unwrap()))
        );
        // The ABI hash we compute equals the topic0 the deployed contract really emitted.
        assert_eq!(
            log.topics()[0],
            hashes[name],
            "{name}: signature differs from on-chain topic0"
        );
        let ev = events::decode(&log, POOL, ORACLE)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|| panic!("{name}: not recognised"));
        let t = |i: usize| Address::from_word(log.topics()[i]);
        let expected = match name {
            "Supply" => PoolEvent::Supply {
                reserve: t(1),
                on_behalf_of: t(2),
            },
            "Withdraw" => PoolEvent::Withdraw {
                reserve: t(1),
                user: t(2),
            },
            "Borrow" => PoolEvent::Borrow {
                reserve: t(1),
                on_behalf_of: t(2),
            },
            "Repay" => PoolEvent::Repay {
                reserve: t(1),
                user: t(2),
            },
            "ReserveUsedAsCollateralEnabled" => PoolEvent::CollateralEnabled {
                reserve: t(1),
                user: t(2),
            },
            "ReserveUsedAsCollateralDisabled" => PoolEvent::CollateralDisabled {
                reserve: t(1),
                user: t(2),
            },
            "UserEModeSet" => PoolEvent::EModeSet { user: t(1) },
            "ReserveDataUpdated" => PoolEvent::ReserveDataUpdated { reserve: t(1) },
            "DeficitCreated" => PoolEvent::DeficitCreated {
                user: t(1),
                debt_asset: t(2),
            },
            "Upgraded" => PoolEvent::Upgraded {
                implementation: t(1),
            },
            "AssetSourceUpdated" => PoolEvent::AssetSourceUpdated {
                asset: t(1),
                source: t(2),
            },
            "LiquidationCall" => {
                let PoolEvent::Liquidation {
                    collateral,
                    debt,
                    user,
                    liquidator,
                    receive_a_token,
                } = ev
                else {
                    panic!("LiquidationCall decoded as {ev:?}")
                };
                assert_eq!((collateral, debt, user), (t(1), t(2), t(3)));
                assert!(
                    !liquidator.is_zero(),
                    "liquidator comes from the data section"
                );
                // receiveAToken is the last data word.
                let last = log.data().data.as_ref()[log.data().data.len() - 1] == 1;
                assert_eq!(receive_a_token, last);
                continue;
            }
            other => panic!("unexpected fixture {other}"),
        };
        assert_eq!(ev, expected, "{name}");
    }
}

#[test]
fn decode_ignores_foreign_emitters_and_rejects_malformed_data() {
    let mut l = rpc_log(&fx("Borrow")["log"]);
    // Same bytes from another contract: not a Pool event.
    l.inner.address = a(0x42);
    assert_eq!(events::decode(&l, POOL, ORACLE).unwrap(), None);
    // A Pool log with truncated data is an error, not a silent skip.
    let mut l = rpc_log(&fx("Borrow")["log"]);
    l.inner.data =
        alloy::primitives::LogData::new_unchecked(l.topics().to_vec(), vec![1, 2, 3].into());
    assert!(events::decode(&l, POOL, ORACLE).is_err());
    // Oracle events only count when the oracle emitted them.
    let mut l = rpc_log(&fx("AssetSourceUpdated")["log"]);
    assert!(matches!(
        events::decode(&l, POOL, ORACLE).unwrap(),
        Some(PoolEvent::AssetSourceUpdated { .. })
    ));
    l.inner.address = POOL;
    assert_eq!(events::decode(&l, POOL, ORACLE).unwrap(), None);
}

#[test]
fn log_guard_mirrors_engine_continuity_rules() {
    let log = |block: u64, hash: u8, index: u64| -> Log {
        Log {
            block_number: Some(block),
            block_hash: Some(B256::repeat_byte(hash)),
            log_index: Some(index),
            ..Default::default()
        }
    };
    let mut g = LogGuard::new([(100, B256::repeat_byte(1))]);
    let mut removed = log(99, 1, 0);
    removed.removed = true;
    assert_eq!(g.accept(&removed), Err(Invalid::Removed));
    assert_eq!(
        g.accept(&log(100, 2, 0)),
        Err(Invalid::ConflictingBlock),
        "vs recorded ancestry"
    );
    assert_eq!(g.accept(&log(101, 1, 4)), Ok(true));
    assert_eq!(g.accept(&log(101, 1, 4)), Ok(false), "exact replay");
    assert_eq!(g.accept(&log(101, 1, 5)), Ok(true));
    assert_eq!(g.accept(&log(101, 1, 3)), Err(Invalid::Reordered));
    assert_eq!(
        g.accept(&log(101, 2, 6)),
        Err(Invalid::ConflictingBlock),
        "same height, new hash"
    );
    assert_eq!(g.accept(&Log::default()), Err(Invalid::MissingIdentity));
}

#[test]
fn event_effects_by_mode() {
    let at = |n| ChainPoint {
        block_number: n,
        block_hash: hash_of(1, n),
    };
    let run = |ev: PoolEvent, mode| {
        let mut co = Coalescer::new(100);
        let (mut deps, mut extra, mut found) = (vec![], vec![], BTreeSet::new());
        apply_event(&ev, at(5), mode, &mut co, &mut deps, &mut extra, &mut found);
        (co.into_effects(), deps, extra, found)
    };
    let borrow = PoolEvent::Borrow {
        reserve: a(1),
        on_behalf_of: a(2),
    };
    // Live: registers, marks, adds the debt dependency.
    let (touch, deps, _, _) = run(borrow.clone(), Mode::Live);
    assert!(matches!(
        &touch[..],
        [Effect::Touch {
            register: Some(_),
            stale_deps: false,
            ..
        }]
    ));
    assert_eq!(
        deps,
        vec![Effect::DepAdd {
            borrower: a(2),
            asset: a(1),
            kind: KIND_DEBT
        }]
    );
    // Replay: marks but edits no dependencies (flags them stale instead).
    let (touch, deps, _, _) = run(borrow.clone(), Mode::Replay);
    assert!(matches!(
        &touch[..],
        [Effect::Touch {
            stale_deps: true,
            ..
        }]
    ));
    assert!(deps.is_empty());
    // Backfill: discovery only.
    let (touch, deps, extra, found) = run(borrow, Mode::Backfill);
    assert!(touch.is_empty() && deps.is_empty() && extra.is_empty());
    assert_eq!(found, BTreeSet::from([a(2)]));
    let (touch, _, _, found) = run(
        PoolEvent::ReserveDataUpdated { reserve: a(1) },
        Mode::Backfill,
    );
    assert!(touch.is_empty() && found.is_empty());
    // Repay/withdraw cannot say whether the debt/collateral is gone: dependency set goes stale.
    let (touch, deps, _, _) = run(
        PoolEvent::Repay {
            reserve: a(1),
            user: a(2),
        },
        Mode::Live,
    );
    assert!(matches!(
        &touch[..],
        [Effect::Touch {
            stale_deps: true,
            register: None,
            ..
        }]
    ));
    assert!(deps.is_empty());
    // Liquidation touches the liquidator only when it receives aTokens.
    let liq = |recv| PoolEvent::Liquidation {
        collateral: a(1),
        debt: a(2),
        user: a(3),
        liquidator: a(4),
        receive_a_token: recv,
    };
    assert_eq!(run(liq(false), Mode::Live).0.len(), 1);
    assert_eq!(run(liq(true), Mode::Live).0.len(), 2);
    // Reserve data change => the reserve's holders; upgrade => flag + everyone.
    let (touch, ..) = run(PoolEvent::ReserveDataUpdated { reserve: a(1) }, Mode::Live);
    assert!(matches!(
        &touch[..],
        [Effect::AssetDirty {
            reason: DirtyReason::ReserveConfig,
            ..
        }]
    ));
    let (_, _, extra, _) = run(
        PoolEvent::Upgraded {
            implementation: a(9),
        },
        Mode::Live,
    );
    assert!(
        extra
            .iter()
            .any(|e| matches!(e, Effect::Flag { flag, .. } if flag == FLAG_FULL_REVERIFICATION))
    );
    assert!(extra.iter().any(|e| matches!(e, Effect::AllDirty { .. })));
}

#[test]
fn user_configuration_bitmap_becomes_collateral_and_debt_dependencies() {
    let ids = [(0u16, a(1)), (1, a(2)), (5, a(3)), (7, a(4))];
    // reserve 0: collateral only; reserve 1: borrowing only; reserve 5: both; reserve 7: none.
    let mask = (U256::from(1u8) << 1)
        | (U256::from(1u8) << 2)
        | (U256::from(1u8) << 10)
        | (U256::from(1u8) << 11);
    assert_eq!(
        deps_from_user_config(mask, &ids),
        vec![
            (a(1), KIND_COLLATERAL),
            (a(2), KIND_DEBT),
            (a(3), KIND_DEBT | KIND_COLLATERAL)
        ]
    );
    assert!(deps_from_user_config(U256::ZERO, &ids).is_empty());
}

#[test]
fn price_move_threshold_is_exact_integer_math() {
    let p = |v: u64| U256::from(v);
    assert!(!moved_enough(p(10_000), p(10_000), 2));
    assert!(!moved_enough(p(10_000), p(10_001), 2), "1 bp < 2 bp");
    assert!(
        moved_enough(p(10_000), p(10_002), 2),
        "exactly 2 bp triggers"
    );
    assert!(moved_enough(p(10_000), p(9_998), 2), "downward too");
    assert!(moved_enough(U256::ZERO, p(1), 2));
    assert!(
        moved_enough(U256::from(1u8), U256::MAX, 2),
        "overflow counts as moved"
    );
}

// ---------------------------------------------------------------- scripted chain

struct Rig {
    node: FakeNode,
    repo: Arc<SnapshotRepository>,
    idx: AaveTriggerIndexer,
    dir: tempfile::TempDir,
}

fn aave_cfg(manifest: Option<AaveManifest>) -> AaveConfig {
    AaveConfig {
        enabled: true,
        name: None,
        pool: POOL,
        data_provider: Address::ZERO,
        oracle: ORACLE,
        from_block: 1,
        scan_step: 50,
        manifest,
    }
}

fn tcfg(step: u64, max_requests: usize, attempts: u32) -> TriggerConfig {
    TriggerConfig {
        step,
        max_requests,
        throttle: Duration::ZERO,
        retry: RetryPolicy {
            attempts,
            max_delay: Duration::from_millis(10),
        },
        fanout_limit: 1_000,
        ..TriggerConfig::for_pool(&aave_cfg(None), CHAIN)
    }
}

fn indexer(
    node: &FakeNode,
    repo: &Arc<SnapshotRepository>,
    cfg: TriggerConfig,
    manifest: Option<AaveManifest>,
) -> AaveTriggerIndexer {
    let p = mev_rpc::http_provider(&node.url).unwrap();
    AaveTriggerIndexer::with_reserves(
        aave_cfg(manifest),
        cfg,
        p.clone(),
        p,
        repo.clone(),
        vec![(0, a(0xe1)), (1, a(0xe2)), (2, a(0xe3))],
    )
}

async fn rig(head: u64, logs: Vec<Value>, cfg: TriggerConfig) -> Rig {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, head),
        logs,
        ..Default::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::new(SnapshotRepository::open(&dir.path().join("liq.db")).unwrap());
    let idx = indexer(&node, &repo, cfg, None);
    Rig {
        node,
        repo,
        idx,
        dir,
    }
}

fn grow(node: &FakeNode, fork: u8, from: u64, to: u64, logs: Vec<Value>) {
    let mut s = node.state.lock().unwrap();
    for n in from..=to {
        s.blocks.insert(n, hash_of(fork, n));
    }
    s.logs.extend(logs);
}

fn reasons(repo: &SnapshotRepository, who: Address) -> Vec<DirtyReason> {
    repo.dirty_entries()
        .unwrap()
        .into_iter()
        .find(|e| e.key.borrower == who)
        .map(|e| {
            let mut r = e.reasons;
            r.sort();
            r
        })
        .unwrap_or_default()
}

const R1: u8 = 0xe1;
const R2: u8 = 0xe2;
const R3: u8 = 0xe3;

#[tokio::test]
async fn live_events_register_borrowers_mark_dirty_and_build_the_reverse_index() {
    let mut r = rig(100, vec![], tcfg(50, 40, 2)).await;
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.head, 100);
    let (ca, cb) = (a(0xa1), a(0xb2));
    grow(
        &r.node,
        1,
        101,
        120,
        vec![
            rehome("Borrow", 102, 1, 0, &[(1, a(R1)), (2, ca)]),
            rehome("Supply", 103, 1, 0, &[(1, a(R2)), (2, ca)]),
            rehome("Borrow", 104, 1, 0, &[(1, a(R1)), (2, cb)]),
            rehome(
                "ReserveUsedAsCollateralEnabled",
                105,
                1,
                0,
                &[(1, a(R3)), (2, cb)],
            ),
            rehome(
                "ReserveUsedAsCollateralDisabled",
                106,
                1,
                0,
                &[(1, a(R3)), (2, cb)],
            ),
            rehome("UserEModeSet", 107, 1, 0, &[(1, ca)]),
            rehome("Withdraw", 108, 1, 0, &[(1, a(R2)), (2, ca)]),
            rehome("Repay", 109, 1, 0, &[(1, a(R1)), (2, cb)]),
            rehome("ReserveDataUpdated", 110, 1, 0, &[(1, a(R2))]),
        ],
    );
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.events, 9);
    assert_eq!(rep.registered, 2);
    assert_eq!(rep.new_gaps, 0);

    let repo = &r.repo;
    assert_eq!(repo.active_borrowers(CHAIN, POOL).unwrap(), {
        let mut v = vec![ca, cb];
        v.sort();
        v
    });
    // Newly discovered borrowers carry the distinct Discovered reason beside the event reason.
    assert!(reasons(repo, ca).contains(&DirtyReason::Discovered));
    assert!(reasons(repo, ca).contains(&DirtyReason::BorrowerEvent));
    // Reverse index from events: debt on R1 for both; A supplied R2; B's R3 toggle netted out.
    let r1 = repo.asset_borrowers(CHAIN, POOL, a(R1)).unwrap();
    assert_eq!(r1.len(), 2);
    assert!(r1.iter().all(|(_, k)| k & KIND_DEBT != 0));
    assert_eq!(
        repo.asset_borrowers(CHAIN, POOL, a(R2)).unwrap(),
        vec![(ca, KIND_COLLATERAL)]
    );
    assert!(repo.asset_borrowers(CHAIN, POOL, a(R3)).unwrap().is_empty());
    // Withdraw/Repay flagged the dependency sets for reconciliation.
    assert!(repo.borrower(CHAIN, POOL, ca).unwrap().unwrap().deps_stale);
    assert!(repo.borrower(CHAIN, POOL, cb).unwrap().unwrap().deps_stale);
    // ReserveDataUpdated reached the unreconciled borrowers (unknown composition = affected).
    assert!(reasons(repo, ca).contains(&DirtyReason::ReserveConfig));
    // Cursor and ancestry advanced together.
    let c = repo.scan_cursor("aave-v3-triggers").unwrap().unwrap();
    assert_eq!(
        (c.next_block, c.tip.map(|t| t.block_number)),
        (121, Some(120))
    );
    // Quiet step: nothing to do, nothing re-marked, nothing rescanned.
    r.node.state.lock().unwrap().log_requests.clear();
    r.idx.step().await.unwrap();
    assert!(r.node.state.lock().unwrap().log_requests.is_empty());
}

#[tokio::test]
async fn reserve_data_update_marks_only_holders_once_dependencies_are_reconciled() {
    let mut r = rig(100, vec![], tcfg(50, 40, 2)).await;
    r.idx.step().await.unwrap();
    let (ca, cb) = (a(0xa1), a(0xb2));
    grow(
        &r.node,
        1,
        101,
        110,
        vec![
            rehome("Borrow", 102, 1, 0, &[(1, a(R1)), (2, ca)]),
            rehome("Borrow", 103, 1, 0, &[(1, a(R1)), (2, cb)]),
        ],
    );
    r.idx.step().await.unwrap();
    let at = ChainPoint {
        block_number: 110,
        block_hash: hash_of(1, 110),
    };
    r.repo
        .reconcile_borrower(
            CHAIN,
            POOL,
            ca,
            &[(a(R1), KIND_DEBT), (a(R2), KIND_COLLATERAL)],
            true,
            at,
        )
        .unwrap();
    r.repo
        .reconcile_borrower(CHAIN, POOL, cb, &[(a(R1), KIND_DEBT)], true, at)
        .unwrap();
    let before = reasons(&r.repo, cb);
    grow(
        &r.node,
        1,
        111,
        115,
        vec![rehome("ReserveDataUpdated", 112, 1, 0, &[(1, a(R2))])],
    );
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.marked, 1, "fan-out reaches exactly the holder of R2");
    assert!(reasons(&r.repo, ca).contains(&DirtyReason::ReserveConfig));
    assert_eq!(reasons(&r.repo, cb), before, "non-holder untouched");
}

#[tokio::test]
async fn reorg_drops_orphaned_discoveries_flags_touched_borrowers_and_invalidates_snapshots() {
    let mut r = rig(100, vec![], tcfg(50, 40, 2)).await;
    r.idx.step().await.unwrap();
    let (ca, cb, cc) = (a(0xa1), a(0xb2), a(0xc3));
    grow(
        &r.node,
        1,
        101,
        110,
        vec![
            rehome("Borrow", 102, 1, 0, &[(1, a(R1)), (2, ca)]), // survives
            rehome("Borrow", 108, 1, 0, &[(1, a(R1)), (2, cb)]), // orphaned discovery
            rehome("Supply", 109, 1, 0, &[(1, a(R2)), (2, ca)]), // orphaned touch of A
        ],
    );
    r.idx.step().await.unwrap();
    assert_eq!(r.repo.active_borrowers(CHAIN, POOL).unwrap().len(), 2);
    // A snapshot of A read on the soon-orphaned fork.
    let snap = BorrowerSnapshot {
        key: AccountKey {
            chain_id: CHAIN,
            pool: POOL,
            borrower: ca,
        },
        revision: 0,
        chain: ChainRef {
            block_number: 110,
            block_hash: hash_of(1, 110),
            source: ChainSource::Provisional,
            observed_at_ns: u64::MAX / 2,
        },
        health_factor_wad: WAD + U256::from(3u8),
        total_debt_base: U256::from(1u8),
        total_collateral_base: U256::from(2u8),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::ZERO,
        ltv: U256::ZERO,
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(1),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal();
    r.repo.upsert_cas(&snap, 0).unwrap();

    // Reorg from 106: the fork has a different Borrow (C) at 111 and no B/Supply.
    r.node.reorg(
        106,
        115,
        2,
        vec![rehome("Borrow", 111, 2, 0, &[(1, a(R1)), (2, cc)])],
    );
    r.node.state.lock().unwrap().log_requests.clear();
    let rep = r.idx.step().await.unwrap();
    let rewound = rep.rewound_to.expect("reorg detected");
    assert!(
        rewound <= 105,
        "rewound to a common ancestor, got {rewound}"
    );
    assert_eq!(
        rep.rewind.removed_borrowers, 1,
        "B existed only on the dropped fork"
    );
    assert_eq!(rep.rewind.invalidated_snapshots, 1);
    assert_eq!(rep.rewind.touched_marked, 1);
    let active = r.repo.active_borrowers(CHAIN, POOL).unwrap();
    assert!(active.contains(&ca) && active.contains(&cc) && !active.contains(&cb));
    assert!(reasons(&r.repo, ca).contains(&DirtyReason::Reorg));
    assert_eq!(
        r.repo.fetch_by_key(&snap.key).unwrap().unwrap().status,
        SnapshotStatus::Invalidated
    );
    // A's Supply dependency came from the dropped block: stale until reconciled.
    assert!(
        r.repo
            .borrower(CHAIN, POOL, ca)
            .unwrap()
            .unwrap()
            .deps_stale
    );
    let reqs = r.node.state.lock().unwrap().log_requests.clone();
    assert!(
        reqs.iter().any(|(f, t)| *f <= 106 && *t >= 115),
        "rescanned the new fork: {reqs:?}"
    );
    // The canonical tip is recorded again: a further step is quiet.
    r.node.state.lock().unwrap().log_requests.clear();
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.rewound_to, None);
    assert!(r.node.state.lock().unwrap().log_requests.is_empty());
}

#[tokio::test]
async fn removed_logs_are_never_applied_and_the_range_becomes_a_healable_gap() {
    let mut r = rig(100, vec![], tcfg(50, 40, 2)).await;
    r.idx.step().await.unwrap();
    let cd = a(0xd4);
    let mut bad = rehome("Borrow", 103, 1, 0, &[(1, a(R1)), (2, cd)]);
    bad["removed"] = json!(true);
    grow(&r.node, 1, 101, 110, vec![bad]);
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.new_gaps, 1);
    assert!(
        r.repo.borrower(CHAIN, POOL, cd).unwrap().is_none(),
        "removed log must not register"
    );
    let gaps = r.repo.open_gaps("aave-v3-triggers").unwrap();
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].reason.contains("removed log"), "{}", gaps[0].reason);
    assert_eq!((gaps[0].from_block, gaps[0].to_block), (101, 110));
    assert_eq!(
        r.repo
            .scan_cursor("aave-v3-triggers")
            .unwrap()
            .unwrap()
            .next_block,
        111
    );

    // The node now serves the log as a normal one: the gap heals and the borrower appears
    // (replayed gaps flag dependencies instead of editing them).
    r.node.state.lock().unwrap().logs[0]["removed"] = json!(false);
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.healed_gaps, 1);
    assert!(r.repo.open_gaps("aave-v3-triggers").unwrap().is_empty());
    let rec = r.repo.borrower(CHAIN, POOL, cd).unwrap().unwrap();
    assert!(rec.active && rec.deps_stale);
}

#[tokio::test]
async fn rate_limited_scan_records_a_gap_instead_of_aborting_and_heals_later() {
    let logs = vec![rehome("Borrow", 30, 1, 0, &[(1, a(R1)), (2, a(0xa1))])];
    let mut r = rig(100, logs, tcfg(50, 40, 1)).await;
    r.node.state.lock().unwrap().rate_limit_logs = 1000;
    let rep = r
        .idx
        .step()
        .await
        .expect("rate limits do not abort the step");
    assert!(rep.new_gaps >= 1);
    let gaps = r.repo.open_gaps("aave-v3-triggers").unwrap();
    assert!(!gaps.is_empty());
    assert!(
        gaps.iter()
            .all(|g| g.reason.contains("429") || g.reason.to_lowercase().contains("rate")),
        "{gaps:?}"
    );
    assert!(
        r.node.state.lock().unwrap().rate_limited >= 2,
        "retried with backoff first"
    );
    r.node.state.lock().unwrap().rate_limit_logs = 0;
    let rep = r.idx.step().await.unwrap();
    assert!(rep.healed_gaps >= 1);
    assert!(r.repo.open_gaps("aave-v3-triggers").unwrap().is_empty());
    assert!(
        r.repo.borrower(CHAIN, POOL, a(0xa1)).unwrap().is_some(),
        "borrower inside the gap found"
    );
}

#[tokio::test]
async fn single_429_is_absorbed_by_retry() {
    let logs = vec![rehome("Borrow", 90, 1, 0, &[(1, a(R1)), (2, a(0xa1))])];
    let mut r = rig(100, logs, tcfg(50, 40, 3)).await;
    r.node.state.lock().unwrap().rate_limit_logs = 1;
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.new_gaps, 0);
    assert_eq!(r.node.state.lock().unwrap().rate_limited, 1);
    assert!(r.repo.borrower(CHAIN, POOL, a(0xa1)).unwrap().is_some());
}

#[tokio::test]
async fn backfill_resumes_after_a_crash_between_ranges_with_no_gap_or_duplicate() {
    let logs: Vec<Value> = (0..12u64)
        .map(|i| {
            rehome(
                "Borrow",
                5 + i * 8,
                1,
                0,
                &[(1, a(R1)), (2, a(0x10 + i as u8))],
            )
        })
        .collect();
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs,
        ..Default::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("liq.db");

    // Process A: 3 ranges of 10 blocks, then it dies without any shutdown path.
    {
        let repo = Arc::new(SnapshotRepository::open(&db).unwrap());
        let mut idx = indexer(&node, &repo, tcfg(10, 3, 2), None);
        idx.step().await.unwrap();
        assert!(repo.active_borrowers(CHAIN, POOL).unwrap().len() < 12);
    } // dropped: connection closed uncleanly from the index's point of view

    // Process B reopens the same files and finishes.
    let mut rounds = 0;
    loop {
        let repo = Arc::new(SnapshotRepository::open(&db).unwrap());
        let mut idx = indexer(&node, &repo, tcfg(10, 3, 2), None);
        idx.step().await.unwrap();
        let c = repo.scan_cursor("aave-v3-triggers").unwrap().unwrap();
        if c.back_block <= 1 {
            assert!(repo.open_gaps("aave-v3-triggers").unwrap().is_empty());
            let all = repo.active_borrowers(CHAIN, POOL).unwrap();
            assert_eq!(all.len(), 12, "every borrower exactly once");
            for b in &all {
                assert_eq!(
                    reasons(&repo, *b),
                    vec![DirtyReason::Discovered],
                    "history discovery only"
                );
            }
            break;
        }
        rounds += 1;
        assert!(rounds < 20, "resume must converge");
    }
    let mut reqs = node.state.lock().unwrap().log_requests.clone();
    reqs.sort_unstable();
    let mut next = 1;
    for (f, t) in reqs {
        assert_eq!(f, next, "ranges tile the history without overlap or hole");
        next = t + 1;
    }
    assert_eq!(next, 101);
}

#[tokio::test]
async fn upgrade_event_and_implementation_slot_both_raise_the_reverification_flag() {
    // (1) The proxy's Upgraded event.
    let mut r = rig(100, vec![], tcfg(50, 40, 2)).await;
    r.idx.step().await.unwrap();
    grow(
        &r.node,
        1,
        101,
        110,
        vec![
            rehome("Borrow", 102, 1, 0, &[(1, a(R1)), (2, a(0xa1))]),
            rehome("Upgraded", 105, 1, 0, &[(1, a(0x99))]),
        ],
    );
    let rep = r.idx.step().await.unwrap();
    assert!(
        r.repo
            .has_flag(CHAIN, POOL, FLAG_FULL_REVERIFICATION)
            .unwrap()
    );
    assert!(
        r.repo.flags(CHAIN, POOL).unwrap()[0]
            .detail
            .contains("Upgraded")
    );
    assert!(rep.marked >= 1);
    assert!(reasons(&r.repo, a(0xa1)).contains(&DirtyReason::ReserveConfig));

    // (2) No event at all: the task 002 verifier sees a different implementation slot value.
    let impl_ok = a(0x11);
    let code = "0x6001600155";
    let manifest = AaveManifest {
        chain_id: CHAIN,
        pool_addresses_provider: a(0x77),
        pool: POOL,
        oracle: ORACLE,
        pool_implementation: impl_ok,
        pool_implementation_code_hash: keccak256(alloy::hex::decode(code).unwrap()),
        verified_block: 1,
        source: "test".into(),
    };
    let slot = U256::from_be_bytes(keccak256("eip1967.proxy.implementation").0) - U256::from(1u8);
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        ..Default::default()
    })
    .await;
    {
        let mut s = node.state.lock().unwrap();
        s.calls.insert(
            (a(0x77), getPoolCall::SELECTOR),
            format!("{:#x}", POOL.into_word()),
        );
        s.calls.insert(
            (a(0x77), getPriceOracleCall::SELECTOR),
            format!("{:#x}", ORACLE.into_word()),
        );
        s.storage.insert((POOL, slot), impl_ok.into_word());
        s.code.insert(impl_ok, code.to_string());
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::new(SnapshotRepository::open(&dir.path().join("l.db")).unwrap());
    let mut idx = indexer(&node, &repo, tcfg(50, 40, 2), Some(manifest.clone()));
    repo.register_borrowers(CHAIN, POOL, &[a(0xa1)], None, 1)
        .unwrap();
    let rep = idx.step().await.unwrap();
    assert!(
        !rep.upgrade_flagged,
        "matching deployment: no flag ({:?})",
        rep.warnings
    );
    assert!(
        !repo
            .has_flag(CHAIN, POOL, FLAG_FULL_REVERIFICATION)
            .unwrap()
    );
    // The implementation slot now points elsewhere.
    node.state
        .lock()
        .unwrap()
        .storage
        .insert((POOL, slot), a(0x22).into_word());
    let rep = idx.step().await.unwrap();
    assert!(rep.upgrade_flagged);
    let f = repo.flags(CHAIN, POOL).unwrap();
    assert!(f[0].detail.contains("manifest"), "{}", f[0].detail);
    assert!(reasons(&repo, a(0xa1)).contains(&DirtyReason::ReserveConfig));
    // Once flagged it is not re-raised every step (the operator clears it after re-pinning).
    let rep = idx.step().await.unwrap();
    assert!(!rep.upgrade_flagged);
    assert!(
        repo.clear_flag(CHAIN, POOL, FLAG_FULL_REVERIFICATION)
            .unwrap()
    );
    let _ = r.dir;
}

#[tokio::test]
async fn oracle_source_events_and_feed_matches_mark_asset_holders_with_bounded_fanout() {
    let cfg = TriggerConfig {
        fanout_limit: 2,
        ..tcfg(50, 40, 2)
    };
    let mut r = rig(100, vec![], cfg).await;
    r.idx.step().await.unwrap();
    // Six unreconciled borrowers: all count as holders of every asset.
    let who: Vec<Address> = (1..=6u8).map(|n| a(0x30 + n)).collect();
    r.repo
        .register_borrowers(CHAIN, POOL, &who, None, 1)
        .unwrap();
    // Feed-match path (what the adapter's FeedWatcher result feeds): bounded now, rest queued.
    let marked = r.idx.mark_assets_price_moved(&[a(R1)]).unwrap();
    assert_eq!(marked, 2);
    assert_eq!(r.repo.fanout_pending(CHAIN, POOL).unwrap(), 4);
    let rep = r.idx.step().await.unwrap();
    assert_eq!(rep.drained, 4, "queue drained on the next step");
    for w in &who {
        assert!(reasons(&r.repo, *w).contains(&DirtyReason::PriceMove));
    }
    // An oracle AssetSourceUpdated log is its own trigger, independent of price logs.
    let before = r.repo.dirty_entries().unwrap().len();
    grow(
        &r.node,
        1,
        101,
        110,
        vec![rehome("AssetSourceUpdated", 104, 1, 0, &[(1, a(R2))])],
    );
    let rep = r.idx.step().await.unwrap();
    assert!(rep.events >= 1 && rep.marked + rep.queued >= 6);
    assert!(r.repo.dirty_entries().unwrap().len() >= before);
}

/// The indexer can live in its own task: `run` keeps stepping (new chain blocks are scanned and
/// their borrowers registered) while a detached `PriceMarker` marks holders through the shared
/// repository, with the same bounded fan-out as the indexer's own method.
#[tokio::test]
async fn indexer_runs_in_its_own_task_while_the_marker_keeps_marking() {
    let cfg = TriggerConfig {
        fanout_limit: 2,
        ..tcfg(50, 40, 2)
    };
    let r = rig(100, vec![], cfg).await;
    let marker = r.idx.marker();
    let (repo, node) = (r.repo.clone(), r.node);
    let task = tokio::spawn(r.idx.run(Duration::from_millis(20)));

    let who: Vec<Address> = (1..=5u8).map(|n| a(0x40 + n)).collect();
    repo.register_borrowers(CHAIN, POOL, &who, None, 1).unwrap();
    assert_eq!(
        marker.mark_assets_price_moved(&[a(R1)]).unwrap(),
        2,
        "bounded"
    );

    let fresh = a(0x99);
    grow(
        &node,
        1,
        101,
        110,
        vec![rehome("Borrow", 105, 1, 0, &[(1, a(R1)), (2, fresh)])],
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seen = repo.borrower(CHAIN, POOL, fresh).unwrap().is_some();
        let drained = repo.fanout_pending(CHAIN, POOL).unwrap() == 0;
        if seen && drained {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the task never stepped (seen={seen}, drained={drained})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for w in &who {
        assert!(reasons(&repo, *w).contains(&DirtyReason::PriceMove));
    }
    assert!(!task.is_finished(), "run never returns");
    task.abort();
    drop(r.dir);
}
