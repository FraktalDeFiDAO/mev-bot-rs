//! Shadow-only liquidation pipeline wiring (JEV-LIQ-004/005 into the engine).
//!
//! Everything here is additive and off unless `[liquidation_shadow] enabled`: it fills the
//! durable snapshot repository (trigger indexer through the Aave adapter, a bounded background
//! snapshot refresh), records the mandatory schedule through the runner and publishes a periodic
//! census. Nothing in this module plans, simulates or sends, and a failure at startup only
//! disables the pipeline.
//!
//! Scale: every per-tick read is bounded (see `mev_liq_state` tiering). Dust accounts below
//! `min_debt_usd` are refreshed on `cold_max_age_seconds`, hot accounts on
//! `max_account_age_seconds`, and rechecks are batched reads at one pinned block.

use std::{
    future::Future,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy::primitives::{Address, U256};
use anyhow::{Context, Result};
use futures::StreamExt;
use mev_config::LiquidationShadowConfig;
use mev_liq_sched::{LoadBounds, PolicyConfig, ShadowParams};
use mev_liq_state::{BorrowerSnapshot, DebtTiers, PoolCensus, SnapshotRepository, StoreOutcome};
use mev_liquidate::{ShadowSchedule, ShadowSink};
use tracing::{error, info, warn};

pub const EVENT_REFRESH: &str = "liq_shadow_refresh";
pub const EVENT_CENSUS: &str = "liq_shadow_census";
const NS: u64 = 1_000_000_000;
/// Aave's base currency is USD with 8 decimals.
const USD_BASE: u64 = 100_000_000;
/// Lowest-health accounts listed in each census.
const CENSUS_LOWEST: usize = 10;

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

/// The hot/cold line and the age of each tier.
pub fn tiers(cfg: &LiquidationShadowConfig) -> DebtTiers {
    DebtTiers {
        min_debt_base: U256::from(cfg.min_debt_usd) * U256::from(USD_BASE),
        hot_max_age_ns: cfg.max_account_age_seconds.saturating_mul(NS),
        cold_max_age_ns: cfg.cold_max_age_seconds.saturating_mul(NS),
    }
}

/// Accounts in flight at most: batches in flight times accounts per batch.
fn slots(cfg: &LiquidationShadowConfig) -> u32 {
    u32::try_from(cfg.recheck_concurrency.max(1) * cfg.recheck_batch.max(1)).unwrap_or(u32::MAX)
}

/// The runner's shadow schedule (records only) with a metrics sink. Its feasibility model is the
/// refresh task's real capacity: `slots` accounts in flight and a per-slot cost chosen so that
/// one minute holds exactly `recheck_budget_per_minute` rechecks.
pub fn schedule(parts: &ShadowParts, sink: ShadowSink) -> ShadowSchedule {
    let cfg = &parts.cfg;
    let slots = slots(cfg);
    let policy = PolicyConfig {
        max_account_age_ns: cfg.max_account_age_seconds.saturating_mul(NS),
        concurrency: slots,
        recheck_cost_ns: (60 * NS).saturating_mul(u64::from(slots))
            / u64::from(cfg.recheck_budget_per_minute.max(1)),
        ..PolicyConfig::default()
    };
    let params = ShadowParams {
        chain_id: parts.chain_id,
        pool: parts.pool,
        scan_name: parts.scan_name.clone(),
        policy,
        max_records: 50,
        bounds: Some(LoadBounds {
            min_debt_base: tiers(cfg).min_debt_base,
            max_unknown: cfg.max_unknown_per_pass,
        }),
    };
    ShadowSchedule::new(parts.repo.clone(), params)
        .enabled(true)
        .every(Duration::from_millis(cfg.tick_ms.max(100)))
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

    fn roll(&mut self, now: Instant) {
        if now.duration_since(self.window_start) >= Duration::from_secs(60) {
            self.window_start = now;
            self.used = 0;
        }
    }

    /// Rechecks still allowed in the current window.
    pub fn available(&mut self, now: Instant) -> usize {
        self.roll(now);
        self.per_minute.saturating_sub(self.used) as usize
    }

    /// Grant up to `want` rechecks now.
    pub fn take(&mut self, want: usize, now: Instant) -> usize {
        let n = want.min(self.available(now));
        self.used += u32::try_from(n).unwrap_or(u32::MAX);
        n
    }
}

/// The minute budget spread over the ticks (rounded up), so one tick never takes it all.
fn per_tick(cfg: &LiquidationShadowConfig) -> usize {
    let ticks_per_minute = (60_000 / cfg.tick_ms.max(100)).max(1);
    (u64::from(cfg.recheck_budget_per_minute).div_ceil(ticks_per_minute)) as usize
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    /// Accounts selected this round (at most the tick's share of the budget).
    pub candidates: usize,
    pub attempted: usize,
    pub stored: usize,
    /// Not stored because the repository already holds a newer block.
    pub conflicts: usize,
    /// Accounts not read: their batch failed or their own call failed.
    pub rpc_errors: usize,
    /// Accounts read but not stored because the database write failed.
    pub store_errors: usize,
}

/// Outcome of one batch (its size first).
enum Batch {
    ReadFailed(usize, String),
    Stored(usize, StoreOutcome),
    StoreFailed {
        len: usize,
        read: usize,
        error: String,
    },
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// One refresh round: the most urgent candidates within the tick's share of the budget, read in
/// batches of `recheck_batch` with at most `recheck_concurrency` batches in flight, each batch
/// stored in one transaction. `recheck` reads a batch at one block (`None` = that account's
/// call failed).
pub async fn refresh_once<F, Fut>(
    parts: &ShadowParts,
    budget: &mut Budget,
    recheck: F,
) -> Result<RefreshReport>
where
    F: Fn(Vec<Address>) -> Fut,
    Fut: Future<Output = Result<Vec<Option<BorrowerSnapshot>>>>,
{
    let want = budget.available(Instant::now()).min(per_tick(&parts.cfg));
    let p = parts.clone();
    let keys = tokio::task::spawn_blocking(move || {
        p.repo
            .refresh_candidates(p.chain_id, p.pool, &tiers(&p.cfg), now_ns(), want)
    })
    .await??;
    let n = budget.take(keys.len(), Instant::now());
    let mut rep = RefreshReport {
        candidates: keys.len(),
        attempted: n,
        ..Default::default()
    };
    let batches: Vec<Vec<Address>> = keys[..n]
        .chunks(parts.cfg.recheck_batch.max(1))
        .map(<[Address]>::to_vec)
        .collect();
    let results: Vec<Batch> = futures::stream::iter(batches)
        .map(|batch| {
            let len = batch.len();
            let fut = recheck(batch);
            let repo = parts.repo.clone();
            async move {
                let snaps = match fut.await {
                    Ok(s) => s,
                    Err(e) => return Batch::ReadFailed(len, format!("{e:#}")),
                };
                let read: Vec<BorrowerSnapshot> = snaps.into_iter().flatten().collect();
                let n_read = read.len();
                let stored = tokio::task::spawn_blocking(move || repo.store_rechecks(&read))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                match stored {
                    Ok(o) => Batch::Stored(len, o),
                    Err(e) => Batch::StoreFailed {
                        len,
                        read: n_read,
                        error: e,
                    },
                }
            }
        })
        .buffer_unordered(parts.cfg.recheck_concurrency.max(1))
        .collect()
        .await;
    let mut first_err: Option<String> = None;
    for r in results {
        match r {
            Batch::ReadFailed(len, e) => {
                rep.rpc_errors += len;
                first_err.get_or_insert(e);
            }
            Batch::Stored(len, o) => {
                rep.stored += o.stored;
                rep.conflicts += o.superseded;
                rep.rpc_errors += len - o.stored - o.superseded;
            }
            Batch::StoreFailed { len, read, error } => {
                rep.store_errors += read;
                rep.rpc_errors += len - read;
                first_err.get_or_insert(error);
            }
        }
    }
    if let Some(e) = first_err {
        warn!(error = %e, "liq_shadow_refresh: batch failed");
    }
    Ok(rep)
}

/// Census plus the lowest-health hot accounts, as logged and emitted every `census_seconds`.
pub fn census(parts: &ShadowParts) -> Result<(PoolCensus, serde_json::Value)> {
    let t = tiers(&parts.cfg);
    let now = now_ns();
    let c = parts
        .repo
        .pool_census(parts.chain_id, parts.pool, &t, now)?;
    let lowest =
        parts
            .repo
            .lowest_health(parts.chain_id, parts.pool, t.min_debt_base, CENSUS_LOWEST)?;
    let pct = |a: u64, b: u64| {
        if b == 0 {
            100.0
        } else {
            a as f64 * 100.0 / b as f64
        }
    };
    let data = serde_json::json!({
        "active": c.active, "inactive": c.inactive, "unknown": c.unknown,
        "hot": c.hot, "hot_fresh": c.hot_fresh, "cold": c.cold, "cold_fresh": c.cold_fresh,
        "dirty": c.dirty, "eligible": c.eligible,
        "hot_fresh_pct": pct(c.hot_fresh, c.hot), "cold_fresh_pct": pct(c.cold_fresh, c.cold),
        "min_debt_usd": parts.cfg.min_debt_usd,
        "lowest": lowest.iter().map(|l| serde_json::json!({
            "borrower": format!("{:#x}", l.borrower),
            "hf": f64::from(l.health_factor_wad) / 1e18,
            "debt_usd": f64::from(l.total_debt_base) / USD_BASE as f64,
            "age_s": l.last_check_ns.map(|t| now.saturating_sub(t) / NS),
        })).collect::<Vec<_>>(),
    });
    Ok((c, data))
}

/// Spawn the background refresh loop and the census. Never awaited by the caller.
pub fn spawn_refresh(
    parts: ShadowParts,
    rechecker: mev_lending_aave_v3::recheck::AaveAccountRechecker,
    sink: ShadowSink,
) {
    let (cp, cs) = (parts.clone(), sink.clone());
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(cp.cfg.census_seconds.max(10)));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            let p = cp.clone();
            match tokio::task::spawn_blocking(move || census(&p)).await {
                Ok(Ok((c, data))) => {
                    info!(
                        active = c.active,
                        unknown = c.unknown,
                        hot = c.hot,
                        hot_fresh = c.hot_fresh,
                        cold = c.cold,
                        cold_fresh = c.cold_fresh,
                        dirty = c.dirty,
                        eligible = c.eligible,
                        inactive = c.inactive,
                        lowest = %data["lowest"],
                        "liq_shadow_census"
                    );
                    cs(EVENT_CENSUS, data);
                }
                Ok(Err(e)) => warn!(error = %format!("{e:#}"), "liq_shadow_census failed"),
                Err(e) => warn!(error = %e, "liq_shadow_census panicked"),
            }
        }
    });
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(parts.cfg.tick_ms.max(100)));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut budget = Budget::new(parts.cfg.recheck_budget_per_minute, Instant::now());
        loop {
            iv.tick().await;
            let rc = rechecker.clone();
            match refresh_once(&parts, &mut budget, move |batch| {
                let rc = rc.clone();
                async move { rc.recheck_many(&batch).await.map(|(_, s)| s) }
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
                        store_errors = r.store_errors,
                        "liq_shadow_refresh"
                    );
                    sink(
                        EVENT_REFRESH,
                        serde_json::json!({
                            "candidates": r.candidates, "attempted": r.attempted,
                            "stored": r.stored, "conflicts": r.conflicts,
                            "rpc_errors": r.rpc_errors, "store_errors": r.store_errors,
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

    use mev_liq_state::{AccountKey, ChainRef, ChainSource, SnapshotStatus};

    use super::*;

    const CHAIN: u64 = 42161;

    fn parts(dir: &Path, conc: usize, batch: usize, budget: u32) -> ShadowParts {
        let cfg = LiquidationShadowConfig {
            enabled: true,
            recheck_concurrency: conc,
            recheck_batch: batch,
            recheck_budget_per_minute: budget,
            tick_ms: 60_000, // one tick per minute: the whole budget is one tick's share
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

    fn snap(p: &ShadowParts, b: Address, debt_usd: u64, hf_milli: u64) -> BorrowerSnapshot {
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
            health_factor_wad: U256::from(hf_milli) * U256::from(1_000_000_000_000_000u64),
            total_debt_base: U256::from(debt_usd) * U256::from(USD_BASE),
            total_collateral_base: U256::from(2 * debt_usd) * U256::from(USD_BASE),
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
    fn startup_wiring_builds_a_bounded_schedule() {
        let d = tempfile::tempdir().unwrap();
        let p = parts(d.path(), 2, 100, 12_000);
        assert!(d.path().join("liq-state.sqlite").exists());
        let s = schedule(&p, Arc::new(|_, _| {}));
        assert!(s.enabled);
        let pol = &s.params.policy;
        assert_eq!(pol.max_account_age_ns, 300 * NS);
        assert_eq!(pol.concurrency, 200, "2 batches x 100 accounts");
        // One max-age window (5 min) holds exactly five minutes of budget.
        assert_eq!(mev_liq_sched::sweep_capacity(pol).0, 5 * 12_000);
        let b = s.params.bounds.unwrap();
        assert_eq!(b.min_debt_base, U256::from(10 * USD_BASE));
        assert_eq!(b.max_unknown, 2000);
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
    fn budget_is_a_rolling_per_minute_cap_spread_over_ticks() {
        let t0 = Instant::now();
        let mut b = Budget::new(5, t0);
        assert_eq!(b.take(3, t0), 3);
        assert_eq!(b.available(t0), 2);
        assert_eq!(b.take(10, t0), 2);
        assert_eq!(b.take(10, t0), 0);
        assert_eq!(b.take(10, t0 + Duration::from_secs(61)), 5);
        let cfg = LiquidationShadowConfig {
            recheck_budget_per_minute: 20_000,
            tick_ms: 5000,
            ..Default::default()
        };
        assert_eq!(per_tick(&cfg), 1667, "12 ticks a minute, rounded up");
    }

    #[tokio::test]
    async fn refresh_batches_within_budget_and_concurrency_and_fills_the_repo() {
        let d = tempfile::tempdir().unwrap();
        let p = parts(d.path(), 2, 2, 5);
        let users: Vec<Address> = (1..=8u8).map(Address::with_last_byte).collect();
        p.repo
            .register_borrowers(CHAIN, p.pool, &users, None, 1)
            .unwrap();
        let (cur, peak, batches) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let mut budget = Budget::new(5, Instant::now());
        let (c2, pk2, ba2, p2) = (cur.clone(), peak.clone(), batches.clone(), p.clone());
        let rep = refresh_once(&p, &mut budget, move |batch| {
            let (c, pk, ba, p) = (c2.clone(), pk2.clone(), ba2.clone(), p2.clone());
            async move {
                assert!(batch.len() <= 2, "batch size");
                ba.fetch_add(1, Ordering::SeqCst);
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(batch
                    .iter()
                    .map(|b| Some(snap(&p, *b, 50, 1_500)))
                    .collect())
            }
        })
        .await
        .unwrap();
        assert_eq!((rep.candidates, rep.attempted, rep.stored), (5, 5, 5));
        assert_eq!(batches.load(Ordering::SeqCst), 3, "2 + 2 + 1");
        assert!(peak.load(Ordering::SeqCst) <= 2, "batches in flight");
        let rep2 = refresh_once(&p, &mut budget, |_| async { Ok(vec![]) })
            .await
            .unwrap();
        assert_eq!(rep2.attempted, 0, "the minute's budget is spent");
        let c = census(&p).unwrap().0;
        assert_eq!((c.active, c.hot, c.hot_fresh, c.unknown), (8, 5, 5, 3));
    }

    #[tokio::test]
    async fn failed_reads_count_as_rpc_errors_and_stay_due() {
        let d = tempfile::tempdir().unwrap();
        let p = parts(d.path(), 1, 3, 100);
        let users: Vec<Address> = (1..=3u8).map(Address::with_last_byte).collect();
        p.repo
            .register_borrowers(CHAIN, p.pool, &users, None, 1)
            .unwrap();
        let mut budget = Budget::new(100, Instant::now());
        let p2 = p.clone();
        let rep = refresh_once(&p, &mut budget, move |batch| {
            let p = p2.clone();
            async move {
                // The middle account's call failed.
                Ok(batch
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (i != 1).then(|| snap(&p, *b, 50, 900)))
                    .collect())
            }
        })
        .await
        .unwrap();
        assert_eq!((rep.stored, rep.rpc_errors), (2, 1));
        let rep = refresh_once(&p, &mut budget, |_| async { anyhow::bail!("upstream 503") })
            .await
            .unwrap();
        assert_eq!((rep.attempted, rep.rpc_errors, rep.stored), (1, 1, 0));
        let (c, data) = census(&p).unwrap();
        assert_eq!((c.unknown, c.eligible), (1, 2));
        assert_eq!(data["lowest"].as_array().unwrap().len(), 2);
        assert_eq!(data["lowest"][0]["hf"], 0.9);
    }
}
