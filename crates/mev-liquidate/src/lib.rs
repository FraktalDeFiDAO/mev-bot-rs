//! Aave v3 liquidation monitor: borrower index (from Borrow events, cached on disk), periodic
//! health-factor sweeps, fast polling of at-risk users, and position/reserve reads for planning.
//!
//! Run `AaveMonitor::run` as its own task; it sends liquidatable users over a channel.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    time::Duration,
};

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::{IAaveDataProvider, IAaveOracle, IAavePool};
use mev_config::AaveConfig;
use mev_lending::{Position, ReserveInfo};
use mev_rpc::{call, decode, multicall};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// getUserAccountData is gas-heavy (walks every reserve + oracle), so batch fewer per multicall.
const ACCOUNT_DATA_CHUNK: usize = 50;
/// Log-scan requests per index tick, so catching up on history never blocks for long.
const MAX_SCAN_CHUNKS_PER_TICK: usize = 20;
/// Pause between scan requests: public RPCs rate-limit bursts (HTTP 429).
const SCAN_THROTTLE: Duration = Duration::from_millis(250);
/// How often to run an index step while history is still being back-filled.
const BACKFILL_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Liquidatable {
    pub user: Address,
    pub health_factor: f64,
    pub debt_usd: f64,
}

/// Borrowers seen in Borrow events. Scans forward from the head for new blocks and backward
/// through history, so recent (most relevant) borrowers are found first.
#[derive(Default, Serialize, Deserialize)]
struct BorrowerIndex {
    /// Next block to scan going forward (0 = not initialized yet).
    next_block: u64,
    /// History below this block has not been scanned yet (<= from_block when complete).
    back_block: u64,
    borrowers: BTreeSet<Address>,
}

/// Per-user account summary from `getUserAccountData`.
#[derive(Debug, Clone, Copy)]
struct Account {
    debt_usd: f64,
    collateral_usd: f64,
    health_factor: f64,
}

pub struct AaveMonitor {
    cfg: AaveConfig,
    /// Fast reads (at-risk polling).
    fast: DynProvider,
    /// Heavy reads (log scans, full sweeps).
    scan: DynProvider,
    cache_path: PathBuf,
    index: BorrowerIndex,
    /// Borrowers with debt, from the last sweep.
    active: Vec<Address>,
    /// Users below `at_risk_hf` with enough debt and collateral -> last known HF.
    at_risk: HashMap<Address, f64>,
}

impl AaveMonitor {
    pub fn new(cfg: AaveConfig, fast: DynProvider, cache_path: PathBuf) -> Result<Self> {
        let scan = mev_rpc::http_provider(&cfg.scan_rpc)?;
        let index = std::fs::read(&cache_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<BorrowerIndex>(&b).ok())
            .unwrap_or_default();
        info!(
            borrowers = index.borrowers.len(),
            next_block = index.next_block,
            back_block = index.back_block,
            "aave borrower index loaded"
        );
        Ok(Self {
            cfg,
            fast,
            scan,
            cache_path,
            index,
            active: vec![],
            at_risk: HashMap::new(),
        })
    }

    pub async fn reserves_list(&self) -> Result<Vec<Address>> {
        Ok(IAavePool::new(self.cfg.pool, &self.fast)
            .getReservesList()
            .call()
            .await?)
    }

    /// Scan Borrow events: new blocks first, then a bounded slice of history (newest first).
    /// Returns true once history is fully back-filled.
    pub async fn index_step(&mut self) -> Result<bool> {
        let head = self.scan.get_block_number().await?;
        if self.index.next_block == 0 {
            self.index.next_block = head + 1;
            self.index.back_block = head + 1;
        }
        let step = self.cfg.scan_step.max(1);
        let mut chunks = 0;
        while self.index.next_block <= head && chunks < MAX_SCAN_CHUNKS_PER_TICK {
            let from = self.index.next_block;
            let to = self.scan_forward(from, (from + step - 1).min(head)).await?;
            self.index.next_block = to + 1;
            chunks += 1;
        }
        while self.index.back_block > self.cfg.from_block && chunks < MAX_SCAN_CHUNKS_PER_TICK {
            let to = self.index.back_block - 1;
            let from = self
                .scan_backward(to.saturating_sub(step - 1).max(self.cfg.from_block), to)
                .await?;
            self.index.back_block = from;
            chunks += 1;
        }
        if let Some(dir) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.cache_path, serde_json::to_vec(&self.index)?)?;
        let done = self.index.back_block <= self.cfg.from_block;
        info!(
            borrowers = self.index.borrowers.len(),
            history_left_blocks = self.index.back_block.saturating_sub(self.cfg.from_block),
            done,
            "aave index step"
        );
        Ok(done)
    }

    /// Scan [from, to], shrinking `to` on errors (response too big). Returns the last block scanned.
    async fn scan_forward(&mut self, from: u64, mut to: u64) -> Result<u64> {
        loop {
            match self.get_borrows(from, to).await {
                Ok(()) => return Ok(to),
                Err(e) if to > from + 1_000 && !is_rate_limit(&e) => to = from + (to - from) / 4,
                Err(e) => return Err(e),
            }
        }
    }

    /// Scan [from, to], shrinking `from` upward on errors. Returns the first block scanned.
    async fn scan_backward(&mut self, mut from: u64, to: u64) -> Result<u64> {
        loop {
            match self.get_borrows(from, to).await {
                Ok(()) => return Ok(from),
                Err(e) if to > from + 1_000 && !is_rate_limit(&e) => from = to - (to - from) / 4,
                Err(e) => return Err(e),
            }
        }
    }

    async fn get_borrows(&mut self, from: u64, to: u64) -> Result<()> {
        tokio::time::sleep(SCAN_THROTTLE).await;
        let filter = Filter::new()
            .address(self.cfg.pool)
            .event_signature(IAavePool::Borrow::SIGNATURE_HASH)
            .from_block(from)
            .to_block(to);
        let logs = self
            .scan
            .get_logs(&filter)
            .await
            .context("Borrow log scan failed")?;
        for l in logs {
            if let Some(t) = l.topics().get(2) {
                self.index.borrowers.insert(Address::from_word(*t));
            }
        }
        Ok(())
    }

    /// Health of every indexed borrower: keeps those with debt, and the at-risk subset.
    pub async fn sweep(&mut self) -> Result<()> {
        let users: Vec<Address> = self.index.borrowers.iter().copied().collect();
        let data = self.account_data(&self.scan, &users).await?;
        self.active.clear();
        self.at_risk.clear();
        for (u, a) in users.into_iter().zip(data) {
            let Some(a) = a else { continue };
            if a.debt_usd <= 0.0 {
                continue;
            }
            self.active.push(u);
            if a.health_factor < self.cfg.at_risk_hf && self.worth_watching(&a) {
                self.at_risk.insert(u, a.health_factor);
            }
        }
        let liquidatable = self.at_risk.values().filter(|hf| **hf < 1.0).count();
        info!(
            indexed = self.index.borrowers.len(),
            with_debt = self.active.len(),
            at_risk = self.at_risk.len(),
            liquidatable,
            "aave sweep"
        );
        Ok(())
    }

    /// Re-check at-risk users on the fast RPC; return the liquidatable ones.
    pub async fn poll(&mut self) -> Result<Vec<Liquidatable>> {
        let users: Vec<Address> = self.at_risk.keys().copied().collect();
        if users.is_empty() {
            return Ok(vec![]);
        }
        let data = self.account_data(&self.fast, &users).await?;
        let mut out = Vec::new();
        for (u, a) in users.into_iter().zip(data) {
            let Some(a) = a else { continue };
            self.at_risk.insert(u, a.health_factor);
            if a.health_factor < 1.0 && self.worth_watching(&a) {
                out.push(Liquidatable {
                    user: u,
                    health_factor: a.health_factor,
                    debt_usd: a.debt_usd,
                });
            }
        }
        Ok(out)
    }

    /// Enough debt for the bonus to beat gas, and real collateral to seize (bad debt pays nothing).
    fn worth_watching(&self, a: &Account) -> bool {
        a.debt_usd >= self.cfg.min_debt_usd && a.collateral_usd >= self.cfg.min_debt_usd
    }

    async fn account_data(
        &self,
        p: &DynProvider,
        users: &[Address],
    ) -> Result<Vec<Option<Account>>> {
        let calls: Vec<_> = users
            .iter()
            .map(|u| {
                call(
                    self.cfg.pool,
                    IAavePool::getUserAccountDataCall { user: *u },
                )
            })
            .collect();
        let res = multicall(p, &calls, ACCOUNT_DATA_CHUNK).await?;
        Ok(res
            .iter()
            .map(|r| {
                decode::<IAavePool::getUserAccountDataCall>(r).map(|d| Account {
                    debt_usd: f64::from(d.totalDebtBase) / 1e8,
                    collateral_usd: f64::from(d.totalCollateralBase) / 1e8,
                    health_factor: f64::from(d.healthFactor.min(U256::from(u128::MAX))) / 1e18,
                })
            })
            .collect())
    }

    /// Run forever: index new/historical borrowers, sweep everyone every `sweep_secs`, and poll
    /// at-risk users every `poll_secs`. Errors back off instead of hammering the RPC.
    pub async fn run(mut self, tx: mpsc::UnboundedSender<Liquidatable>) {
        let sweep_every = Duration::from_secs(self.cfg.sweep_secs);
        let mut index_tick = tokio::time::interval(BACKFILL_EVERY);
        let mut sweep = tokio::time::interval(sweep_every);
        let mut poll = tokio::time::interval(Duration::from_secs(self.cfg.poll_secs));
        let mut backfilled = false;
        let mut backoff = mev_rpc::Backoff::default();
        loop {
            tokio::select! {
                _ = index_tick.tick() => match self.index_step().await {
                    Ok(done) => {
                        backoff.reset();
                        if done && !backfilled {
                            backfilled = true;
                            info!("aave history back-filled; indexing new blocks once per sweep");
                            index_tick = tokio::time::interval(sweep_every);
                            index_tick.reset();
                        }
                    }
                    Err(e) => {
                        let d = backoff.next_delay();
                        warn!(error = %format!("{e:#}"), retry_in_s = d.as_secs(), "aave index failed");
                        index_tick.reset_after(d);
                    }
                },
                _ = sweep.tick() => {
                    if let Err(e) = self.sweep().await {
                        warn!(error = %format!("{e:#}"), "aave sweep failed; retrying in 30s");
                        sweep.reset_after(Duration::from_secs(30));
                    }
                }
                _ = poll.tick() => match self.poll().await {
                    Ok(users) => {
                        for u in users {
                            if tx.send(u).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => warn!(error = %format!("{e:#}"), "aave poll failed"),
                },
            }
        }
    }
}

fn is_rate_limit(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}");
    s.contains("429") || s.contains("Too Many Requests")
}

/// Reserve parameters and oracle prices for planning (one multicall).
pub async fn reserves_info(
    p: &DynProvider,
    cfg: &AaveConfig,
    assets: &[Address],
    decimals: &HashMap<Address, u8>,
) -> Result<Vec<ReserveInfo>> {
    let mut calls = Vec::new();
    for a in assets {
        calls.push(call(
            cfg.data_provider,
            IAaveDataProvider::getReserveConfigurationDataCall { asset: *a },
        ));
        calls.push(call(
            cfg.data_provider,
            IAaveDataProvider::getLiquidationProtocolFeeCall { asset: *a },
        ));
        calls.push(call(
            cfg.oracle,
            IAaveOracle::getAssetPriceCall { asset: *a },
        ));
    }
    let res = multicall(p, &calls, 200).await?;
    let mut out = Vec::new();
    for (i, a) in assets.iter().enumerate() {
        let (Some(c), Some(fee), Some(px)) = (
            decode::<IAaveDataProvider::getReserveConfigurationDataCall>(&res[3 * i]),
            decode::<IAaveDataProvider::getLiquidationProtocolFeeCall>(&res[3 * i + 1]),
            decode::<IAaveOracle::getAssetPriceCall>(&res[3 * i + 2]),
        ) else {
            continue;
        };
        out.push(ReserveInfo {
            asset: *a,
            decimals: decimals
                .get(a)
                .copied()
                .unwrap_or(u8::try_from(c.decimals).unwrap_or(18)),
            liquidation_bonus_bps: u32::try_from(c.liquidationBonus).unwrap_or(0),
            protocol_fee_bps: u32::try_from(fee).unwrap_or(0),
            price_usd: f64::from(px) / 1e8,
        });
    }
    Ok(out)
}

/// A user's collateral and debt balances across `assets`.
pub async fn position(
    p: &DynProvider,
    cfg: &AaveConfig,
    user: Address,
    health_factor: f64,
    assets: &[Address],
) -> Result<Position> {
    let calls: Vec<_> = assets
        .iter()
        .map(|a| {
            call(
                cfg.data_provider,
                IAaveDataProvider::getUserReserveDataCall { asset: *a, user },
            )
        })
        .collect();
    let res = multicall(p, &calls, 200).await?;
    let mut pos = Position {
        user,
        health_factor,
        ..Default::default()
    };
    for (a, r) in assets.iter().zip(res.iter()) {
        let Some(d) = decode::<IAaveDataProvider::getUserReserveDataCall>(r) else {
            continue;
        };
        let coll = f64::from(d.currentATokenBalance);
        let debt = f64::from(d.currentVariableDebt) + f64::from(d.currentStableDebt);
        if coll > 0.0 && d.usageAsCollateralEnabled {
            pos.collateral.push((*a, coll));
        }
        if debt > 0.0 {
            pos.debt.push((*a, debt));
        }
    }
    Ok(pos)
}

/// Raw debt balance of `asset` for `user` (for sizing the flash loan).
pub fn debt_of(pos: &Position, asset: Address) -> f64 {
    pos.debt
        .iter()
        .find(|(a, _)| *a == asset)
        .map(|(_, d)| *d)
        .unwrap_or(0.0)
}
