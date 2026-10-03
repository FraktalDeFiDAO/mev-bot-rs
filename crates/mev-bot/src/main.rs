//! Bot binary: wires the crates together. No strategy logic lives here.

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
use clap::{Parser, ValueEnum};
use futures::StreamExt;
use mev_bindings::ArbExecutor;
use mev_config::{ChainConfig, Secrets};
use mev_exec::{Landed, Sender};
use mev_metrics::Metrics;
use mev_risk::{RiskGuard, RiskLimits};
use mev_rpc::Rpc;
use mev_search::{SearchParams, two_pool_arbs};
use mev_sim::{SimOutcome, Simulator};
use mev_state::PoolStore;
use mev_types::Opportunity;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Find and simulate opportunities; never send.
    Shadow,
    /// Send transactions for opportunities that pass simulation and risk checks.
    Live,
}

#[derive(Parser, Debug)]
#[command(about = "L2 MEV bot")]
struct Args {
    #[arg(long, default_value = "config/arbitrum.toml")]
    config: PathBuf,
    #[arg(long, value_enum, default_value = "shadow")]
    mode: Mode,
    /// Directory for event logs and the STOP kill-switch file.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

struct Ctx {
    cfg: ChainConfig,
    rpc: Rpc,
    store: PoolStore,
    decimals: HashMap<Address, u8>,
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

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let cfg = ChainConfig::load(&args.config)?;
    let secrets = Secrets::from_env(&cfg.name.to_uppercase())?;
    info!(chain = cfg.name, mode = ?args.mode, "starting");

    let rpc = Rpc::connect(&secrets.http_url, &secrets.ws_url).await?;
    let usd = cfg.token(&cfg.usd_token)?;
    let native = cfg.token(&cfg.native_token)?;

    // Pools and token metadata.
    let infos = mev_discovery::discover_pools(&rpc, &cfg).await?;
    let pools = mev_discovery::load_pools(&rpc, infos).await?;
    let tokens: Vec<Address> = cfg.tokens.values().copied().collect();
    let decimals = mev_discovery::token_decimals(&rpc, &tokens).await?;
    let usd_dec = *decimals.get(&usd).context("USD token decimals")?;
    let store = mev_discovery::build_store(pools, cfg.min_pool_liquidity_usd, usd, usd_dec, native);
    if store.is_empty() {
        bail!("no pools passed discovery");
    }

    // Loan caps: min(max_loan_usd, 50% of what the Balancer vault holds).
    let loan_tokens: Vec<Address> = cfg
        .loan_tokens
        .iter()
        .map(|s| cfg.token(s))
        .collect::<Result<_>>()?;
    let vault_bal = mev_discovery::balances(&rpc, cfg.balancer_vault, &loan_tokens).await?;
    let mut max_loan = HashMap::new();
    for t in &loan_tokens {
        let (Some(bal), Some(px)) = (
            vault_bal.get(t),
            store.usd_per_raw_unit(*t, usd, usd_dec, native),
        ) else {
            continue;
        };
        let cap = (cfg.risk.max_loan_usd / px).min(f64::from(*bal) * 0.5);
        if cap > 0.0 {
            info!(token = cfg.symbol(*t), cap_usd = cap * px, "loan cap");
            max_loan.insert(*t, cap);
        }
    }

    // Executor, simulator, sender.
    let (sim, sender) = match secrets.executor {
        Some(exec) => {
            let owner = ArbExecutor::new(exec, &rpc.http)
                .owner()
                .call()
                .await
                .context("reading executor owner")?;
            let receiver = ArbExecutor::new(exec, &rpc.http)
                .profitReceiver()
                .call()
                .await?;
            info!(%exec, %owner, %receiver, "executor");
            let sender = match (&secrets.private_key, args.mode) {
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
            (Some(Simulator::new(rpc.http.clone(), exec, owner)), sender)
        }
        None => {
            if args.mode == Mode::Live {
                bail!("live mode needs EXECUTOR_ADDRESS");
            }
            warn!("EXECUTOR_ADDRESS not set: shadow mode will log candidates without simulating");
            (None, None)
        }
    };

    let limits = RiskLimits {
        min_profit_usd: cfg.risk.min_profit_usd,
        max_gas_usd_per_day: cfg.risk.max_gas_usd_per_day,
        max_consecutive_failures: cfg.risk.max_consecutive_failures,
    };
    let data_dir = args.data_dir.join(&cfg.name);
    let metrics = Arc::new(Metrics::new(data_dir.clone())?);
    let risk = RiskGuard::new(limits, data_dir.join("STOP"));
    info!(pools = store.len(), kill_switch = %data_dir.join("STOP").display(), "ready");

    let mut ctx = Ctx {
        cfg,
        rpc,
        store,
        decimals,
        max_loan,
        sim,
        sender,
        risk,
        metrics,
        last_sim: HashMap::new(),
        mode: args.mode,
    };
    run(&mut ctx).await
}

async fn run(ctx: &mut Ctx) -> Result<()> {
    let (tx_done, mut rx_done) = mpsc::unbounded_channel::<TxDone>();
    let mut resync = tokio::time::interval(Duration::from_secs(60));
    let mut summary = tokio::time::interval(Duration::from_secs(60));
    loop {
        let mut logs = Box::pin(mev_ingest::pool_logs(&ctx.rpc.ws, ctx.store.addresses()).await?);
        info!("subscribed to pool events");
        loop {
            tokio::select! {
                maybe = logs.next() => {
                    let Some(log) = maybe else { warn!("log stream ended; resubscribing"); break };
                    if let Some(pool) = ctx.store.apply_log(&log) {
                        ctx.metrics.update(|c| c.updates += 1);
                        on_pool_update(ctx, pool, &tx_done).await;
                    }
                }
                Some(done) = rx_done.recv() => on_tx_done(ctx, done),
                _ = resync.tick() => {
                    if let Err(e) = mev_discovery::refresh(&ctx.rpc, &mut ctx.store).await {
                        warn!(error = %e, "resync failed");
                    }
                }
                _ = summary.tick() => {
                    let c = ctx.metrics.snapshot();
                    info!(updates = c.updates, candidates = c.candidates, simulated = c.simulated,
                          profitable = c.sim_profitable, sent = c.sent, ok = c.landed_ok, reverted = c.landed_reverted,
                          profit_usd = format!("{:.4}", c.profit_usd), gas_usd = format!("{:.4}", c.gas_usd),
                          gas_today_usd = format!("{:.4}", ctx.risk.spent_today_usd()), "summary");
                }
                _ = tokio::signal::ctrl_c() => { info!("shutting down"); return Ok(()); }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Err(e) = mev_discovery::refresh(&ctx.rpc, &mut ctx.store).await {
            error!(error = %e, "resync after reconnect failed");
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn on_pool_update(ctx: &mut Ctx, pool: Address, tx_done: &mpsc::UnboundedSender<TxDone>) {
    let usd = ctx.cfg.token(&ctx.cfg.usd_token).unwrap();
    let native = ctx.cfg.token(&ctx.cfg.native_token).unwrap();
    let usd_dec = ctx.decimals[&usd];
    let store = &ctx.store;
    let price = |t: Address| store.usd_per_raw_unit(t, usd, usd_dec, native);
    let params = SearchParams {
        max_loan: &ctx.max_loan,
        usd_per_raw: &price,
        min_est_profit_usd: ctx.cfg.risk.min_profit_usd,
    };
    let opps = two_pool_arbs(store, pool, &params);
    let Some(opp) = opps.into_iter().next() else {
        return;
    };
    let loan_px = price(opp.loan_token).unwrap_or(0.0);
    let native_px = price(native).unwrap_or(0.0);

    ctx.metrics.update(|c| c.candidates += 1);
    let base = json!({
        "loan": ctx.cfg.symbol(opp.loan_token), "amount": opp.amount_in.to_string(),
        "est_profit_usd": opp.est_profit_usd, "buy": opp.buy_pool, "sell": opp.sell_pool,
    });
    let Some(sim) = &ctx.sim else {
        ctx.metrics.event("candidate", base);
        return;
    };

    // Don't re-simulate the same route more than twice a second.
    let key = (opp.buy_pool, opp.sell_pool);
    if ctx
        .last_sim
        .get(&key)
        .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
    {
        return;
    }
    ctx.last_sim.insert(key, Instant::now());
    ctx.metrics.update(|c| c.simulated += 1);

    let outcome = match sim.simulate(&opp, U256::ZERO).await {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "simulation error");
            return;
        }
    };
    let SimOutcome::Profitable { profit, gas } = outcome else {
        if let SimOutcome::Reverted(r) = outcome {
            ctx.metrics
                .event("sim_revert", json!({ "opp": base, "reason": r }));
        }
        return;
    };
    let gas_price = ctx.rpc.http.get_gas_price().await.unwrap_or(10_000_000) as f64;
    let gas_usd = gas as f64 * gas_price * native_px;
    let profit_usd = f64::from(profit) * loan_px;
    let net_usd = profit_usd - gas_usd;
    ctx.metrics.update(|c| c.sim_profitable += 1);
    ctx.metrics.event("sim_ok", json!({ "opp": base, "profit_usd": profit_usd, "gas": gas, "gas_usd": gas_usd, "net_usd": net_usd }));
    info!(
        loan = ctx.cfg.symbol(opp.loan_token),
        profit_usd = format!("{profit_usd:.4}"),
        gas_usd = format!("{gas_usd:.4}"),
        net_usd = format!("{net_usd:.4}"),
        "simulated opportunity"
    );

    if ctx.mode != Mode::Live {
        return;
    }
    let Some(sender) = ctx.sender.clone() else {
        return;
    };
    if let Err(block) = ctx.risk.check(net_usd, gas_usd, now_secs()) {
        ctx.metrics.event(
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
    ctx.risk.mark_sent();
    ctx.metrics.update(|c| c.sent += 1);
    let tx_done = tx_done.clone();
    let metrics = ctx.metrics.clone();
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

fn on_tx_done(ctx: &mut Ctx, done: TxDone) {
    let native = ctx.cfg.token(&ctx.cfg.native_token).unwrap();
    let usd = ctx.cfg.token(&ctx.cfg.usd_token).unwrap();
    let usd_dec = ctx.decimals[&usd];
    let native_px = ctx
        .store
        .usd_per_raw_unit(native, usd, usd_dec, native)
        .unwrap_or(0.0);
    match done {
        TxDone::Landed { hash, opp, landed } => {
            let gas_usd = landed.gas_used as f64 * landed.effective_gas_price as f64 * native_px;
            ctx.risk.record(landed.success, gas_usd, now_secs());
            ctx.metrics.update(|c| {
                c.gas_usd += gas_usd;
                if landed.success {
                    c.landed_ok += 1;
                    c.profit_usd += opp.est_profit_usd;
                } else {
                    c.landed_reverted += 1;
                }
            });
            ctx.metrics.event(
                "landed",
                json!({ "hash": hash, "success": landed.success, "gas_used": landed.gas_used,
                "gas_usd": gas_usd, "block": landed.block, "est_profit_usd": opp.est_profit_usd }),
            );
            info!(%hash, success = landed.success, gas_usd = format!("{gas_usd:.4}"), "landed");
        }
        TxDone::Failed { hash, error } => {
            ctx.risk.record(false, 0.0, now_secs());
            ctx.metrics
                .event("send_failed", json!({ "hash": hash, "error": error }));
            warn!(?hash, %error, "send failed");
        }
    }
}
