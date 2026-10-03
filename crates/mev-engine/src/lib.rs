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
use mev_metrics::Metrics;
use mev_risk::{RiskGuard, RiskLimits, loan_cap};
use mev_rpc::{Backoff, Rpc};
use mev_search::{SearchParams, two_pool_arbs};
use mev_sim::{SimOutcome, Simulator};
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
}

enum TxDone {
    Landed {
        hash: TxHash,
        opp: Opportunity,
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

        let infos = mev_discovery::discover_pools(&rpc, &cfg).await?;
        let pools = mev_discovery::load_pools(&rpc, infos).await?;
        let tokens: Vec<Address> = cfg.tokens.values().copied().collect();
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
        let data_dir = data_dir.join(&cfg.name);
        let metrics = Arc::new(Metrics::new(data_dir.clone())?);
        let risk = RiskGuard::new(limits, data_dir.join("STOP"));
        info!(pools = store.len(), kill_switch = %data_dir.join("STOP").display(), "ready");

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
        })
    }

    /// Run until Ctrl-C. Resubscribes and resyncs if the event stream drops.
    pub async fn run(&mut self) -> Result<()> {
        let (tx_done, mut rx_done) = mpsc::unbounded_channel::<TxDone>();
        let mut resync = tokio::time::interval(Duration::from_secs(60));
        let mut summary = tokio::time::interval(Duration::from_secs(60));
        let mut backoff = Backoff::default();
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
                        Ok(landed) => TxDone::Landed { hash, opp, landed },
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
            TxDone::Landed { hash, opp, landed } => {
                let gas_usd =
                    landed.gas_used as f64 * landed.effective_gas_price as f64 * native_px;
                self.risk.record(landed.success, gas_usd, now_secs());
                self.metrics.update(|c| {
                    c.gas_usd += gas_usd;
                    if landed.success {
                        c.landed_ok += 1;
                        c.profit_usd += opp.est_profit_usd;
                    } else {
                        c.landed_reverted += 1;
                    }
                });
                self.metrics.event(
                    "landed",
                    json!({ "hash": hash, "success": landed.success, "gas_used": landed.gas_used,
                            "gas_usd": gas_usd, "block": landed.block, "est_profit_usd": opp.est_profit_usd }),
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
