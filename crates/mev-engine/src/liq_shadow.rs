//! Shadow-only liquidation pipeline wiring (JEV-LIQ-004/005 into the engine).
//!
//! Everything here is additive and off unless `[liquidation_shadow] enabled`: it fills the
//! durable snapshot repository (trigger indexer through the Aave adapter, a bounded background
//! snapshot refresh) and lets the runner record the mandatory schedule. Nothing in this module
//! plans, simulates or sends, and a failure at startup only disables the pipeline.

use std::{
    collections::HashSet,
    future::Future,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy::primitives::Address;
use anyhow::{Context, Result};
use futures::StreamExt;
use mev_config::LiquidationShadowConfig;
use mev_liq_sched::{PolicyConfig, ShadowParams};
use mev_liq_state::{AccountKey, BorrowerSnapshot, SnapshotRepository};
use mev_liquidate::{ShadowSchedule, ShadowSink};
use tracing::{error, info, warn};

pub const EVENT_REFRESH: &str = "liq_shadow_refresh";
const NS: u64 = 1_000_000_000;

/// What the engine keeps from startup to build the schedule and the refresh task later.
#[derive(Clone)]
pub struct ShadowParts {
    pub repo: Arc<SnapshotRepository>,
    pub chain_id: u64,
    pub pool: Address,
    pub scan_name: String,
    pub cfg: LiquidationShadowConfig,
}

/// Open (creating, migrating) the snapshot database. Any failure is returned, never fatal.
pub fn open_repo(
    cfg: &LiquidationShadowConfig,
    data_dir: &Path,
) -> Result<Arc<SnapshotRepository>> {
    let path = cfg
        .snapshot_db
        .clone()
        .unwrap_or_else(|| data_dir.join("liq-state.sqlite"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let repo = SnapshotRepository::open(&path)
        .with_context(|| format!("open snapshot db {}", path.display()))?;
    Ok(Arc::new(repo))
}

/// Log the ALERT used for every startup failure of the shadow pipeline.
pub fn alert_disabled(stage: &str, e: &anyhow::Error) {
    error!(
        stage,
        error = %format!("{e:#}"),
        "ALERT: liquidation shadow pipeline disabled (bot continues without it)"
    );
}

/// The runner's shadow schedule (records only) with a metrics sink.
pub fn schedule(parts: &ShadowParts, sink: ShadowSink) -> ShadowSchedule {
    let policy = PolicyConfig {
        max_account_age_ns: parts.cfg.max_account_age_seconds.saturating_mul(NS),
        concurrency: u32::try_from(parts.cfg.recheck_concurrency.max(1)).unwrap_or(u32::MAX),
        ..PolicyConfig::default()
    };
    let params = ShadowParams {
        chain_id: parts.chain_id,
        pool: parts.pool,
        scan_name: parts.scan_name.clone(),
        policy,
        max_records: 50,
    };
    ShadowSchedule::new(parts.repo.clone(), params)
        .enabled(true)
        .every(Duration::from_millis(parts.cfg.tick_ms.max(100)))
        .with_sink(sink)
}

/// Rechecks allowed per rolling minute window.
#[derive(Debug)]
pub struct Budget {
    per_minute: u32,
    window_start: Instant,
    used: u32,
}

impl Budget {
    pub fn new(per_minute: u32, now: Instant) -> Self {
        Self {
            per_minute,
            window_start: now,
            used: 0,
        }
    }

    /// Grant up to `want` rechecks now.
    pub fn take(&mut self, want: usize, now: Instant) -> usize {
        if now.duration_since(self.window_start) >= Duration::from_secs(60) {
            self.window_start = now;
            self.used = 0;
        }
        let left = self.per_minute.saturating_sub(self.used) as usize;
        let n = want.min(left);
        self.used += u32::try_from(n).unwrap_or(u32::MAX);
        n
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub candidates: usize,
    pub attempted: usize,
    pub stored: usize,
    pub conflicts: usize,
    pub rpc_errors: usize,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Accounts to refresh: dirty first, then expired, for this pool only, deduplicated.
fn candidates(parts: &ShadowParts, now_ns: u64) -> Result<Vec<AccountKey>> {
    let max_age = parts.cfg.max_account_age_seconds.saturating_mul(NS);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let dirty = parts
        .repo
        .list_dirty()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let expired = parts
        .repo
        .list_expired(now_ns, max_age)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for k in dirty.into_iter().chain(expired) {
        if k.chain_id == parts.chain_id && k.pool == parts.pool && seen.insert(k.borrower) {
            out.push(k);
        }
    }
    Ok(out)
}

/// One refresh round: at most `min(budget, candidates)` rechecks, at most
/// `recheck_concurrency` in flight. `recheck` reads the account at the head.
pub async fn refresh_once<F, Fut>(
    parts: &ShadowParts,
    budget: &mut Budget,
    recheck: F,
) -> Result<RefreshReport>
where
    F: Fn(Address) -> Fut,
    Fut: Future<Output = Result<BorrowerSnapshot>>,
{
    let p = parts.clone();
    let mut keys = tokio::task::spawn_blocking(move || candidates(&p, now_ns())).await??;
    let mut rep = RefreshReport {
        candidates: keys.len(),
        ..Default::default()
    };
    let n = budget.take(keys.len(), Instant::now());
    keys.truncate(n);
    rep.attempted = n;
    let conc = parts.cfg.recheck_concurrency.max(1);
    let results: Vec<Result<(), (bool, String)>> = futures::stream::iter(keys)
        .map(|k| {
            let fut = recheck(k.borrower);
            let repo = parts.repo.clone();
            async move {
                let snap = match fut.await {
                    Ok(s) => s,
                    Err(e) => return Err((true, format!("{e:#}"))),
                };
                tokio::task::spawn_blocking(move || {
                    let expected = repo
                        .fetch_by_key(&k)
                        .map_err(|e| (false, e.to_string()))?
                        .map_or(0, |s| s.revision);
                    repo.upsert_cas(&snap, expected)
                        .map(|_| ())
                        .map_err(|e| (false, e.to_string()))
                })
                .await
                .unwrap_or_else(|e| Err((false, e.to_string())))
            }
        })
        .buffer_unordered(conc)
        .collect()
        .await;
    for r in results {
        match r {
            Ok(()) => rep.stored += 1,
            Err((true, _)) => rep.rpc_errors += 1,
            Err((false, _)) => rep.conflicts += 1,
        }
    }
    Ok(rep)
}

/// Spawn the background refresh loop. Never awaited by the caller.
pub fn spawn_refresh(
    parts: ShadowParts,
    rechecker: mev_lending_aave_v3::recheck::AaveAccountRechecker,
    sink: ShadowSink,
) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(parts.cfg.tick_ms.max(100)));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut budget = Budget::new(parts.cfg.recheck_budget_per_minute, Instant::now());
        loop {
            iv.tick().await;
            let rc = rechecker.clone();
            match refresh_once(&parts, &mut budget, move |b| {
                let rc = rc.clone();
                async move { rc.recheck_at_head(b).await }
            })
            .await
            {
                Ok(r) if r.attempted > 0 => {
                    info!(
                        candidates = r.candidates,
                        attempted = r.attempted,
                        stored = r.stored,
                        conflicts = r.conflicts,
                        rpc_errors = r.rpc_errors,
                        "liq_shadow_refresh"
                    );
                    sink(
                        EVENT_REFRESH,
                        serde_json::json!({
                            "candidates": r.candidates, "attempted": r.attempted,
                            "stored": r.stored, "conflicts": r.conflicts,
                            "rpc_errors": r.rpc_errors,
                        }),
                    );
                }
                Ok(_) => {}
                Err(e) => warn!(error = %format!("{e:#}"), "liq_shadow_refresh failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use alloy::primitives::U256;
    use mev_liq_state::{ChainRef, ChainSource, SnapshotStatus};

    use super::*;

    const CHAIN: u64 = 42161;

    fn parts(dir: &Path, conc: usize, budget: u32) -> ShadowParts {
        let cfg = LiquidationShadowConfig {
            enabled: true,
            recheck_concurrency: conc,
            recheck_budget_per_minute: budget,
            ..Default::default()
        };
        ShadowParts {
            repo: open_repo(&cfg, dir).unwrap(),
            chain_id: CHAIN,
            pool: Address::repeat_byte(0xaa),
            scan_name: "aave-v3-triggers".into(),
            cfg,
        }
    }

    fn snap(p: &ShadowParts, b: Address) -> BorrowerSnapshot {
        BorrowerSnapshot {
            key: AccountKey {
                chain_id: CHAIN,
                pool: p.pool,
                borrower: b,
            },
            revision: 0,
            chain: ChainRef {
                block_number: 10,
                block_hash: Default::default(),
                source: ChainSource::Provisional,
                observed_at_ns: now_ns(),
            },
            health_factor_wad: U256::from(2u64) * U256::from(10u64).pow(U256::from(18u64)),
            total_debt_base: U256::from(1u64),
            total_collateral_base: U256::from(2u64),
            available_borrows_base: U256::ZERO,
            liquidation_threshold: U256::ZERO,
            ltv: U256::ZERO,
            reserves: vec![],
            risk_metadata_version: 0,
            dirty_reasons: vec![],
            last_successful_check_ns: Some(now_ns()),
            status: SnapshotStatus::Valid,
            provenance: "test".into(),
            feature_digest: Default::default(),
        }
        .seal()
    }

    #[test]
    fn startup_wiring_builds_against_a_temp_data_dir() {
        let d = tempfile::tempdir().unwrap();
        let p = parts(d.path(), 2, 10);
        assert!(d.path().join("liq-state.sqlite").exists());
        let s = schedule(&p, Arc::new(|_, _| {}));
        assert!(s.enabled);
        assert_eq!(s.params.policy.max_account_age_ns, 300 * NS);
        assert_eq!(s.params.policy.concurrency, 2u32);
    }

    #[test]
    fn unopenable_db_is_an_error_not_a_panic() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("file");
        std::fs::write(&f, b"x").unwrap();
        let cfg = LiquidationShadowConfig {
            snapshot_db: Some(f.join("sub/liq.sqlite")),
            ..Default::default()
        };
        assert!(open_repo(&cfg, d.path()).is_err());
    }

    #[test]
    fn budget_is_a_rolling_per_minute_cap() {
        let t0 = Instant::now();
        let mut b = Budget::new(5, t0);
        assert_eq!(b.take(3, t0), 3);
        assert_eq!(b.take(10, t0), 2);
        assert_eq!(b.take(10, t0), 0);
        assert_eq!(b.take(10, t0 + Duration::from_secs(61)), 5);
    }

    #[tokio::test]
    async fn refresh_respects_concurrency_and_budget_and_fills_the_repo() {
        let d = tempfile::tempdir().unwrap();
        let p = parts(d.path(), 2, 5);
        let users: Vec<Address> = (1..=8u8).map(Address::with_last_byte).collect();
        p.repo
            .register_borrowers(CHAIN, p.pool, &users, None, 1)
            .unwrap();
        let (cur, peak, calls) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let mut budget = Budget::new(5, Instant::now());
        let (c2, pk2, ca2, p2) = (cur.clone(), peak.clone(), calls.clone(), p.clone());
        let rep = refresh_once(&p, &mut budget, move |b| {
            let (c, pk, ca, p) = (c2.clone(), pk2.clone(), ca2.clone(), p2.clone());
            async move {
                ca.fetch_add(1, Ordering::SeqCst);
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(snap(&p, b))
            }
        })
        .await
        .unwrap();
        assert_eq!(rep.candidates, 8);
        assert_eq!(rep.attempted, 5, "budget caps the round");
        assert_eq!(rep.stored, 5);
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        assert!(peak.load(Ordering::SeqCst) <= 2, "concurrency cap");
        // the next round has no budget left
        let rep2 = refresh_once(&p, &mut budget, |b| {
            let s = snap(&p, b);
            async move { Ok(s) }
        })
        .await
        .unwrap();
        assert_eq!(rep2.attempted, 0);
    }
}
