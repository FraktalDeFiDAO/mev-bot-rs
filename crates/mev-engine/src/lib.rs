//! Arbitrage engine: bootstraps the building blocks for one chain and runs the
//! event -> search -> simulate -> risk -> send -> receipt loop.
//!
//! Any binary (bot, backtester, dashboard) can embed it:
//! `Engine::bootstrap(cfg, secrets, Mode::Shadow, data_dir).await?.run().await`.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy::{
    primitives::{Address, TxHash, U256},
    providers::Provider,
};
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use mev_bindings::ArbExecutor;
use mev_config::{ChainConfig, Secrets};
use mev_exec::{Landed, Sender};
use mev_lending::{AdapterRpc, LiquidationAdapter, Plan};
use mev_liquidate::{Runner, RunnerConfig, RunnerEvent};
use mev_metrics::Metrics;
use mev_risk::{RiskGuard, RiskLimits, loan_cap};
use mev_rpc::{Backoff, Rpc};
use mev_search::{SearchParams, best_route, two_pool_arbs};
pub use mev_sim::SimOutcome;
use mev_sim::Simulator;
use mev_state::PoolStore;
use mev_types::Opportunity;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Share of the flash lender's balance we're willing to borrow.
const LENDER_SHARE: f64 = 0.5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Find and simulate opportunities; never send.
    Shadow,
    /// Send transactions for opportunities that pass simulation and risk checks.
    Live,
}

pub struct Engine {
    cfg: ChainConfig,
    rpc: Rpc,
    store: PoolStore,
    usd: Address,
    native: Address,
    usd_decimals: u8,
    max_loan: HashMap<Address, f64>,
    sim: Option<Simulator>,
    sender: Option<Arc<Sender>>,
    risk: RiskGuard,
    metrics: Arc<Metrics>,
    last_sim: HashMap<(Address, Address), Instant>,
    mode: Mode,
    liq: Option<LiqCtx>,
}

/// Liquidation side of the engine (the runner and its adapters run as their own task).
struct LiqCtx {
    via: Vec<Address>,
    rx: Option<mpsc::UnboundedReceiver<RunnerEvent>>,
}

/// Result of `Engine::scan_once` for one route.
pub struct ScanResult {
    pub loan: String,
    pub loan_px: f64,
    pub buy: String,
    pub sell: String,
    pub opp: Opportunity,
    pub sim: Option<SimOutcome>,
}

enum TxDone {
    Landed {
        hash: TxHash,
        kind: &'static str,
        est_profit_usd: f64,
        landed: Landed,
    },
    Failed {
        hash: Option<TxHash>,
        error: String,
    },
}

impl Engine {
    /// Connect, discover pools, size loans, and wire the executor (if deployed).
    pub async fn bootstrap(
        cfg: ChainConfig,
        secrets: Secrets,
        mode: Mode,
        data_dir: PathBuf,
    ) -> Result<Self> {
        info!(chain = cfg.name, ?mode, "starting");
        let rpc = Rpc::connect(&secrets.http_url, &secrets.ws_url).await?;
        let usd = cfg.token(&cfg.usd_token)?;
        let native = cfg.token(&cfg.native_token)?;

        let data_dir = data_dir.join(&cfg.name);
        let mut pairs = mev_discovery::candidate_pairs(&rpc, &cfg).await?;

        // Liquidation adapters; their assets need swap routes (seized collateral -> debt asset).
        let liq_setup =
            build_liquidation(&cfg, &rpc, &data_dir, secrets.executor.unwrap_or_default()).await?;
        if let Some((runner, via)) = &liq_setup {
            for a in runner.assets() {
                for v in via {
                    pairs.push((a, *v));
                }
            }
        }

        let infos = mev_discovery::discover_pools(&rpc, &cfg, &pairs).await?;
        let pools = mev_discovery::load_pools(&rpc, infos).await?;
        let tokens: Vec<Address> = pools
            .iter()
            .flat_map(|p| [p.info.token0, p.info.token1])
            .chain(cfg.tokens.values().copied())
            .chain(liq_setup.iter().flat_map(|(r, _)| r.assets()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let decimals = mev_discovery::token_decimals(&rpc, &tokens).await?;
        let usd_decimals = *decimals.get(&usd).context("USD token decimals")?;
        let store = mev_discovery::build_store(
            pools,
            cfg.min_pool_liquidity_usd,
            usd,
            usd_decimals,
            native,
        );
        if store.is_empty() {
            bail!("no pools passed discovery");
        }

        let loan_tokens: Vec<Address> = cfg
            .loan_tokens
            .iter()
            .map(|s| cfg.token(s))
            .collect::<Result<_>>()?;
        let lender_bal = mev_discovery::balances(&rpc, cfg.balancer_vault, &loan_tokens).await?;
        let mut max_loan = HashMap::new();
        for t in &loan_tokens {
            let (Some(bal), Some(px)) = (
                lender_bal.get(t),
                store.usd_per_raw_unit(*t, usd, usd_decimals, native),
            ) else {
                continue;
            };
            let cap = loan_cap(cfg.risk.max_loan_usd, px, f64::from(*bal), LENDER_SHARE);
            if cap > 0.0 {
                info!(token = cfg.symbol(*t), cap_usd = cap * px, "loan cap");
                max_loan.insert(*t, cap);
            }
        }

        let (sim, sender) = wire_executor(&cfg, &secrets, &rpc, mode).await?;

        let limits = RiskLimits {
            min_profit_usd: cfg.risk.min_profit_usd,
            max_gas_usd_per_day: cfg.risk.max_gas_usd_per_day,
            max_consecutive_failures: cfg.risk.max_consecutive_failures,
        };
        let metrics = Arc::new(Metrics::new(data_dir.clone())?);
        let risk = RiskGuard::new(limits, data_dir.join("STOP"));
        info!(pools = store.len(), kill_switch = %data_dir.join("STOP").display(), "ready");

        let liq = match liq_setup {
            Some((runner, via)) => {
                if let Some(exec) = secrets.executor {
                    check_allowlist(&rpc, exec, &runner.targets()).await;
                }
                // Sequencer feed: real-time txs drive price-feed-triggered re-checks.
                let feed = cfg.sequencer_feed.clone().map(|url| {
                    let (tx, rx) = mpsc::unbounded_channel();
                    tokio::spawn(mev_sequencer::run(url, tx));
                    rx
                });
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(runner.run(feed, tx));
                Some(LiqCtx { via, rx: Some(rx) })
            }
            None => None,
        };

        Ok(Self {
            cfg,
            rpc,
            store,
            usd,
            native,
            usd_decimals,
            max_loan,
            sim,
            sender,
            risk,
            metrics,
            last_sim: HashMap::new(),
            mode,
            liq,
        })
    }

    /// Run until Ctrl-C. Resubscribes and resyncs if the event stream drops.
    pub async fn run(&mut self) -> Result<()> {
        let (tx_done, mut rx_done) = mpsc::unbounded_channel::<TxDone>();
        let mut resync = tokio::time::interval(Duration::from_secs(60));
        let mut summary = tokio::time::interval(Duration::from_secs(60));
        let mut backoff = Backoff::default();
        let mut liq_rx = self.liq.as_mut().and_then(|l| l.rx.take());
        loop {
            // Never exit on network trouble: resubscribe, and if the WS provider itself is dead
            // (alloy gave up reconnecting), replace it — with backoff so outages don't spin.
            let mut logs = loop {
                match mev_ingest::pool_logs(&self.rpc.ws, self.store.addresses()).await {
                    Ok(stream) => break Box::pin(stream),
                    Err(e) => {
                        let d = backoff.next_delay();
                        warn!(error = %format!("{e:#}"), retry_in_s = d.as_secs(), "subscribe failed; reconnecting WS");
                        tokio::time::sleep(d).await;
                        if let Err(e) = self.rpc.reconnect_ws().await {
                            warn!(error = %format!("{e:#}"), "WS reconnect failed");
                        }
                    }
                }
            };
            backoff.reset();
            info!("subscribed to pool events");
            loop {
                tokio::select! {
                    maybe = logs.next() => {
                        let Some(log) = maybe else { warn!("log stream ended; resubscribing"); break };
                        if let Some(pool) = self.store.apply_log(&log) {
                            self.metrics.update(|c| c.updates += 1);
                            self.on_pool_update(pool, &tx_done).await;
                        }
                    }
                    Some(done) = rx_done.recv() => self.on_tx_done(done),
                    Some(ev) = recv_opt(&mut liq_rx) => self.on_runner_event(ev, &tx_done).await,
                    _ = resync.tick() => {
                        if let Err(e) = mev_discovery::refresh(&self.rpc, &mut self.store).await {
                            warn!(error = %e, "resync failed");
                        }
                    }
                    _ = summary.tick() => self.log_summary(),
                    _ = tokio::signal::ctrl_c() => { info!("shutting down"); return Ok(()); }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Err(e) = mev_discovery::refresh(&self.rpc, &mut self.store).await {
                error!(error = %e, "resync after reconnect failed");
            }
        }
    }

    /// One-shot diagnostic: search every pool now (no profit floor), dedupe routes, and simulate
    /// up to `max_sims` of the best ones through the executor. Never sends.
    pub async fn scan_once(&self, max_sims: usize) -> Vec<ScanResult> {
        let price = |t: Address| self.price(t);
        let params = SearchParams {
            max_loan: &self.max_loan,
            usd_per_raw: &price,
            min_est_profit_usd: 0.0,
        };
        let mut seen = std::collections::HashSet::new();
        let mut opps: Vec<Opportunity> = Vec::new();
        for addr in self.store.addresses() {
            for o in two_pool_arbs(&self.store, addr, &params) {
                if seen.insert((o.buy_pool, o.sell_pool)) {
                    opps.push(o);
                }
            }
        }
        opps.sort_by(|a, b| b.est_profit_usd.total_cmp(&a.est_profit_usd));
        let mut out = Vec::new();
        for opp in opps.into_iter().take(max_sims) {
            let sim = match &self.sim {
                Some(sim) => Some(
                    sim.simulate(&opp, U256::ZERO)
                        .await
                        .unwrap_or_else(|e| SimOutcome::Reverted(e.to_string())),
                ),
                None => None,
            };
            let loan_px = self.price(opp.loan_token).unwrap_or(0.0);
            let buy = self
                .store
                .get(&opp.buy_pool)
                .map(|p| p.info.dex.clone())
                .unwrap_or_default();
            let sell = self
                .store
                .get(&opp.sell_pool)
                .map(|p| p.info.dex.clone())
                .unwrap_or_default();
            out.push(ScanResult {
                loan: self.cfg.symbol(opp.loan_token),
                loan_px,
                buy,
                sell,
                opp,
                sim,
            });
        }
        out
    }

    pub fn pool_count(&self) -> usize {
        self.store.len()
    }

    fn price(&self, token: Address) -> Option<f64> {
        self.store
            .usd_per_raw_unit(token, self.usd, self.usd_decimals, self.native)
    }

    fn log_summary(&self) {
        self.metrics.heartbeat();
        let c = self.metrics.snapshot();
        info!(
            updates = c.updates,
            candidates = c.candidates,
            simulated = c.simulated,
            profitable = c.sim_profitable,
            sent = c.sent,
            ok = c.landed_ok,
            reverted = c.landed_reverted,
            profit_usd = format!("{:.4}", c.profit_usd),
            gas_usd = format!("{:.4}", c.gas_usd),
            gas_today_usd = format!("{:.4}", self.risk.spent_today_usd()),
            "summary"
        );
    }

    async fn on_pool_update(&mut self, pool: Address, tx_done: &mpsc::UnboundedSender<TxDone>) {
        let price = |t: Address| self.price(t);
        let params = SearchParams {
            max_loan: &self.max_loan,
            usd_per_raw: &price,
            min_est_profit_usd: self.cfg.risk.min_profit_usd,
        };
        let Some(opp) = two_pool_arbs(&self.store, pool, &params).into_iter().next() else {
            return;
        };
        let loan_px = self.price(opp.loan_token).unwrap_or(0.0);
        let native_px = self.price(self.native).unwrap_or(0.0);

        self.metrics.update(|c| c.candidates += 1);
        let base = json!({
            "loan": self.cfg.symbol(opp.loan_token), "amount": opp.amount_in.to_string(),
            "est_profit_usd": opp.est_profit_usd, "buy": opp.buy_pool, "sell": opp.sell_pool,
        });
        let Some(sim) = &self.sim else {
            self.metrics.event("candidate", base);
            return;
        };

        // Don't re-simulate the same route more than twice a second.
        let key = (opp.buy_pool, opp.sell_pool);
        if self
            .last_sim
            .get(&key)
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_sim.insert(key, Instant::now());
        self.metrics.update(|c| c.simulated += 1);

        let outcome = match sim.simulate(&opp, U256::ZERO).await {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "simulation error");
                return;
            }
        };
        let (profit, gas) = match outcome {
            SimOutcome::Profitable { profit, gas } => (profit, gas),
            SimOutcome::Reverted(r) => {
                self.metrics
                    .event("sim_revert", json!({ "opp": base, "reason": r }));
                return;
            }
        };
        let gas_price = self.rpc.http.get_gas_price().await.unwrap_or(10_000_000) as f64;
        let gas_usd = gas as f64 * gas_price * native_px;
        let profit_usd = f64::from(profit) * loan_px;
        let net_usd = profit_usd - gas_usd;
        self.metrics.update(|c| c.sim_profitable += 1);
        self.metrics.event(
            "sim_ok",
            json!({ "opp": base, "profit_usd": profit_usd, "gas": gas, "gas_usd": gas_usd, "net_usd": net_usd }),
        );
        info!(
            loan = self.cfg.symbol(opp.loan_token),
            profit_usd = format!("{profit_usd:.4}"),
            gas_usd = format!("{gas_usd:.4}"),
            net_usd = format!("{net_usd:.4}"),
            "simulated opportunity"
        );

        if self.mode != Mode::Live {
            return;
        }
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if let Err(block) = self.risk.check(net_usd, gas_usd, now_secs()) {
            self.metrics.event(
                "risk_block",
                json!({ "opp": base, "reason": format!("{block:?}") }),
            );
            return;
        }
        // On-chain floor: the trade must at least pay for its gas, or it reverts.
        let min_profit = if loan_px > 0.0 {
            U256::from((gas_usd / loan_px) as u128)
        } else {
            U256::ZERO
        };
        let gas_limit = gas + gas / 4;
        self.risk.mark_sent();
        self.metrics.update(|c| c.sent += 1);
        let tx_done = tx_done.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let done = match sender.send(&opp, min_profit, gas_limit).await {
                Ok(hash) => {
                    metrics.event("sent", json!({ "hash": hash, "net_usd": net_usd }));
                    info!(%hash, "sent");
                    match sender.wait(hash, Duration::from_secs(30)).await {
                        Ok(landed) => TxDone::Landed {
                            hash,
                            kind: "arb",
                            est_profit_usd: opp.est_profit_usd,
                            landed,
                        },
                        Err(e) => TxDone::Failed {
                            hash: Some(hash),
                            error: e.to_string(),
                        },
                    }
                }
                Err(e) => TxDone::Failed {
                    hash: None,
                    error: e.to_string(),
                },
            };
            let _ = tx_done.send(done);
        });
    }

    async fn on_runner_event(&mut self, ev: RunnerEvent, tx_done: &mpsc::UnboundedSender<TxDone>) {
        match ev {
            RunnerEvent::Plan(p) => self.on_plan(p, tx_done).await,
            RunnerEvent::NoPlan(h) => self.metrics.event(
                "liq_no_plan",
                json!({ "account": h.account.to_string(), "hf": h.health_factor, "debt_usd": h.debt_usd }),
            ),
            RunnerEvent::FeedTrigger { adapter, lead } => {
                self.metrics.update(|c| c.feed_triggers += 1);
                tracing::debug!(adapter, lead_ms = lead.as_millis() as u64, "price-feed trigger");
            }
        }
    }

    /// Route, simulate, and (in live mode) send a liquidation planned by an adapter.
    async fn on_plan(&mut self, plan: Plan, tx_done: &mpsc::UnboundedSender<TxDone>) {
        let Some(liq) = self.liq.as_ref() else { return };
        let via = liq.via.clone();
        self.metrics.update(|c| c.liq_planned += 1);
        let base = json!({ "protocol": plan.protocol, "account": plan.account.to_string(),
            "collateral": self.cfg.symbol(plan.collateral_asset), "debt": self.cfg.symbol(plan.debt_asset),
            "bonus_usd": plan.bonus_usd });
        if plan.bonus_usd < self.cfg.risk.min_profit_usd {
            return self.metrics.event("liq_dust", base);
        }
        let hops = if plan.collateral_asset == plan.debt_asset {
            vec![]
        } else {
            match best_route(
                &self.store,
                plan.collateral_asset,
                plan.debt_asset,
                plan.expected_collateral,
                &via,
            ) {
                Some((h, _)) => h,
                None => return self.metrics.event("liq_no_route", base),
            }
        };
        match self.max_loan.get(&plan.debt_asset) {
            Some(cap) if f64::from(plan.flash_amount) <= *cap => {}
            Some(_) => return self.metrics.event("liq_too_large", base),
            None => return self.metrics.event("liq_no_lender", base),
        }
        let Some(sim) = &self.sim else {
            return self.metrics.event("liq_candidate", base);
        };
        let (profit, gas) = match sim.simulate_liquidation(&plan, &hops, U256::ZERO).await {
            Ok(SimOutcome::Profitable { profit, gas }) => (profit, gas),
            Ok(SimOutcome::Reverted(r)) => {
                return self
                    .metrics
                    .event("liq_sim_revert", json!({ "liq": base, "reason": r }));
            }
            Err(e) => return warn!(error = %e, "liquidation simulation error"),
        };
        let Some(debt_px) = self.price(plan.debt_asset) else {
            return self.metrics.event("liq_no_price", base);
        };
        let profit_usd = f64::from(profit) * debt_px;
        let native_px = self.price(self.native).unwrap_or(0.0);
        let gas_price = self.rpc.http.get_gas_price().await.unwrap_or(10_000_000) as f64;
        let gas_usd = gas as f64 * gas_price * native_px;
        let net_usd = profit_usd - gas_usd;
        self.metrics.update(|c| c.liq_profitable += 1);
        self.metrics.event(
            "liq_sim_ok",
            json!({ "liq": base, "profit_usd": profit_usd, "gas": gas, "gas_usd": gas_usd, "net_usd": net_usd }),
        );
        info!(protocol = plan.protocol, account = %plan.account, profit_usd = format!("{profit_usd:.4}"),
              gas_usd = format!("{gas_usd:.4}"), net_usd = format!("{net_usd:.4}"), "simulated liquidation");

        if self.mode != Mode::Live {
            return;
        }
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if let Err(block) = self.risk.check(net_usd, gas_usd, now_secs()) {
            return self.metrics.event(
                "risk_block",
                json!({ "liq": base, "reason": format!("{block:?}") }),
            );
        }
        // On-chain floor: at least cover gas, in debt-asset units.
        let min_profit = if debt_px > 0.0 {
            U256::from((gas_usd / debt_px) as u128)
        } else {
            U256::ZERO
        };
        let gas_limit = gas + gas * 3 / 10;
        self.risk.mark_sent();
        self.metrics.update(|c| c.sent += 1);
        let tx_done = tx_done.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let done = match sender
                .send_liquidation(&plan, &hops, min_profit, gas_limit)
                .await
            {
                Ok(hash) => {
                    metrics.event(
                        "sent",
                        json!({ "hash": hash, "kind": "liquidation", "net_usd": net_usd }),
                    );
                    info!(%hash, protocol = plan.protocol, "sent liquidation");
                    match sender.wait(hash, Duration::from_secs(30)).await {
                        Ok(landed) => TxDone::Landed {
                            hash,
                            kind: "liquidation",
                            est_profit_usd: net_usd,
                            landed,
                        },
                        Err(e) => TxDone::Failed {
                            hash: Some(hash),
                            error: e.to_string(),
                        },
                    }
                }
                Err(e) => TxDone::Failed {
                    hash: None,
                    error: e.to_string(),
                },
            };
            let _ = tx_done.send(done);
        });
    }

    fn on_tx_done(&mut self, done: TxDone) {
        let native_px = self.price(self.native).unwrap_or(0.0);
        match done {
            TxDone::Landed {
                hash,
                kind,
                est_profit_usd,
                landed,
            } => {
                let gas_usd =
                    landed.gas_used as f64 * landed.effective_gas_price as f64 * native_px;
                self.risk.record(landed.success, gas_usd, now_secs());
                self.metrics.update(|c| {
                    c.gas_usd += gas_usd;
                    if landed.success {
                        c.landed_ok += 1;
                        c.profit_usd += est_profit_usd;
                    } else {
                        c.landed_reverted += 1;
                    }
                });
                self.metrics.event(
                    "landed",
                    json!({ "hash": hash, "success": landed.success, "gas_used": landed.gas_used,
                            "gas_usd": gas_usd, "block": landed.block, "est_profit_usd": est_profit_usd, "kind": kind }),
                );
                info!(%hash, success = landed.success, gas_usd = format!("{gas_usd:.4}"), "landed");
            }
            TxDone::Failed { hash, error } => {
                self.risk.record(false, 0.0, now_secs());
                self.metrics
                    .event("send_failed", json!({ "hash": hash, "error": error }));
                warn!(?hash, %error, "send failed");
            }
        }
    }
}

/// Build the simulator (needs a deployed executor) and, in live mode, the signing sender.
async fn wire_executor(
    cfg: &ChainConfig,
    secrets: &Secrets,
    rpc: &Rpc,
    mode: Mode,
) -> Result<(Option<Simulator>, Option<Arc<Sender>>)> {
    let Some(exec) = secrets.executor else {
        if mode == Mode::Live {
            bail!("live mode needs EXECUTOR_ADDRESS");
        }
        warn!("EXECUTOR_ADDRESS not set: shadow mode will log candidates without simulating");
        return Ok((None, None));
    };
    let contract = ArbExecutor::new(exec, &rpc.http);
    let owner = contract
        .owner()
        .call()
        .await
        .context("reading executor owner")?;
    let receiver = contract.profitReceiver().call().await?;
    info!(%exec, %owner, %receiver, "executor");
    let sender = match (&secrets.private_key, mode) {
        (Some(pk), Mode::Live) => {
            let extra: Vec<String> = cfg.sequencer_rpc.iter().cloned().collect();
            let s = Sender::new(pk, rpc.http.clone(), &extra, exec, cfg.chain_id).await?;
            if s.address != owner {
                bail!(
                    "PRIVATE_KEY address {} is not the executor owner {owner}",
                    s.address
                );
            }
            let bal = s.eth_balance().await?;
            info!(address = %s.address, eth = %alloy::primitives::utils::format_ether(bal), "hot wallet");
            Some(Arc::new(s))
        }
        (None, Mode::Live) => bail!("live mode needs PRIVATE_KEY"),
        _ => None,
    };
    Ok((Some(Simulator::new(rpc.http.clone(), exec, owner)), sender))
}

/// Build the liquidation runner from config: one adapter per enabled protocol section.
async fn build_liquidation(
    cfg: &ChainConfig,
    rpc: &Rpc,
    data_dir: &std::path::Path,
    executor: Address,
) -> Result<Option<(Runner, Vec<Address>)>> {
    let Some(lcfg) = cfg.liquidation.clone().filter(|l| l.enabled) else {
        return Ok(None);
    };
    let arpc = AdapterRpc {
        fast: rpc.http.clone(),
        scan: mev_rpc::http_provider(&lcfg.scan_rpc)?,
        sweep: mev_rpc::http_provider(&lcfg.sweep_rpc)?,
    };
    let mut adapters: Vec<Box<dyn LiquidationAdapter>> = Vec::new();
    if let Some(a) = cfg.aave.clone().filter(|a| a.enabled) {
        let cache = data_dir.join("aave_v3_borrowers.json");
        adapters.push(Box::new(
            mev_lending_aave_v3::AaveV3Adapter::new(a, arpc.clone(), cache).await?,
        ));
    }
    if let Some(c) = cfg.compound_v3.clone().filter(|c| c.enabled) {
        let cache = data_dir.join("compound_v3_borrowers.json");
        adapters.push(Box::new(
            mev_lending_compound_v3::CompoundV3Adapter::new(c, arpc.clone(), cache).await?,
        ));
    }
    if let Some(m) = cfg.morpho.clone().filter(|m| m.enabled) {
        let cache = data_dir.join("morpho_borrowers.json");
        adapters.push(Box::new(
            mev_lending_morpho::MorphoAdapter::new(m, arpc.clone(), cache).await?,
        ));
    }
    if let Some(f) = cfg.fluid.clone().filter(|f| f.enabled) {
        adapters.push(Box::new(
            mev_lending_fluid::FluidAdapter::new(f, arpc.clone()).await?,
        ));
    }
    // Venues not live on this chain report their status; the runner logs and skips them.
    adapters.push(Box::new(mev_lending_radiant::Adapter::new()));
    adapters.push(Box::new(mev_lending_spark::Adapter::new()));
    adapters.push(Box::new(mev_lending_dolomite::Adapter::new()));
    adapters.push(Box::new(mev_lending_gearbox::Adapter::new()));
    adapters.push(Box::new(mev_lending_silo::Adapter::new()));
    let via: Vec<Address> = lcfg
        .route_via
        .iter()
        .map(|s| cfg.token(s))
        .collect::<Result<_>>()?;
    let rcfg = RunnerConfig {
        at_risk_hf: lcfg.at_risk_hf,
        min_debt_usd: lcfg.min_debt_usd,
        sweep_secs: lcfg.sweep_secs,
        fallback_poll_secs: lcfg.fallback_poll_secs,
        executor,
    };
    Ok(Some((Runner::new(rcfg, adapters), via)))
}

/// Warn (with the exact fix) about liquidation targets the executor hasn't allowlisted.
async fn check_allowlist(rpc: &Rpc, exec: Address, targets: &[(&'static str, Address)]) {
    let c = ArbExecutor::new(exec, &rpc.http);
    for (protocol, t) in targets {
        match c.allowedTarget(*t).call().await {
            Ok(true) => {}
            Ok(false) => warn!(
                protocol,
                target = %t,
                fix = format!("cast send {exec} 'setTarget(address,bool)' {t} true --private-key $PRIVATE_KEY --rpc-url $ARBITRUM_HTTP_URL"),
                "liquidation target not allowlisted on executor; liquidations will revert until it is"
            ),
            Err(e) => warn!(protocol, target = %t, error = %e, "could not read executor allowlist"),
        }
    }
}

/// `recv` on an optional channel; pends forever when there is none (so select! ignores it).
async fn recv_opt<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
