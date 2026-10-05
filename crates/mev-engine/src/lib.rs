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
use mev_exec::{Fees, Landed, Sender};
use mev_lending::{AdapterRpc, LiquidationAdapter, Plan};
use mev_liquidate::{Runner, RunnerConfig, RunnerEvent};
use mev_metrics::Metrics;
use mev_risk::{
    RiskGuard, RiskLimits,
    bid::{BidInput, BidPolicy, Decision, TipBook, TipBudget},
    loan_cap,
};
use mev_rpc::{Backoff, Rpc, RpcMeter};
use mev_search::{SearchParams, best_route, two_pool_arbs};
pub use mev_sim::SimOutcome;
use mev_sim::Simulator;
use mev_state::PoolStore;
use mev_types::Opportunity;
use race::Trigger;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Recently observed winning tips kept for the competition estimate (count, max age).
const TIP_BOOK_CAPACITY: usize = 200;
const TIP_BOOK_HORIZON_SECS: u64 = 3600;

/// The bid policy from `[pga]` config.
fn bid_policy(c: &mev_config::PgaConfig) -> BidPolicy {
    let gwei = |g: f64| (g.max(0.0) * 1e9) as u128;
    BidPolicy {
        max_fraction_of_net: c.max_fraction_of_net,
        max_tip_usd_per_tx: c.max_tip_usd_per_tx,
        min_net_usd: 0.0,
        min_tip_wei: gwei(c.min_tip_gwei),
        default_tip_wei: gwei(c.default_tip_gwei),
        outbid_margin: c.outbid_margin,
    }
}

/// A pool claimed by an in-flight arbitrage is free again after this even without a receipt (the
/// sender waits at most 30 s for one).
const BUSY_TTL: Duration = Duration::from_secs(30);

/// How long an arbitrage route is skipped after its send reverted (lost race or mispriced).
const ROUTE_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// Upper bound on indexed triangular cycles (memory and per-update cost stay bounded).
const MAX_CYCLES: usize = 20_000;

mod race;

/// Gas a backrun liquidation is priced at (flash loan + protocol call + swap), and its limit.
/// Unsimulated, so the limit is generous; unused gas isn't charged.
const BACKRUN_GAS: u64 = 900_000;
const BACKRUN_GAS_LIMIT: u64 = 2_000_000;
/// Share of the oracle-price bonus expected to survive swap fees and slippage.
const BACKRUN_BONUS_HAIRCUT: f64 = 0.7;

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
    /// What the arbitrage search may size up to: `max_loan`, or with flash swaps the risk limit
    /// (a V3-first route borrows from its own first pool). Balancer-funded routes above `max_loan`
    /// are dropped (`fundable`).
    search_loan: HashMap<Address, f64>,
    sim: Option<Simulator>,
    sender: Option<Arc<Sender>>,
    risk: RiskGuard,
    metrics: Arc<Metrics>,
    last_sim: HashMap<(Address, Address), Instant>,
    /// Arbitrage routes whose send reverted, and when (skipped for `ROUTE_COOLDOWN`).
    cooldown: HashMap<(Address, Address), Instant>,
    /// UTC day (days since epoch) on which a daily cap switched all sending / arbitrage sending
    /// to shadow; sending resumes when the day rolls over and the caps reset.
    halt_all_day: Option<u64>,
    halt_arb_day: Option<u64>,
    /// Pools touched by in-flight arbitrages (claimed at send) and each route's pools.
    busy_pools: HashMap<Address, Instant>,
    busy_routes: HashMap<(Address, Address), Vec<Address>>,
    /// PGA priority-fee bidding: policy, recently observed winning tips, tip spend caps.
    bid_policy: BidPolicy,
    /// Fast Feed counters when the paid feed is enabled and keyed.
    fast_feed_stats: Option<Arc<mev_ingest::fast_feed::FastFeedStats>>,
    tips: TipBook,
    tip_budget: TipBudget,
    mode: Mode,
    liq: Option<LiqCtx>,
    presim_rx: Option<PresimRx>,
    /// Recent feed arrivals by tx hash (race telemetry clock).
    feed_log: Arc<std::sync::Mutex<mev_sequencer::FeedLog>>,
    presim_stats: Arc<mev_presim::PresimStats>,
    localsim_stats: Arc<mev_localsim::worker::LocalStats>,
    /// Pools + tokens a feed tx must mention to be pre-simulated (refreshed on resync).
    presim_watch: mev_presim::Watchlist,
    /// Request meters per provider ("fast" = paid), drained into the minute summary.
    meters: Vec<(&'static str, Arc<RpcMeter>)>,
    /// (fetched at, wei); refreshed in the background so hot paths never wait on it.
    gas_price: Arc<std::sync::Mutex<Option<(Instant, f64)>>>,
    gas_refreshing: Arc<std::sync::atomic::AtomicBool>,
    /// Tokens we looked up swap routes for on demand (and when): at most one lookup per hour.
    route_lookups: HashMap<Address, Instant>,
    /// Triangular cycles over watched pools, indexed by pool (rebuilt on resync).
    cycles: mev_search::cycles::CycleIndex,
}

/// Liquidation side of the engine (the runner and its adapters run as their own task).
/// Pool logs from simulated feed txs (feed-driven arbitrage).
type PresimRx = mpsc::UnboundedReceiver<mev_presim::Presimulated>;

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
        /// Arbitrage route (buy, sell pool): cooled down if it reverted.
        route: Option<(Address, Address)>,
    },
    Failed {
        hash: Option<TxHash>,
        error: String,
    },
    /// A priority fee (wei/gas) seen winning ordering over us: the competition estimate's input.
    TipSeen(u128),
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
        let rpc = Rpc::connect(&secrets.http_urls(), &secrets.ws_urls()).await?;
        let usd = cfg.token(&cfg.usd_token)?;
        let native = cfg.token(&cfg.native_token)?;

        let data_dir = data_dir.join(&cfg.name);
        let mut pairs = mev_discovery::candidate_pairs(&rpc, &cfg).await?;

        // Liquidation adapters; their assets need swap routes (seized collateral -> debt asset).
        let mut meters = vec![("fast", rpc.meter.clone())];
        let liq_setup = build_liquidation(
            &cfg,
            &rpc,
            &secrets.sweep_urls,
            &data_dir,
            secrets.executor.unwrap_or_default(),
            &mut meters,
        )
        .await?;
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
        let search_loan: HashMap<Address, f64> = if cfg.executor_flash_swaps {
            loan_tokens
                .iter()
                .filter_map(|t| {
                    let px = store.usd_per_raw_unit(*t, usd, usd_decimals, native)?;
                    (px > 0.0).then(|| (*t, cfg.risk.max_loan_usd / px))
                })
                .collect()
        } else {
            max_loan.clone()
        };

        let (sim, sender) = wire_executor(&cfg, &secrets, &rpc, mode).await?;

        let limits = RiskLimits {
            min_profit_usd: cfg.risk.min_profit_usd,
            max_gas_usd_per_day: cfg.risk.max_gas_usd_per_day,
            max_consecutive_failures: cfg.risk.max_consecutive_failures,
            max_arb_gas_usd_per_day: cfg
                .risk
                .max_arb_gas_usd_per_day
                .unwrap_or(cfg.risk.max_gas_usd_per_day * 2.0 / 3.0),
        };
        let metrics = Arc::new(Metrics::new(data_dir.clone())?);
        metrics.set_live(mode == Mode::Live);
        let risk = RiskGuard::new(limits, data_dir.join("STOP"))
            .with_max_in_flight(cfg.risk.max_concurrent_sends);
        info!(pools = store.len(), kill_switch = %data_dir.join("STOP").display(), "ready");

        // Sequencer feed, shared by the liquidation runner (price-feed triggers, backruns) and the
        // pre-simulator (feed-driven arbitrage).
        let presim_cfg = cfg
            .presim
            .clone()
            .filter(|p| p.enabled && !p.rpc_urls.is_empty());
        let feed_log: Arc<std::sync::Mutex<mev_sequencer::FeedLog>> = Default::default();
        let fast_feed_stats = (cfg.pga.fast_feed.enabled && secrets.fast_feed_key.is_some())
            .then(|| Arc::new(mev_ingest::fast_feed::FastFeedStats::default()));
        if cfg.pga.fast_feed.enabled && secrets.fast_feed_key.is_none() {
            warn!(
                "pga.fast_feed enabled but ARBITRUM_FAST_FEED_KEY is not set: using the public feed only"
            );
        }
        let (liq_feed, presim_feed) = match cfg.sequencer_feed.clone() {
            Some(url) => {
                let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<mev_sequencer::SequencedTx>();
                // Optional paid Fast Feed: same txs, earlier (inside the PGA round). Merged into
                // the same stream; the fan-out below drops whichever copy arrives second.
                if let Some(stats) = &fast_feed_stats {
                    spawn_fast_feed(
                        cfg.pga.fast_feed.url.clone(),
                        secrets.fast_feed_key.clone().unwrap_or_default(),
                        raw_tx.clone(),
                        stats.clone(),
                    );
                }
                tokio::spawn(mev_sequencer::run(url, raw_tx));
                let (liq_tx, liq_rx) = mpsc::unbounded_channel();
                let (pre_tx, pre_rx) = mpsc::unbounded_channel();
                let (want_liq, want_pre) = (liq_setup.is_some(), presim_cfg.is_some());
                let log = feed_log.clone();
                tokio::spawn(async move {
                    let mut seen = RecentHashes::new(FEED_DEDUP);
                    while let Some(st) = raw_rx.recv().await {
                        if !seen.insert(st.hash) {
                            continue; // already delivered by the other feed
                        }
                        log.lock().unwrap().record(&st);
                        if want_pre {
                            let _ = pre_tx.send(st.clone());
                        }
                        if want_liq {
                            let _ = liq_tx.send(st);
                        }
                    }
                });
                (Some(liq_rx), Some(pre_rx))
            }
            None => (None, None),
        };
        let presim_stats = Arc::new(mev_presim::PresimStats::default());
        let presim_watch: mev_presim::Watchlist = Default::default();
        *presim_watch.write().unwrap() = watchlist(&store);
        let localsim_stats = Arc::new(mev_localsim::worker::LocalStats::default());
        let presim_rx = match (presim_cfg, presim_feed) {
            (Some(pc), Some(feed)) => {
                // Keyed endpoints (from .env) first, then the public ones.
                let presim_urls: Vec<String> = secrets
                    .presim_urls
                    .iter()
                    .chain(&pc.rpc_urls)
                    .cloned()
                    .collect();
                let p = metered_rotating(presim_urls.clone(), "presim", &mut meters)?;
                let (tx, rx) = mpsc::unbounded_channel();
                // Local mode: the worker simulates the feed in-process and passes only candidates
                // it can't run yet to the RPC pre-simulator.
                let rpc_feed = if pc.local && !pc.diff_rpc_urls.is_empty() {
                    let (fb_tx, fb_rx) = mpsc::unbounded_channel();
                    let endpoints = mev_localsim::worker::Endpoints {
                        diff: metered_rotating(
                            pc.diff_rpc_urls.clone(),
                            "localsim-diff",
                            &mut meters,
                        )?,
                        fill: metered_rotating(
                            if pc.fill_rpc_urls.is_empty() {
                                pc.diff_rpc_urls.clone()
                            } else {
                                pc.fill_rpc_urls.clone()
                            },
                            "localsim-fill",
                            &mut meters,
                        )?,
                        head: p.clone(),
                    };
                    tokio::spawn(mev_localsim::worker::run(
                        feed,
                        endpoints,
                        presim_watch.clone(),
                        tx.clone(),
                        fb_tx,
                        localsim_stats.clone(),
                    ));
                    info!("local feed simulation on (revm, diff-fed state)");
                    fb_rx
                } else {
                    feed
                };
                tokio::spawn(mev_presim::run(
                    rpc_feed,
                    p,
                    pc.max_in_flight,
                    Duration::from_millis(pc.timeout_ms),
                    tx,
                    presim_stats.clone(),
                    presim_watch.clone(),
                ));
                info!(
                    rpcs = presim_urls.len(),
                    max_in_flight = pc.max_in_flight,
                    "feed pre-simulation on"
                );
                Some(rx)
            }
            _ => None,
        };
        let liq = match liq_setup {
            Some((runner, via)) => {
                if let Some(exec) = secrets.executor {
                    check_allowlist(&rpc, exec, &runner.targets()).await;
                }
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(runner.run(liq_feed, tx));
                Some(LiqCtx { via, rx: Some(rx) })
            }
            None => None,
        };

        let pga_policy = bid_policy(&cfg.pga);
        let pga_budget =
            TipBudget::new(cfg.pga.max_tip_usd_per_minute, cfg.pga.max_tip_usd_per_day);
        Ok(Self {
            cfg,
            rpc,
            store,
            usd,
            native,
            usd_decimals,
            max_loan,
            search_loan,
            sim,
            sender,
            risk,
            metrics,
            last_sim: HashMap::new(),
            cooldown: HashMap::new(),
            halt_all_day: None,
            halt_arb_day: None,
            busy_pools: HashMap::new(),
            busy_routes: HashMap::new(),
            bid_policy: pga_policy,
            fast_feed_stats,
            tips: TipBook::new(TIP_BOOK_CAPACITY, TIP_BOOK_HORIZON_SECS),
            tip_budget: pga_budget,
            mode,
            liq,
            presim_rx,
            feed_log,
            presim_stats,
            localsim_stats,
            presim_watch,
            meters,
            gas_price: Default::default(),
            gas_refreshing: Default::default(),
            route_lookups: HashMap::new(),
            cycles: Default::default(),
        })
    }

    /// Run until Ctrl-C. Resubscribes and resyncs if the event stream drops.
    pub async fn run(&mut self) -> Result<()> {
        let (tx_done, mut rx_done) = mpsc::unbounded_channel::<TxDone>();
        let mut resync = tokio::time::interval(Duration::from_secs(60));
        let mut summary = tokio::time::interval(Duration::from_secs(60));
        let mut backoff = Backoff::default();
        self.rebuild_cycles();
        info!(cycles = self.cycles.len(), "triangular cycles indexed");
        let mut liq_rx = self.liq.as_mut().and_then(|l| l.rx.take());
        let mut presim_rx = self.presim_rx.take();
        // Created once, outside the loops: a signal arriving while a handler runs is not lost.
        // SIGTERM is what `podman stop` sends.
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let shutdown = async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
        };
        tokio::pin!(shutdown);
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
                            let trigger = Trigger { tx: log.transaction_hash, source: "block", seen: Instant::now() };
                            self.on_pool_update(pool, trigger, &tx_done).await;
                        }
                    }
                    Some(pre) = recv_opt(&mut presim_rx) => {
                        // Same path as block logs, a block early; the real logs overwrite it.
                        for log in &pre.logs {
                            if let Some(pool) = self.store.apply_log(log) {
                                self.metrics.update(|c| c.presim_updates += 1);
                                let trigger = Trigger { tx: Some(pre.tx), source: "presim", seen: pre.received };
                                self.on_pool_update(pool, trigger, &tx_done).await;
                            }
                        }
                    }
                    Some(done) = rx_done.recv() => self.on_tx_done(done),
                    Some(ev) = recv_opt(&mut liq_rx) => self.on_runner_event(ev, &tx_done).await,
                    _ = resync.tick() => {
                        if let Err(e) = mev_discovery::refresh(&self.rpc, &mut self.store).await {
                            warn!(error = %e, "resync failed");
                        }
                        *self.presim_watch.write().unwrap() = watchlist(&self.store);
                        self.rebuild_cycles();
                    }
                    _ = summary.tick() => self.log_summary(),
                    _ = &mut shutdown => { info!("shutting down"); return Ok(()); }
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

    /// Swap route for seized collateral. If none is known, look the token's pools up once (per
    /// hour) through the DEX factories: markets found after startup (Morpho grows its index
    /// continuously) otherwise never get routes. New pools are refreshed by the periodic resync.
    async fn route(&mut self, plan: &Plan, via: &[Address]) -> Option<Vec<mev_types::Hop>> {
        let (col, debt, amt) = (
            plan.collateral_asset,
            plan.debt_asset,
            plan.expected_collateral,
        );
        if let Some((h, _)) = best_route(&self.store, col, debt, amt, via) {
            return Some(h);
        }
        if self
            .route_lookups
            .get(&col)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
        {
            return None;
        }
        self.route_lookups.insert(col, Instant::now());
        let pairs: Vec<(Address, Address)> = std::iter::once((col, debt))
            .chain(via.iter().map(|v| (col, *v)))
            .filter(|(a, b)| a != b)
            .collect();
        let found = async {
            let infos = mev_discovery::discover_pools(&self.rpc, &self.cfg, &pairs).await?;
            mev_discovery::load_pools(&self.rpc, infos).await
        }
        .await;
        match found {
            Ok(pools) => {
                let n = pools.len();
                for p in pools {
                    self.store.insert(p);
                }
                info!(token = %col, pools = n, "on-demand route discovery");
            }
            Err(e) => {
                warn!(token = %col, error = %format!("{e:#}"), "on-demand route discovery failed")
            }
        }
        best_route(&self.store, col, debt, amt, via).map(|(h, _)| h)
    }

    /// L2 gas price, cached for 10 s (simulations come in bursts; each fetch is a paid call).
    /// Gas price (wei). Returns the cached value at once; when it is older than 10 s a background
    /// refresh starts. Only the very first call waits for the RPC.
    async fn gas_price(&mut self) -> f64 {
        use std::sync::atomic::Ordering;
        let cached = *self.gas_price.lock().unwrap();
        let Some((at, wei)) = cached else {
            let wei = fetch_gas_price(&self.rpc.http).await;
            *self.gas_price.lock().unwrap() = Some((Instant::now(), wei));
            return wei;
        };
        if at.elapsed() >= Duration::from_secs(10)
            && !self.gas_refreshing.swap(true, Ordering::SeqCst)
        {
            let (http, cache, busy) = (
                self.rpc.http.clone(),
                self.gas_price.clone(),
                self.gas_refreshing.clone(),
            );
            tokio::spawn(async move {
                let wei = fetch_gas_price(&http).await;
                *cache.lock().unwrap() = Some((Instant::now(), wei));
                busy.store(false, Ordering::SeqCst);
            });
        }
        wei
    }

    /// Is any of `pools` used by an in-flight arbitrage (claims expire after `BUSY_TTL`)?
    fn pools_busy(&self, pools: &[Address]) -> bool {
        pools.iter().any(|p| {
            self.busy_pools
                .get(p)
                .is_some_and(|t| t.elapsed() < BUSY_TTL)
        })
    }

    fn claim_pools(&mut self, route: (Address, Address), pools: &[Address]) {
        let now = Instant::now();
        self.busy_pools.retain(|_, t| t.elapsed() < BUSY_TTL);
        for p in pools {
            self.busy_pools.insert(*p, now);
        }
        self.busy_routes.insert(route, pools.to_vec());
    }

    /// The arbitrage on `route` landed: its pools are free again.
    fn release_pools(&mut self, route: (Address, Address)) {
        if let Some(ps) = self.busy_routes.remove(&route) {
            for p in ps {
                self.busy_pools.remove(&p);
            }
        }
    }

    /// May real transactions be sent now? Live mode, unless a daily cap switched sending to
    /// shadow for today (`arb`: arbitrage; a full halt also covers liquidations). Stamps the
    /// resulting mode on the event log and records every switch.
    fn sending(&self, arb: bool) -> bool {
        let day = now_secs() / 86_400;
        let live = self.mode == Mode::Live
            && self.halt_all_day != Some(day)
            && !(arb && self.halt_arb_day == Some(day));
        let overall = self.mode == Mode::Live && self.halt_all_day != Some(day);
        if self.metrics.set_live(overall) {
            let to = if overall { "live" } else { "shadow" };
            self.metrics.event("mode_switch", json!({ "to": to }));
        }
        live
    }

    /// A risk check refused a send: a failure-streak pause or the daily gas cap switches to
    /// shadow for the rest of the UTC day (arbitrage alone for its own gas sub-budget).
    fn on_cap(&mut self, block: &mev_risk::Block, arb: bool) {
        use mev_risk::Block;
        match block {
            Block::Paused { .. } => self.halt(true, format!("{block:?}")),
            Block::DailyGasCap { .. } => self.halt(!arb, format!("{block:?}")),
            _ => {}
        }
    }

    /// Switch to shadow until the next UTC day: all sending, or arbitrage only.
    fn halt(&mut self, all: bool, reason: String) {
        let day = now_secs() / 86_400;
        let slot = if all {
            &mut self.halt_all_day
        } else {
            &mut self.halt_arb_day
        };
        if *slot == Some(day) {
            return;
        }
        *slot = Some(day);
        warn!(all, %reason, "cap reached: switching to shadow until the next UTC day");
        self.metrics.event(
            "cap_reached",
            json!({ "scope": if all { "all" } else { "arb" }, "reason": reason }),
        );
        let _ = self.sending(true);
    }

    /// Did a send on `o`'s route revert within `ROUTE_COOLDOWN`?
    fn cooling(&self, o: &Opportunity) -> bool {
        self.cooldown
            .get(&(o.buy_pool, o.sell_pool))
            .is_some_and(|t| t.elapsed() < ROUTE_COOLDOWN)
    }

    /// The PGA bid for a send happening now: fee fields and the tip's USD cost, or `None` (logged)
    /// when the policy or the tip spend caps say skip. `gas_price` is the current base fee (wei),
    /// `native_px` USD per wei; `gross_usd` is expected profit before any gas.
    fn pga_bid(
        &mut self,
        gross_usd: f64,
        gas: u64,
        gas_price: f64,
        native_px: f64,
        ev: &serde_json::Value,
    ) -> Option<(Fees, f64)> {
        let base_fee = gas_price.max(0.0) as u128;
        if !self.cfg.pga.enabled {
            return Some((Fees::no_bid(base_fee), 0.0));
        }
        let now = now_secs();
        let competition = self.tips.estimate(self.cfg.pga.competition_quantile, now);
        let input = BidInput {
            gross_profit_usd: gross_usd,
            gas_units: gas,
            base_fee_wei: base_fee,
            usd_per_wei: native_px,
            competition_tip_wei: competition,
        };
        let bid = match self.bid_policy.decide(&input) {
            Decision::Bid(b) => b,
            Decision::Skip(why) => {
                self.metrics.update(|c| c.bids_skipped += 1);
                self.metrics.event(
                    "bid_skip",
                    json!({ "opp": ev, "reason": format!("{why:?}"), "competition_wei": competition.map(|c| c.to_string()) }),
                );
                return None;
            }
        };
        if let Err(cap) = self.tip_budget.allow(bid.tip_cost_usd, now) {
            if matches!(cap, mev_risk::bid::TipCap::PerDay { .. }) && self.mode == Mode::Live {
                self.halt(true, format!("{cap:?}"));
            }
            self.metrics.update(|c| c.bids_skipped += 1);
            self.metrics.event(
                "bid_skip",
                json!({ "opp": ev, "reason": format!("{cap:?}") }),
            );
            return None;
        }
        self.tip_budget.record(bid.tip_cost_usd, now);
        self.metrics.update(|c| {
            c.bids_placed += 1;
            c.tip_usd += bid.tip_cost_usd;
            c.bid_fraction_sum += bid.fraction_of_net;
        });
        self.metrics.event(
            "bid",
            json!({ "opp": ev, "tip_wei": bid.max_priority_fee_per_gas.to_string(), "tip_usd": bid.tip_cost_usd,
                    "fraction_of_net": bid.fraction_of_net, "competition_wei": competition.map(|c| c.to_string()) }),
        );
        Some((Fees::from_bid(&bid), bid.tip_cost_usd))
    }

    /// The Aave pool to flash-borrow from when the executor supports `liquidateViaAave`.
    fn aave_flash_pool(&self) -> Option<Address> {
        self.cfg
            .executor_aave_flash
            .then(|| self.cfg.aave.as_ref().map(|a| a.pool))
            .flatten()
    }

    /// Can the executor fund `o`: a flash swap (no lender) or a Balancer loan within its cap.
    fn fundable(&self, o: &Opportunity) -> bool {
        (self.cfg.executor_flash_swaps && mev_bindings::flash_swappable(o))
            || self
                .max_loan
                .get(&o.loan_token)
                .is_some_and(|cap| f64::from(o.amount_in) <= *cap)
    }

    /// Re-enumerate triangular cycles over the current pool set.
    fn rebuild_cycles(&mut self) {
        let loans: Vec<Address> = self.search_loan.keys().copied().collect();
        self.cycles = mev_search::cycles::CycleIndex::build(&self.store, &loans, MAX_CYCLES);
    }

    fn price(&self, token: Address) -> Option<f64> {
        self.store
            .usd_per_raw_unit(token, self.usd, self.usd_decimals, self.native)
    }

    fn log_summary(&self) {
        self.metrics.heartbeat();
        if let Some(ff) = &self.fast_feed_stats {
            use std::sync::atomic::Ordering::Relaxed;
            let msgs = ff.msgs.swap(0, Relaxed);
            let (gaps, missing) = (ff.gaps.swap(0, Relaxed), ff.missing.swap(0, Relaxed));
            let lag = ff
                .lag_ms_total
                .swap(0, Relaxed)
                .checked_div(msgs)
                .unwrap_or(0);
            let reconnects = ff.reconnects.load(Relaxed);
            self.metrics.update(|c| {
                c.fast_feed_msgs += msgs;
                c.fast_feed_gaps += gaps;
                c.fast_feed_reconnects = reconnects;
            });
            info!(
                msgs,
                gaps,
                missing,
                reconnects,
                mean_lag_ms = lag,
                "fast feed (last minute)"
            );
        }
        if self.presim_rx.is_some() || self.cfg.presim.as_ref().is_some_and(|p| p.enabled) {
            info!(stats = %self.presim_stats.take_summary(), "presim (last minute)");
            if self.cfg.presim.as_ref().is_some_and(|p| p.local) {
                info!(stats = %self.localsim_stats.take_summary(), "localsim (last minute)");
            }
        }
        // RPC usage since the last summary (≈ per minute): the data-efficiency scoreboard.
        let mut usage = serde_json::Map::new();
        for (name, m) in &self.meters {
            let by_method = m.drain();
            let total: u64 = by_method.iter().map(|(_, n)| n).sum();
            let top: Vec<String> = by_method
                .iter()
                .take(4)
                .map(|(k, n)| format!("{k}={n}"))
                .collect();
            info!(
                provider = name,
                calls = total,
                top = top.join(" "),
                "rpc usage (last minute)"
            );
            usage.insert(
                (*name).to_string(),
                json!({ "calls": total, "by_method": by_method }),
            );
        }
        self.metrics
            .event("rpc_usage", serde_json::Value::Object(usage));
        let c = self.metrics.snapshot();
        info!(
            updates = c.updates,
            presim_updates = c.presim_updates,
            candidates = c.candidates,
            simulated = c.simulated,
            profitable = c.sim_profitable,
            liq_planned = c.liq_planned,
            liq_profitable = c.liq_profitable,
            bids = c.bids_placed,
            bids_skipped = c.bids_skipped,
            bids_won = c.bids_won,
            bids_lost = c.bids_lost,
            tip_usd = format!("{:.4}", c.tip_usd),
            round_within = c.round_within,
            round_over = c.round_over,
            feed_triggers = c.feed_triggers,
            sent = c.sent,
            ok = c.landed_ok,
            reverted = c.landed_reverted,
            profit_usd = format!("{:.4}", c.profit_usd),
            gas_usd = format!("{:.4}", c.gas_usd),
            gas_today_usd = format!("{:.4}", self.risk.spent_today_usd()),
            "summary"
        );
    }

    async fn on_pool_update(
        &mut self,
        pool: Address,
        trigger: Trigger,
        tx_done: &mpsc::UnboundedSender<TxDone>,
    ) {
        let price = |t: Address| self.price(t);
        let params = SearchParams {
            max_loan: &self.search_loan,
            usd_per_raw: &price,
            min_est_profit_usd: self.cfg.risk.min_profit_usd,
        };
        // Best fundable two-pool and triangular candidates touching this pool.
        let fundable = |o: &Opportunity| self.fundable(o) && !self.cooling(o);
        let two = two_pool_arbs(&self.store, pool, &params)
            .into_iter()
            .find(fundable);
        let tri = self
            .cycles
            .arbs(&self.store, pool, &params)
            .into_iter()
            .find(fundable);
        let Some(opp) = [two, tri]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.est_profit_usd.total_cmp(&b.est_profit_usd))
        else {
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

        // Don't re-try the same route more than twice a second.
        let key = (opp.buy_pool, opp.sell_pool);
        if self
            .last_sim
            .get(&key)
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_sim.insert(key, Instant::now());

        // Live without pre-simulation: trust the local model and save the eth_call round trip
        // (the race is decided in ~100s of ms). Shadow always simulates to keep measuring it.
        // Live sends on the model only when it is exact for this route (no V3 tick crossed);
        // otherwise the executor eth_call confirms it first (a crossed tick can mean less output).
        let presimulate = !self.sending(true) || self.cfg.risk.arb_presimulate || !opp.exact;
        let (profit_usd, gas) = if presimulate {
            self.metrics.update(|c| c.simulated += 1);
            let outcome = match sim.simulate_fast(&opp, U256::ZERO).await {
                Ok(o) => o,
                Err(e) => {
                    warn!(error = %e, "simulation error");
                    return;
                }
            };
            match outcome {
                SimOutcome::Profitable { profit, gas } => (f64::from(profit) * loan_px, gas),
                SimOutcome::Reverted(r) => {
                    self.metrics
                        .event("sim_revert", json!({ "opp": base, "reason": r }));
                    return;
                }
            }
        } else {
            (
                opp.est_profit_usd,
                mev_sim::arb_gas_estimate(opp.hops.len()),
            )
        };
        let gas_price = self.gas_price().await;
        let gas_usd = gas as f64 * gas_price * native_px;
        let net_usd = profit_usd - gas_usd;
        self.metrics.update(|c| c.sim_profitable += 1);
        self.metrics.event(
            if presimulate { "sim_ok" } else { "model_ok" },
            json!({ "opp": base, "profit_usd": profit_usd, "gas": gas, "gas_usd": gas_usd, "net_usd": net_usd }),
        );
        info!(
            loan = self.cfg.symbol(opp.loan_token),
            profit_usd = format!("{profit_usd:.4}"),
            gas_usd = format!("{gas_usd:.4}"),
            net_usd = format!("{net_usd:.4}"),
            "arb opportunity"
        );

        if !self.sending(true) {
            // Shadow: log the PGA bid this would have placed (nothing is sent).
            let _ = self.pga_bid(profit_usd, gas, gas_price, native_px, &base);
            return;
        }
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if let Err(block) = self.risk.check_arb(net_usd, gas_usd, now_secs()) {
            self.on_cap(&block, true);
            self.metrics.event(
                "risk_block",
                json!({ "opp": base, "reason": format!("{block:?}") }),
            );
            return;
        }
        // Concurrent sends must not touch the same pool (they would race each other).
        let route_pools: Vec<Address> = opp.hops.iter().map(|h| h.pool).collect();
        if self.pools_busy(&route_pools) {
            self.metrics.event("pool_busy", json!({ "opp": base }));
            return;
        }
        // PGA bid, decided now (fresh base fee and competition), not at detection.
        let Some((fees, tip_usd)) = self.pga_bid(profit_usd, gas, gas_price, native_px, &base)
        else {
            return;
        };
        // On-chain floor: the trade must pay for its gas and its bid, or it reverts.
        let min_profit = if loan_px > 0.0 {
            U256::from(((gas_usd + tip_usd) / loan_px) as u128)
        } else {
            U256::ZERO
        };
        // `gas` is a calibrated estimate (no estimate_gas round-trip); leave headroom for heavier
        // hops (Algebra pools use ~2x a Uniswap V3 hop).
        let gas_limit = gas * 5 / 2;
        self.risk.mark_sent();
        self.metrics.update(|c| c.sent += 1);
        let tx_done = tx_done.clone();
        let metrics = self.metrics.clone();
        let (http, feed_log) = (self.rpc.http.clone(), self.feed_log.clone());
        let pools = route_pools;
        self.claim_pools((opp.buy_pool, opp.sell_pool), &pools);
        let decided = Instant::now();
        let round_ms = self.cfg.pga.round_ms;
        tokio::spawn(async move {
            let sent = match sender.send(&opp, min_profit, gas_limit, Some(fees)).await {
                Ok(sent) => sent,
                Err(e) => {
                    let _ = tx_done.send(TxDone::Failed {
                        hash: None,
                        error: e.to_string(),
                    });
                    return;
                }
            };
            let hash = sent.hash;
            let endpoints: Vec<String> = sent
                .endpoints
                .iter()
                .map(|(l, ms, ok)| format!("{l}={ms}ms{}", if *ok { "" } else { "(rejected)" }))
                .collect();
            // Trigger seen -> first endpoint accepted (the latency that decides races).
            let latency_ms = race::ms(trigger.seen, sent.first_accept.unwrap_or(sent.signed));
            metrics.event(
                "sent",
                json!({ "hash": hash, "net_usd": net_usd, "latency_ms": latency_ms, "source": trigger.source, "endpoints": endpoints }),
            );
            info!(%hash, latency_ms, source = trigger.source, endpoints = endpoints.join(" "), "arb sent (trigger -> accepted)");
            // Inside the PGA round its trigger was in? (Later = bidding in a later round.)
            metrics.update(|c| {
                if latency_ms <= round_ms as i64 {
                    c.round_within += 1
                } else {
                    c.round_over += 1
                }
            });
            let landed = match sender.wait(hash, Duration::from_secs(30)).await {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx_done.send(TxDone::Failed {
                        hash: Some(hash),
                        error: e.to_string(),
                    });
                    return;
                }
            };
            let _ = tx_done.send(TxDone::Landed {
                hash,
                kind: "arb",
                est_profit_usd: opp.est_profit_usd,
                landed: landed.clone(),
                route: Some((opp.buy_pool, opp.sell_pool)),
            });
            // Off the hot path: who beat us, and by how much.
            let rep = race::report(
                &http,
                &feed_log,
                trigger,
                decided,
                sent,
                &landed,
                &pools,
                opp.est_profit_usd,
            )
            .await;
            info!(
                %hash,
                won = landed.success,
                behind_winner_ms = ?rep["behind_winner_ms"].as_i64(),
                winner_index = ?rep["winner_index"].as_u64(),
                our_index = ?rep["our_index"].as_u64(),
                "race"
            );
            if let Some(tip) = rep["winner_tip_wei"]
                .as_str()
                .and_then(|t| t.parse::<u128>().ok())
            {
                let _ = tx_done.send(TxDone::TipSeen(tip));
            }
            metrics.event("race", rep);
        });
    }

    async fn on_runner_event(&mut self, ev: RunnerEvent, tx_done: &mpsc::UnboundedSender<TxDone>) {
        match ev {
            RunnerEvent::Plan(p) => self.on_plan(p, tx_done).await,
            RunnerEvent::Backrun { plan, lead } => self.on_backrun(plan, lead, tx_done).await,
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
    /// Checks shared by simulated and backrun liquidations: size sanity, swap route, lender
    /// capacity. Logs the reason and returns None when the plan can't be sent.
    /// Also picks the funding: Balancer within its cap, otherwise the Aave pool (`Some(pool)`)
    /// when the executor supports it.
    async fn liq_precheck(
        &mut self,
        plan: &Plan,
    ) -> Option<(Vec<mev_types::Hop>, serde_json::Value, Option<Address>)> {
        let via = self.liq.as_ref()?.via.clone();
        let base = json!({ "protocol": plan.protocol, "account": plan.account.to_string(),
            "collateral": self.cfg.symbol(plan.collateral_asset), "debt": self.cfg.symbol(plan.debt_asset),
            "bonus_usd": plan.bonus_usd });
        if plan.bonus_usd < self.cfg.risk.min_profit_usd {
            self.metrics.event("liq_dust", base);
            return None;
        }
        if plan.bonus_usd > self.cfg.risk.max_bonus_usd {
            // Seen live: bad-debt Morpho markets whose oracles still price dead collateral.
            self.metrics.event("liq_implausible", base);
            return None;
        }
        let hops = if plan.collateral_asset == plan.debt_asset {
            vec![]
        } else {
            match self.route(plan, &via).await {
                Some(h) => h,
                None => {
                    self.metrics.event("liq_no_route", base);
                    return None;
                }
            }
        };
        let fits_balancer = self
            .max_loan
            .get(&plan.debt_asset)
            .is_some_and(|cap| f64::from(plan.flash_amount) <= *cap);
        if fits_balancer {
            return Some((hops, base, None));
        }
        // Too large for Balancer (or not held there): Aave's own pool lends its reserves deeply.
        if let Some(pool) = self.aave_flash_pool() {
            return Some((hops, base, Some(pool)));
        }
        let why = if self.max_loan.contains_key(&plan.debt_asset) {
            "liq_too_large"
        } else {
            "liq_no_lender"
        };
        self.metrics.event(why, base);
        None
    }

    /// A liquidation predicted from a price update not yet in a block: send at once, without
    /// simulation (the chain doesn't have the price yet). The executor's on-chain `minProfit`
    /// check (>= gas) is the guard, so a wrong prediction costs only gas.
    async fn on_backrun(
        &mut self,
        plan: Plan,
        lead: Duration,
        tx_done: &mpsc::UnboundedSender<TxDone>,
    ) {
        self.metrics.update(|c| c.liq_planned += 1);
        let Some((hops, base, aave_pool)) = self.liq_precheck(&plan).await else {
            return;
        };
        let Some(debt_px) = self.price(plan.debt_asset) else {
            return self.metrics.event("liq_no_price", base);
        };
        let native_px = self.price(self.native).unwrap_or(0.0);
        let gas_price = self.gas_price().await;
        let gas_usd = BACKRUN_GAS as f64 * gas_price * native_px;
        // Unsimulated: discount the oracle-price bonus for swap fees and slippage.
        let net_usd = plan.bonus_usd * BACKRUN_BONUS_HAIRCUT - gas_usd;
        let lead_ms = lead.as_millis() as u64;
        let ev = json!({ "liq": base, "net_usd": net_usd, "gas_usd": gas_usd, "lead_ms": lead_ms });
        if net_usd <= 0.0 {
            return self.metrics.event("liq_backrun_unprofitable", ev);
        }
        self.metrics.update(|c| c.liq_profitable += 1);
        self.metrics.event("liq_backrun", ev.clone());
        if !self.sending(false) {
            // Shadow: log the PGA bid this would have placed (nothing is sent).
            let gross = plan.bonus_usd * BACKRUN_BONUS_HAIRCUT;
            let _ = self.pga_bid(gross, BACKRUN_GAS, gas_price, native_px, &ev);
            return;
        }
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if let Err(block) = self.risk.check_concurrent(net_usd, gas_usd, now_secs()) {
            self.on_cap(&block, false);
            return self.metrics.event(
                "risk_block",
                json!({ "liq": ev, "reason": format!("{block:?}") }),
            );
        }
        let gross = plan.bonus_usd * BACKRUN_BONUS_HAIRCUT;
        let Some((fees, tip_usd)) = self.pga_bid(gross, BACKRUN_GAS, gas_price, native_px, &ev)
        else {
            return;
        };
        let min_profit = if debt_px > 0.0 {
            U256::from(((gas_usd + tip_usd) / debt_px) as u128)
        } else {
            U256::ZERO
        };
        self.risk.mark_sent();
        self.metrics.update(|c| c.sent += 1);
        let tx_done = tx_done.clone();
        let metrics = self.metrics.clone();
        tokio::spawn(async move {
            let sent = sender
                .send_liquidation_fast(&plan, &hops, min_profit, BACKRUN_GAS_LIMIT, fees, aave_pool)
                .await;
            let done = match sent.map(|s| s.hash) {
                Ok(hash) => {
                    metrics.event(
                        "sent",
                        json!({ "hash": hash, "kind": "liquidation_backrun", "net_usd": net_usd, "lead_ms": lead_ms }),
                    );
                    info!(%hash, protocol = plan.protocol, account = %plan.account, lead_ms, "sent liquidation backrun");
                    match sender.wait(hash, Duration::from_secs(30)).await {
                        Ok(landed) => TxDone::Landed {
                            hash,
                            kind: "liquidation_backrun",
                            est_profit_usd: net_usd,
                            landed,
                            route: None,
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

    async fn on_plan(&mut self, plan: Plan, tx_done: &mpsc::UnboundedSender<TxDone>) {
        if self.liq.is_none() {
            return;
        }
        self.metrics.update(|c| c.liq_planned += 1);
        let Some((hops, base, aave_pool)) = self.liq_precheck(&plan).await else {
            return;
        };
        let Some(sim) = &self.sim else {
            return self.metrics.event("liq_candidate", base);
        };
        let (profit, gas) = match sim
            .simulate_liquidation(&plan, &hops, U256::ZERO, aave_pool)
            .await
        {
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
        let gas_price = self.gas_price().await;
        let gas_usd = gas as f64 * gas_price * native_px;
        let net_usd = profit_usd - gas_usd;
        self.metrics.update(|c| c.liq_profitable += 1);
        self.metrics.event(
            "liq_sim_ok",
            json!({ "liq": base, "profit_usd": profit_usd, "gas": gas, "gas_usd": gas_usd, "net_usd": net_usd }),
        );
        info!(protocol = plan.protocol, account = %plan.account, profit_usd = format!("{profit_usd:.4}"),
              gas_usd = format!("{gas_usd:.4}"), net_usd = format!("{net_usd:.4}"), "simulated liquidation");

        if !self.sending(false) {
            // Shadow: log the PGA bid this would have placed (nothing is sent).
            let _ = self.pga_bid(profit_usd, gas, gas_price, native_px, &base);
            return;
        }
        let Some(sender) = self.sender.clone() else {
            return;
        };
        if let Err(block) = self.risk.check(net_usd, gas_usd, now_secs()) {
            self.on_cap(&block, false);
            return self.metrics.event(
                "risk_block",
                json!({ "liq": base, "reason": format!("{block:?}") }),
            );
        }
        let Some((fees, tip_usd)) = self.pga_bid(profit_usd, gas, gas_price, native_px, &base)
        else {
            return;
        };
        // On-chain floor: at least cover gas and the bid, in debt-asset units.
        let min_profit = if debt_px > 0.0 {
            U256::from(((gas_usd + tip_usd) / debt_px) as u128)
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
                .send_liquidation(&plan, &hops, min_profit, gas_limit, Some(fees), aave_pool)
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
                            route: None,
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
                route,
            } => {
                if let Some(r) = route {
                    self.release_pools(r);
                }
                if let (false, Some(r)) = (landed.success, route) {
                    // Lost race or mispriced: the next attempt on this route likely fails the same
                    // way. Spend gas on uncontested (long-tail) routes instead.
                    self.cooldown.insert(r, Instant::now());
                }
                let gas_usd =
                    landed.gas_used as f64 * landed.effective_gas_price as f64 * native_px;
                if kind == "arb" {
                    self.risk.record_arb_gas(gas_usd, now_secs());
                }
                if !landed.success && kind != "liquidation" {
                    // Arbs and unsimulated backruns revert when another bot got there first.
                    self.risk.record_lost_race(gas_usd, now_secs());
                } else {
                    self.risk.record(landed.success, gas_usd, now_secs());
                }
                if landed.success {
                    self.risk
                        .record_profit(est_profit_usd - gas_usd, now_secs());
                }
                let bidding = self.cfg.pga.enabled;
                self.metrics.update(|c| {
                    if bidding {
                        if landed.success {
                            c.bids_won += 1
                        } else {
                            c.bids_lost += 1
                        }
                    }
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
            TxDone::TipSeen(tip) => self.tips.record(tip, now_secs()),
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
            // Broadcast to the sequencer and every extra endpoint (the primary is the reader).
            let extra: Vec<String> = cfg
                .sequencer_rpc
                .iter()
                .cloned()
                .chain(secrets.extra_http_urls.iter().cloned())
                .collect();
            let mut s = Sender::new(pk, rpc.http.clone(), &extra, exec, cfg.chain_id).await?;
            s.flash_swaps = cfg.executor_flash_swaps;
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
    let mut sim = Simulator::new(rpc.http.clone(), exec, owner);
    sim.flash_swaps = cfg.executor_flash_swaps;
    Ok((Some(sim), sender))
}

/// Build the liquidation runner from config: one adapter per enabled protocol section.
async fn build_liquidation(
    cfg: &ChainConfig,
    rpc: &Rpc,
    sweep_urls: &[String],
    data_dir: &std::path::Path,
    executor: Address,
    meters: &mut Vec<(&'static str, Arc<RpcMeter>)>,
) -> Result<Option<(Runner, Vec<Address>)>> {
    let Some(lcfg) = cfg.liquidation.clone().filter(|l| l.enabled) else {
        return Ok(None);
    };
    let arpc = AdapterRpc {
        fast: rpc.http.clone(),
        scan: metered(&lcfg.scan_rpc, "scan", meters)?,
        // Keyed sweep endpoints first (from .env), then the public ones as fallbacks.
        sweep: metered_rotating(
            sweep_urls
                .iter()
                .cloned()
                .chain(std::iter::once(lcfg.sweep_rpc.clone()))
                .chain(lcfg.sweep_rpc_fallbacks.clone())
                .collect(),
            "sweep",
            meters,
        )?,
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

fn metered(
    url: &str,
    name: &'static str,
    meters: &mut Vec<(&'static str, Arc<RpcMeter>)>,
) -> Result<alloy::providers::DynProvider> {
    let m = RpcMeter::new();
    meters.push((name, m.clone()));
    mev_rpc::metered_http_provider(url, m)
}

fn metered_rotating(
    urls: Vec<String>,
    name: &'static str,
    meters: &mut Vec<(&'static str, Arc<RpcMeter>)>,
) -> Result<alloy::providers::DynProvider> {
    let m = RpcMeter::new();
    meters.push((name, m.clone()));
    mev_rpc::metered_rotating_provider(&urls, m)
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

/// Watched pools and their tokens as raw 20-byte addresses (presim calldata filter).
fn watchlist(store: &PoolStore) -> std::collections::HashSet<[u8; 20]> {
    store
        .pools()
        .flat_map(|p| [p.info.address, p.info.token0, p.info.token1])
        .map(|a| a.0.0)
        .collect()
}

async fn fetch_gas_price(http: &alloy::providers::DynProvider) -> f64 {
    http.get_gas_price()
        .await
        .map(|g| g as f64)
        .unwrap_or(10_000_000.0)
}

/// Hashes seen recently, bounded (oldest evicted): drops a tx delivered by both feeds.
struct RecentHashes {
    set: std::collections::HashSet<TxHash>,
    order: std::collections::VecDeque<TxHash>,
    cap: usize,
}

impl RecentHashes {
    fn new(cap: usize) -> Self {
        Self {
            set: Default::default(),
            order: Default::default(),
            cap,
        }
    }

    /// True if `h` is new.
    fn insert(&mut self, h: TxHash) -> bool {
        if !self.set.insert(h) {
            return false;
        }
        self.order.push_back(h);
        if self.order.len() > self.cap
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

/// Recent tx hashes remembered for de-duplicating the two feeds (~a minute of Arbitrum traffic).
const FEED_DEDUP: usize = 20_000;

/// Run the Fast Feed client and forward its txs as `SequencedTx` (block-derived `seq`).
fn spawn_fast_feed(
    url: String,
    key: String,
    out: mpsc::UnboundedSender<mev_sequencer::SequencedTx>,
    stats: Arc<mev_ingest::fast_feed::FastFeedStats>,
) {
    use alloy::consensus::Transaction as _;
    let (ff_tx, mut ff_rx) = mpsc::unbounded_channel::<mev_ingest::fast_feed::FastFeedTx>();
    tokio::spawn(mev_ingest::fast_feed::run(url, key, ff_tx, stats));
    tokio::spawn(async move {
        while let Some(f) = ff_rx.recv().await {
            let st = mev_sequencer::SequencedTx {
                seq: f
                    .block_number
                    .saturating_sub(mev_sequencer::ARB_ONE_GENESIS_BLOCK),
                received: f.received,
                hash: f.hash,
                to: f.tx.to(),
                input: f.tx.input().clone(),
                tx: f.tx,
            };
            if out.send(st).is_err() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod feed_dedup_tests {
    use super::*;

    #[test]
    fn recent_hashes_drop_repeats_and_stay_bounded() {
        let mut r = RecentHashes::new(2);
        let (a, b, c) = (
            TxHash::repeat_byte(1),
            TxHash::repeat_byte(2),
            TxHash::repeat_byte(3),
        );
        assert!(r.insert(a) && !r.insert(a));
        assert!(r.insert(b) && r.insert(c));
        assert!(r.insert(a), "evicted after capacity, so new again");
    }
}

/// Backtest of the live bidding path (policy + tip book) on synthetic PGA rounds, against the
/// pre-PGA behaviour of always bidding zero. Synthetic: rivals tip 0.001-0.03 gwei (the range
/// seen ahead of our reverted arbs on 2026-10-05), opportunities $0.05-$1 gross.
#[cfg(test)]
mod pga_replay {
    use super::*;
    use mev_sim::pga::{Offer, PastView, Round, Strategy, replay};

    struct Live {
        policy: BidPolicy,
        tips: TipBook,
        fed: usize,
    }

    impl Strategy for Live {
        fn bid(&mut self, o: &Offer, past: &PastView) -> Option<u128> {
            // Feed reveals exactly as the engine does from race reports (one per past round).
            for t in &past.winning_tips()[self.fed..] {
                self.tips.record(*t, 0);
            }
            self.fed = past.winning_tips().len();
            let x = BidInput {
                gross_profit_usd: o.gross_usd,
                gas_units: o.gas_units,
                base_fee_wei: o.base_fee_wei,
                usd_per_wei: o.usd_per_wei,
                competition_tip_wei: self.tips.estimate(0.75, 0),
            };
            match self.policy.decide(&x) {
                Decision::Bid(b) => Some(b.max_priority_fee_per_gas),
                Decision::Skip(_) => None,
            }
        }
    }

    struct Zero;
    impl Strategy for Zero {
        fn bid(&mut self, _o: &Offer, _p: &PastView) -> Option<u128> {
            Some(0)
        }
    }

    fn synthetic(n: usize) -> Vec<Round> {
        // Deterministic LCG: no rand dependency.
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as f64 / (1u64 << 31) as f64
        };
        (0..n)
            .map(|_| Round {
                gross_usd: 0.05 + next() * 0.95,
                gas_units: 330_000,
                base_fee_wei: 10_000_000,
                usd_per_wei: 3000.0 / 1e18,
                competitor_tips: vec![(1_000_000.0 + next() * 29_000_000.0) as u128],
                competitor_first: next() < 0.5,
            })
            .collect()
    }

    #[test]
    fn bidding_beats_the_zero_tip_baseline_and_never_loses_money_per_win() {
        let rounds = synthetic(500);
        let zero = replay(&rounds, &mut Zero);
        let mut live = Live {
            policy: bid_policy(&mev_config::PgaConfig::default()),
            tips: TipBook::new(TIP_BOOK_CAPACITY, TIP_BOOK_HORIZON_SECS),
            fed: 0,
        };
        let bid = replay(&rounds, &mut live);
        assert_eq!(zero.wins, 0, "a zero tip loses every contested round");
        assert!(zero.net_usd < 0.0);
        assert!(
            bid.wins > 0 && bid.net_usd > zero.net_usd,
            "{bid:?} vs {zero:?}"
        );
        // Default caps: no single tip above $0.25.
        assert!(bid.tips_paid_usd <= 0.25 * bid.bids as f64 + 1e-9);
    }
}
