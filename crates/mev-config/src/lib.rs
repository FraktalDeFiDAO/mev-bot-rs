//! Per-chain TOML config plus secrets from the environment (never from files in the repo).

use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, anyhow};
use mev_types::{Address, B256};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DexType {
    Univ2,
    Camelot,
    Univ3,
    Pancakev3,
    Algebra,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DexConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: DexType,
    pub factory: Address,
    /// V2 fixed fee in basis points.
    #[serde(default)]
    pub fee_bps: u16,
    /// V3 fee tiers to look up.
    #[serde(default)]
    pub fees: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub min_profit_usd: f64,
    pub max_gas_usd_per_day: f64,
    pub max_consecutive_failures: u32,
    pub max_loan_usd: f64,
    /// Liquidation plans claiming a bigger bonus than this are treated as broken/manipulated
    /// oracles (e.g. bad-debt markets pricing dead collateral) and skipped.
    #[serde(default = "default_max_bonus_usd")]
    pub max_bonus_usd: f64,
    /// Live arbitrage: simulate (one eth_call round trip) before sending. Off by default: send
    /// as soon as the local pool model shows profit; the executor's `minProfit` (>= gas) reverts
    /// trades that turn out unprofitable, so a stale model costs gas, not capital.
    #[serde(default)]
    pub arb_presimulate: bool,
    /// Speed over accuracy for live sends: skip *every* pre-send simulation (the `arb_presimulate`, inexact-route,
    /// unvetted-route and V4 checks all stay configured but are bypassed) and send on the local model. The
    /// executor's `minProfit` still reverts a trade that turns out unprofitable, so a wrong model costs gas, not
    /// capital. Turn off once an own node makes the check cheap (DECISIONS #88).
    #[serde(default)]
    pub fast_send: bool,
    /// Transactions in flight at once (arbitrages on disjoint pools). Default 1.
    #[serde(default = "default_max_concurrent_sends")]
    pub max_concurrent_sends: u32,
    /// Daily gas arbitrage may spend (of `max_gas_usd_per_day`); the rest stays for liquidations.
    /// Default: two thirds.
    #[serde(default)]
    pub max_arb_gas_usd_per_day: Option<f64>,
}

/// Find long-tail pairs from recent swap activity instead of only the fixed `pairs` list.
#[derive(Debug, Clone, Deserialize)]
pub struct AutoDiscovery {
    pub enabled: bool,
    /// Recent blocks to scan for Swap/Sync events.
    pub blocks: u64,
    /// Blocks per eth_getLogs request.
    pub step: u64,
    /// RPC used only for the log scan (free tiers often cap getLogs ranges).
    pub log_rpc: String,
    /// Each active token is paired with these (loan-able) tokens.
    pub pair_with: Vec<String>,
    /// Cap on active tokens taken from the scan (most active first).
    pub max_tokens: usize,
    /// Also pair the most active non-partner tokens with each other (X/Y pools), so triangles
    /// between two long-tail tokens (L -> X -> Y -> L) can form. 0 = off.
    #[serde(default)]
    pub cross_pairs: usize,
}

/// Settings shared by all liquidation adapters.
#[derive(Debug, Clone, Deserialize)]
pub struct LiquidationConfig {
    pub enabled: bool,
    /// RPC for account-discovery log scans. Must return complete wide-range getLogs results —
    /// several public RPCs silently truncate them (verified 2026-10-03: only arb1.arbitrum.io/rpc
    /// was complete).
    pub scan_rpc: String,
    /// RPC for full health sweeps (eth_call only, so any public RPC works).
    pub sweep_rpc: String,
    /// More sweep RPCs: requests rotate across all of them and fall through on rate limits.
    #[serde(default)]
    pub sweep_rpc_fallbacks: Vec<String>,
    /// Accounts below this health factor are polled on price-feed updates.
    pub at_risk_hf: f64,
    /// Ignore positions with less debt (or collateral) than this: the bonus wouldn't cover gas.
    pub min_debt_usd: f64,
    pub sweep_secs: u64,
    /// Fallback at-risk poll period; sequencer price-feed triggers handle the fast path.
    pub fallback_poll_secs: u64,
    /// Swap routes for seized collateral may go through these tokens.
    pub route_via: Vec<String>,
    /// Shadow mode only (JEV-LIQ-015): also simulate the exact final calldata with REVM at a
    /// hash-pinned block and log it against an `eth_call` of the same bytes at that block.
    /// Never influences a dispatch decision. Default off.
    #[serde(default)]
    pub revm_shadow: bool,
}

/// Shadow-only liquidation pipeline (trigger indexer, snapshot refresh, mandatory-schedule
/// record). Never plans, simulates or sends. Everything is off unless `enabled`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LiquidationShadowConfig {
    pub enabled: bool,
    /// Snapshot database; `None` = `<data-dir>/liq-state.sqlite`.
    pub snapshot_db: Option<std::path::PathBuf>,
    /// Recheck batches in flight (each batch is `recheck_batch` accounts at one pinned block).
    pub recheck_concurrency: usize,
    /// Accounts per batch (read through Multicall3, 50 accounts per request).
    pub recheck_batch: usize,
    /// Accounts rechecked per minute at most, spread evenly over the ticks.
    pub recheck_budget_per_minute: u32,
    /// Hard deadline after which a hot account's snapshot counts as expired.
    pub max_account_age_seconds: u64,
    /// Debt (whole USD, Aave base currency) below which an account is dust ("cold").
    pub min_debt_usd: u64,
    /// Expiry of cold (dust) snapshots; events still mark them dirty at once.
    pub cold_max_age_seconds: u64,
    /// Never-read borrowers the schedule pass loads at once (the rest are counted as deferred).
    pub max_unknown_per_pass: usize,
    /// Borrowers whose dependency set the trigger indexer reconciles per step (borrowers
    /// without debt are deactivated there).
    pub reconcile_batch: usize,
    /// Period of the repository census (`liq_shadow_census`: counts and lowest health factors).
    pub census_seconds: u64,
    /// Period of the schedule pass and of the refresh task.
    pub tick_ms: u64,
}

impl Default for LiquidationShadowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            snapshot_db: None,
            recheck_concurrency: 2,
            recheck_batch: 200,
            recheck_budget_per_minute: 20_000,
            max_account_age_seconds: 300,
            min_debt_usd: 10,
            cold_max_age_seconds: 3600,
            max_unknown_per_pass: 2000,
            reconcile_batch: 500,
            census_seconds: 60,
            tick_ms: 5000,
        }
    }
}

/// Aave v3 adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct AaveConfig {
    pub enabled: bool,
    pub pool: Address,
    pub data_provider: Address,
    pub oracle: Address,
    /// First block to index Borrow events from (pool deployment).
    pub from_block: u64,
    pub scan_step: u64,
    /// Pinned deployment (`[aave.manifest]`); checked once at startup (JEV-LIQ-002).
    pub manifest: Option<AaveManifest>,
}

/// Pinned Aave v3 deployment: what the chain must look like before the adapter is trusted.
#[derive(Debug, Clone, Deserialize)]
pub struct AaveManifest {
    pub chain_id: u64,
    pub pool_addresses_provider: Address,
    pub pool: Address,
    pub oracle: Address,
    /// Pool implementation behind the proxy (EIP-1967 slot) at the pinned block.
    pub pool_implementation: Address,
    /// keccak256 of `eth_getCode(pool_implementation)`. An Aave upgrade changes it and fails
    /// the startup check until the manifest is re-pinned.
    pub pool_implementation_code_hash: B256,
    /// Block at which the values above were observed (provenance only).
    pub verified_block: u64,
    /// aave-address-book file and commit the addresses were taken from.
    pub source: String,
}

/// One Compound III (Comet) market.
#[derive(Debug, Clone, Deserialize)]
pub struct CometConfig {
    pub address: Address,
    /// Chainlink feed converting the Comet's price unit to USD (only for non-USD-priced Comets,
    /// e.g. the WETH Comet prices in ETH).
    pub usd_feed: Option<Address>,
}

/// Moonwell (a Compound v2 fork) adapter: one comptroller, its markets are read on-chain.
#[derive(Debug, Clone, Deserialize)]
pub struct MoonwellConfig {
    pub enabled: bool,
    pub comptroller: Address,
    /// Never scan `Borrow` events below this block (comptroller deployment).
    pub from_block: u64,
    pub scan_step: u64,
}

/// Compound III adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct CompoundV3Config {
    pub enabled: bool,
    pub from_block: u64,
    pub scan_step: u64,
    /// Discounted protocol collateral reserves worth less than this (USD) are ignored.
    pub min_storefront_usd: f64,
    pub comets: Vec<CometConfig>,
}

/// Morpho Blue adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct MorphoConfig {
    pub enabled: bool,
    pub morpho: Address,
    /// Asset -> USD oracle (8 decimals) for loan tokens; the Aave oracle covers the usual ones.
    pub usd_oracle: Address,
    pub from_block: u64,
    pub scan_step: u64,
}

/// Priority Gas Auction (PGA) bidding on Arbitrum One: each tx sets its own priority fee, the
/// Sequencer orders each 125 ms round by it (DECISIONS #59). Defaults are conservative.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PgaConfig {
    /// Bid a priority fee at all (off = every tx bids zero: last in its round).
    pub enabled: bool,
    /// Largest share of expected net profit spent on the tip.
    pub max_fraction_of_net: f64,
    /// Hard caps on tip spend, USD.
    pub max_tip_usd_per_tx: f64,
    pub max_tip_usd_per_minute: f64,
    pub max_tip_usd_per_day: f64,
    /// Tip when no competition has been observed yet, gwei per gas.
    pub default_tip_gwei: f64,
    /// Never bid below this, gwei per gas.
    pub min_tip_gwei: f64,
    /// Outbid the competition estimate by this fraction.
    pub outbid_margin: f64,
    /// Feed txs tipping less than this (gwei per gas) are ordinary users, not bots, and are
    /// ignored when estimating competition (wallets default to ~0.01 gwei).
    pub feed_min_bot_tip_gwei: f64,
    /// Quantile of recently observed winning tips used as the competition estimate.
    pub competition_quantile: f64,
    /// Winners recorded on a route before it is priced from them alone.
    pub route_min_observations: usize,
    /// Quantile of the market (all winners, all bot-like feed tips) used for a route with less
    /// history than that: the median, not the top of a bimodal market.
    pub market_quantile: f64,
    /// Nominal PGA round (docs: 125 ms on Arbitrum One); a send signed later than this after its
    /// trigger is counted as over (it likely bids in a later round than its trigger).
    pub round_ms: u64,
    pub fast_feed: FastFeedConfig,
}

impl Default for PgaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_fraction_of_net: 0.3,
            max_tip_usd_per_tx: 0.25,
            max_tip_usd_per_minute: 0.5,
            max_tip_usd_per_day: 2.0,
            default_tip_gwei: 0.05,
            min_tip_gwei: 0.01,
            outbid_margin: 0.25,
            feed_min_bot_tip_gwei: 0.02,
            competition_quantile: 0.9,
            route_min_observations: 3,
            market_quantile: 0.5,
            round_ms: 125,
            fast_feed: FastFeedConfig::default(),
        }
    }
}

/// Batching: several liquidations and/or arbitrages ready within a short window go out as one
/// transaction through the executor's `batch` (one nonce, one tip, one base fee for the lot; each
/// trade still succeeds or fails alone). Needs `executor_batch = true` (a redeployed executor).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BatchConfig {
    pub enabled: bool,
    /// How long the first queued trade waits for company, ms.
    pub window_ms: u64,
    /// Flush at once when this many are queued (the executor allows at most 8).
    pub max_items: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            window_ms: 15,
            max_items: 4,
        }
    }
}

/// Initialized-tick data for V3-style pools (Uniswap V3, PancakeV3), so a swap is priced across tick
/// boundaries instead of only inside the current bucket. Loaded around the price at startup and
/// reloaded every `refresh_secs` (liquidity added or removed in between is not seen until then; the
/// pre-send simulation still verifies every trade). Off = the one-bucket model.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TicksConfig {
    pub enabled: bool,
    /// Ticks loaded on each side of the current one (800 is about 8 % of price).
    pub window_ticks: i32,
    pub refresh_secs: u64,
}

impl Default for TicksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            window_ticks: 800,
            refresh_secs: 300,
        }
    }
}

/// Uniswap V4 (hookless pools only; hooked pools can change a swap's outcome in ways we can't model).
/// Pools are found from the PoolManager's `Initialize` events (cached on disk and extended
/// incrementally) and ranked by recent swap activity. Routes through V4 pools are only sent when the
/// deployed executor supports them (`executor_v4`); until then they are logged as `v4_candidate`.
#[derive(Debug, Clone, Deserialize)]
pub struct UniswapV4Config {
    pub enabled: bool,
    pub pool_manager: Address,
    /// RPC for the wide `getLogs` scans (must allow wide ranges: the official Arbitrum RPC does).
    pub log_rpc: String,
    /// First block to scan for pool creation (the PoolManager's deployment is after this).
    #[serde(default = "default_v4_first_block")]
    pub first_block: u64,
    /// Recent blocks scanned for swap activity, and blocks per `getLogs` request.
    #[serde(default = "default_v4_blocks")]
    pub blocks: u64,
    #[serde(default = "default_v4_step")]
    pub step: u64,
    /// A pool needs this many swaps in the window to be tracked; at most `max_pools` are (most active first).
    #[serde(default = "default_v4_min_swaps")]
    pub min_swaps: u64,
    #[serde(default = "default_v4_max_pools")]
    pub max_pools: usize,
    /// Simulate every route through a V4 pool before sending it, even when the model says it is
    /// exact. One extra `eth_call` per candidate; guards against V4 state drifting between events
    /// (a live V4 send lost $0.20 on a model-only route).
    #[serde(default = "default_v4_presimulate")]
    pub presimulate: bool,
}

fn default_v4_presimulate() -> bool {
    true
}

fn default_v4_first_block() -> u64 {
    250_000_000
}
fn default_v4_blocks() -> u64 {
    12_000
}
fn default_v4_step() -> u64 {
    500
}
fn default_v4_min_swaps() -> u64 {
    3
}
fn default_v4_max_pools() -> usize {
    150
}

/// Per-route win-rate gate for arbitrage sends (`mev_risk::gate`, DECISIONS #54): a route is sent
/// only while `p_win × net − (1 − p_win) × cost > 0`; losing streaks back a route off.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GateConfig {
    pub enabled: bool,
    /// Tries every route gets before it is judged.
    pub min_attempts: u32,
    /// Weight of the global win rate when smoothing a route's own (pseudo-attempts).
    pub prior_strength: f64,
    /// Floor on the global win rate used as the prior.
    pub prior_floor: f64,
    /// Consecutive losses that trigger a back-off, its first length and cap (seconds).
    pub backoff_after: u32,
    pub backoff_secs: u64,
    pub backoff_max_secs: u64,
    /// Results older than this are forgotten (seconds).
    pub memory_secs: u64,
    /// Stop all arbitrage sends after this many with no win (0 = off); one probe per `global_probe_secs`.
    pub global_min_attempts: u32,
    pub global_probe_secs: u64,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_attempts: 3,
            prior_strength: 5.0,
            prior_floor: 0.02,
            backoff_after: 3,
            backoff_secs: 600,
            backoff_max_secs: 6 * 3600,
            memory_secs: 24 * 3600,
            global_min_attempts: 30,
            global_probe_secs: 3600,
        }
    }
}

/// Chainlink SVR / FastLane Atlas solver (docs/research/ATLAS.md). Optional and off: the bot only
/// listens to auctions when `enabled`, and only bids when `bid` is also set, a deployed
/// `AtlasSolver` is named, the owner's ETH is bonded on Atlas and the solver's `maxBid` is set.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AtlasConfig {
    /// Connect to the auction feed and log the auctions on this chain (no bids, no funds).
    pub enabled: bool,
    /// Submit solver operations for liquidations the oracle update makes profitable.
    pub bid: bool,
    /// The deployed `AtlasSolver` contract.
    pub solver: Option<Address>,
    /// Chainlink's solver websocket.
    pub url: String,
    /// Share of a liquidation's expected net profit offered as the bid.
    pub bid_fraction: f64,
    /// Never bid more than this, ETH (keep equal to the contract's `maxBid`).
    pub max_bid_eth: f64,
    /// Solver gas limit if the control contract's `getSolverGasLimit` can't be read.
    pub solver_gas_limit: u64,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bid: false,
            solver: None,
            url: "wss://svr-bid-endpoint.chain.link/ws/solver".into(),
            bid_fraction: 0.7,
            max_bid_eth: 0.01,
            solver_gas_limit: 1_500_000,
        }
    }
}

/// The paid Fast Feed (ordered txs inside the PGA round, before the block closes). Off by
/// default: a daily ticket costs at least $17 (DECISIONS #59). The API key comes only from the
/// environment (`<PREFIX>_FAST_FEED_KEY`), never from config files.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FastFeedConfig {
    pub enabled: bool,
    pub url: String,
}

impl Default for FastFeedConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "wss://arb1-txfeed.arbitrum.io/".into(),
        }
    }
}

/// Feed-driven arbitrage: simulate sequencer-feed transactions (eth_simulateV1) to learn which
/// pools they move before their block exists.
#[derive(Debug, Clone, Deserialize)]
pub struct PresimConfig {
    pub enabled: bool,
    /// Tried in order, falling through on errors/rate limits (PublicNode ~37 ms p50 from the server).
    pub rpc_urls: Vec<String>,
    /// Simulations in flight; feed txs arriving while full are skipped (their block logs still count).
    #[serde(default = "default_presim_in_flight")]
    pub max_in_flight: usize,
    /// Give up on a simulation after this long: its result would arrive after the block.
    #[serde(default = "default_presim_timeout_ms")]
    pub timeout_ms: u64,
    /// Simulate feed txs in-process (revm over a state cache kept exact by per-block diffs);
    /// `rpc_urls` then only serves candidates whose state isn't cached yet.
    #[serde(default)]
    pub local: bool,
    /// Must serve `debug_traceBlockByNumber` with `prestateTracer` (dRPC does; PublicNode/arb1 don't).
    #[serde(default)]
    pub diff_rpc_urls: Vec<String>,
    /// State reads at a block for cache misses.
    #[serde(default)]
    pub fill_rpc_urls: Vec<String>,
    /// Roll back presim-applied state that no real log confirmed within a few seconds. Off: the bot only
    /// records what it *would* roll back (`presim_unconfirmed_observed`) and the next resync shows whether the
    /// chain agreed, so the decision is tested before it is acted on (DECISIONS #86).
    #[serde(default)]
    pub rollback_unconfirmed: bool,
    /// Keep every candidate feed transaction (with what we decoded and what the simulation said) in
    /// `feedtx-YYYY-MM-DD.jsonl` for later analysis; files older than `record_keep_days` are deleted.
    #[serde(default)]
    pub record_feed: bool,
    #[serde(default = "default_record_keep_days")]
    pub record_keep_days: u32,
    /// With `record_feed`: also fetch every `chain_sample_every`-th block's receipts and write `chaintx-DATE.jsonl`
    /// (status, gas, position, swap/liquidity/liquidation events) for recorded feed transactions and for any
    /// transaction emitting such an event. 0 turns it off. One `eth_getBlockReceipts` call per sampled block
    /// (Arbitrum makes ~4 blocks/s, so 4 means ~1 call/s).
    #[serde(default)]
    pub chain_sample_every: u64,
}

fn default_record_keep_days() -> u32 {
    7
}

fn default_max_cycles() -> usize {
    20_000
}

fn default_presim_in_flight() -> usize {
    16
}

fn default_presim_timeout_ms() -> u64 {
    250
}

/// Fluid vaults adapter (resolver-driven: no account index).
#[derive(Debug, Clone, Deserialize)]
pub struct FluidConfig {
    pub enabled: bool,
    pub vault_resolver: Address,
    /// Asset -> USD oracle (8 decimals); the Aave oracle covers the usual vault tokens.
    pub usd_oracle: Address,
    /// T1 (ERC20/ETH collateral, ERC20 debt) vaults to watch. Each must be allowlisted on the executor.
    pub vaults: Vec<Address>,
    /// Also watch every T1 vault the resolver lists, not only `vaults`. Liquidations move between vaults (in the
    /// 9 days to 2026-10-12 none of the 69 happened on the two vaults first configured), so a fixed list goes stale.
    /// A vault that is not allowlisted on the executor is quoted and simulated but its liquidation reverts there
    /// (`TargetNotAllowed`); allowlist it with `scripts/allow-targets.sh`.
    #[serde(default)]
    pub auto_vaults: bool,
    /// Fluid's `DexFactory`: every DEX pool it lists has its `Swap` events recorded (re-listed hourly).
    #[serde(default)]
    pub dex_factory: Option<Address>,
}

/// Journals of what happens on the watched exchanges, written to `data/<chain>/` as one JSON line per record.
#[derive(Debug, Clone, Deserialize)]
pub struct JournalConfig {
    /// Every event emitted by a tracked pool (and the V4 PoolManager), every block, as it arrives: swaps, liquidity
    /// changes, state restatements, fee collection, flash loans, pool creation, donations. `poolevents-DATE.jsonl`.
    #[serde(default)]
    pub pool_events: bool,
    /// Days of journal files kept.
    #[serde(default = "default_journal_keep_days")]
    pub keep_days: u32,
}

fn default_journal_keep_days() -> u32 {
    7
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            pool_events: false,
            keep_days: default_journal_keep_days(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub balancer_vault: Address,
    /// Extra endpoint that receives raw txs directly (Arbitrum sequencer).
    pub sequencer_rpc: Option<String>,
    pub min_pool_liquidity_usd: f64,
    /// Most triangular cycles kept in the index (deepest first; thin long-tail cycles rank last).
    #[serde(default = "default_max_cycles")]
    pub max_cycles: usize,
    pub gas_limit_estimate: u64,
    /// The deployed executor has `executeFlashSwap`: routes starting on a V3-style pool run
    /// without a lender (no fee, no lender-balance cap). Only set after deploying that executor.
    #[serde(default)]
    pub executor_flash_swaps: bool,
    /// The deployed executor has `batch`. Only set after deploying that executor.
    #[serde(default)]
    pub executor_batch: bool,
    /// The deployed executor has Uniswap V4 hops (and its PoolManager is set). Only set after
    /// deploying that executor; until then V4 routes are logged, never sent.
    #[serde(default)]
    pub executor_v4: bool,
    /// Uniswap V4 pool discovery (off unless configured).
    #[serde(default)]
    pub uniswap_v4: Option<UniswapV4Config>,
    /// Send ready trades together as one transaction (needs `executor_batch`). Off by default.
    #[serde(default)]
    pub batch: BatchConfig,
    /// Tick-aware pricing for V3-style pools.
    #[serde(default)]
    pub ticks: TicksConfig,
    /// The deployed executor has `liquidateViaAave`: liquidations whose loan exceeds the Balancer
    /// cap borrow from the Aave pool (`[aave] pool`) instead (5 bps premium, deep liquidity).
    #[serde(default)]
    pub executor_aave_flash: bool,
    pub tokens: HashMap<String, Address>,
    /// Tokens we may flash-borrow (must be held by the Balancer vault).
    pub loan_tokens: Vec<String>,
    /// Token used as the USD reference (e.g. USDC).
    pub usd_token: String,
    /// Token used as the native/gas reference (e.g. WETH).
    pub native_token: String,
    pub pairs: Vec<[String; 2]>,
    pub dexes: Vec<DexConfig>,
    pub risk: RiskConfig,
    pub auto_discovery: Option<AutoDiscovery>,
    /// Arbitrum sequencer feed (free, unmetered real-time tx stream).
    pub sequencer_feed: Option<String>,
    /// OP Stack Flashblocks WebSocket (Base): the same role as `sequencer_feed` on chains without
    /// an Arbitrum-style feed. Ignored when `sequencer_feed` is set.
    #[serde(default)]
    pub flashblocks_feed: Option<String>,
    /// Free public HTTP endpoints for the main reads, used only while every keyed
    /// `<PREFIX>_HTTP_URL*` fails or is rate-limited (an exhausted monthly quota stopped the bot
    /// on 2026-10-15).
    #[serde(default)]
    pub http_fallbacks: Vec<String>,
    /// Free public WS endpoints, tried after the keyed `<PREFIX>_WS_URL*` ones.
    #[serde(default)]
    pub ws_fallbacks: Vec<String>,
    pub liquidation: Option<LiquidationConfig>,
    #[serde(default)]
    pub liquidation_shadow: LiquidationShadowConfig,
    pub aave: Option<AaveConfig>,
    pub compound_v3: Option<CompoundV3Config>,
    pub morpho: Option<MorphoConfig>,
    #[serde(default)]
    pub moonwell: Option<MoonwellConfig>,
    pub fluid: Option<FluidConfig>,
    #[serde(default)]
    pub presim: Option<PresimConfig>,
    #[serde(default)]
    pub journal: JournalConfig,
    /// Arbitrum One Priority Gas Auction bidding and the optional Fast Feed. Absent = defaults.
    #[serde(default)]
    pub pga: PgaConfig,
    /// Optional Atlas/SVR solver (off by default).
    #[serde(default)]
    pub atlas: AtlasConfig,
    /// Per-route win-rate gate for arbitrage sends.
    #[serde(default)]
    pub gate: GateConfig,
}

impl ChainConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        fn validate_numbers(value: &toml::Value, field: &str) -> Result<()> {
            match value {
                toml::Value::Float(v) if !v.is_finite() || *v < 0.0 => {
                    anyhow::bail!("{field} must be finite and nonnegative");
                }
                toml::Value::Integer(v) if *v < 0 => anyhow::bail!("{field} must be nonnegative"),
                toml::Value::Table(t) => {
                    for (k, v) in t {
                        validate_numbers(v, &format!("{field}.{k}"))?;
                    }
                }
                toml::Value::Array(a) => {
                    for v in a {
                        validate_numbers(v, field)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        validate_numbers(&toml::from_str::<toml::Value>(&raw)?, "config")?;
        let cfg: ChainConfig =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        anyhow::ensure!(
            !cfg.atlas.bid,
            "atlas.bid requires verified escrow liability and charge settlement; bidding is unavailable"
        );
        for [a, b] in &cfg.pairs {
            cfg.token(a)?;
            cfg.token(b)?;
        }
        for t in cfg
            .loan_tokens
            .iter()
            .chain([&cfg.usd_token, &cfg.native_token])
        {
            cfg.token(t)?;
        }
        if let Some(ad) = &cfg.auto_discovery {
            for t in &ad.pair_with {
                cfg.token(t)?;
            }
        }
        Ok(cfg)
    }

    pub fn token(&self, symbol: &str) -> Result<Address> {
        self.tokens
            .get(symbol)
            .copied()
            .ok_or_else(|| anyhow!("unknown token symbol {symbol}"))
    }

    /// Configured pairs as addresses.
    pub fn pair_addresses(&self) -> Result<Vec<(Address, Address)>> {
        self.pairs
            .iter()
            .map(|[a, b]| Ok((self.token(a)?, self.token(b)?)))
            .collect()
    }

    pub fn symbol(&self, addr: Address) -> String {
        self.tokens
            .iter()
            .find(|(_, a)| **a == addr)
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| addr.to_string())
    }
}

/// Secrets and endpoints, read from env (loaded from a git-ignored `.env`).
#[derive(Clone)]
pub struct Secrets {
    pub http_url: String,
    pub ws_url: String,
    pub private_key: Option<String>,
    pub profit_receiver: Option<Address>,
    pub executor: Option<Address>,
    /// Additional endpoints (`<PREFIX>_HTTP_URL_2..9`, `<PREFIX>_WS_URL_2..9`), e.g. a second
    /// Alchemy app: paid reads rotate across all HTTP URLs, WS fails over in order.
    pub extra_http_urls: Vec<String>,
    pub extra_ws_urls: Vec<String>,
    /// Keyed endpoints for the heavy health sweeps (`<PREFIX>_SWEEP_URL_1..9`), tried before the
    /// public `sweep_rpc` / fallbacks (free public endpoints rate-limit the sweep bursts).
    pub sweep_urls: Vec<String>,
    /// Keyed endpoints serving `eth_simulateV1` (`<PREFIX>_PRESIM_URL_1..9`), rotated together with
    /// the public `[presim] rpc_urls`.
    pub presim_urls: Vec<String>,
    /// Fast Feed API key (`<PREFIX>_FAST_FEED_KEY`), if subscribed.
    pub fast_feed_key: Option<String>,
}

impl Secrets {
    /// Primary HTTP URL followed by the extras.
    pub fn http_urls(&self) -> Vec<String> {
        std::iter::once(self.http_url.clone())
            .chain(self.extra_http_urls.iter().cloned())
            .filter(|u| !u.is_empty())
            .collect()
    }

    /// Primary WS URL followed by the extras.
    pub fn ws_urls(&self) -> Vec<String> {
        std::iter::once(self.ws_url.clone())
            .chain(self.extra_ws_urls.iter().cloned())
            .filter(|u| !u.is_empty())
            .collect()
    }
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field(
                "private_key",
                &self.private_key.as_ref().map(|_| "<redacted>"),
            )
            .field("profit_receiver", &self.profit_receiver)
            .field("executor", &self.executor)
            .finish_non_exhaustive()
    }
}

impl Secrets {
    pub fn from_env(prefix: &str) -> Result<Self> {
        Self::load_env(prefix, true)
    }

    /// Shadow execution never reads the production signing key.
    pub fn read_only(prefix: &str) -> Result<Self> {
        Self::load_env(prefix, false)
    }

    fn load_env(prefix: &str, signing: bool) -> Result<Self> {
        let get = |k: &str| std::env::var(k).ok();
        let var = |k: &str| chain_var(prefix, k, &get);
        let addr = |k: &str| -> Result<Option<Address>> {
            match var(k) {
                Some(v) => {
                    Ok(Some(v.parse().with_context(|| {
                        format!("{prefix}_{k} / {k} is not an address")
                    })?))
                }
                None => Ok(None),
            }
        };
        Ok(Self {
            // Optional: a chain may run on the public `http_fallbacks` / `ws_fallbacks` alone.
            http_url: var("HTTP_URL").unwrap_or_default(),
            ws_url: var("WS_URL").unwrap_or_default(),
            private_key: signing
                .then(|| std::env::var("PRIVATE_KEY").ok())
                .flatten()
                .filter(|k| !k.trim().is_empty()),
            profit_receiver: addr("PROFIT_RECEIVER")?,
            executor: addr("EXECUTOR_ADDRESS")?,
            extra_http_urls: numbered(prefix, "HTTP_URL"),
            // `<PREFIX>_WS_URL_2..9`, then `<PREFIX>_WS_FALLBACK_1..9` (more providers to fail over to).
            extra_ws_urls: numbered(prefix, "WS_URL")
                .into_iter()
                .chain(numbered_from(prefix, "WS_FALLBACK", 1))
                .collect(),
            sweep_urls: numbered_from(prefix, "SWEEP_URL", 1),
            presim_urls: numbered_from(prefix, "PRESIM_URL", 1),
            fast_feed_key: var("FAST_FEED_KEY"),
        })
    }
}

fn default_max_concurrent_sends() -> u32 {
    1
}

fn default_max_bonus_usd() -> f64 {
    50_000.0
}

/// The chain the unprefixed variables (`HTTP_URL`, `EXECUTOR_ADDRESS`, ...) belong to: they predate
/// multi-chain support. Another chain reading them would connect to Arbitrum's endpoints or treat
/// the Arbitrum executor as its own.
const LEGACY_PREFIX: &str = "ARBITRUM";
/// Same on every chain (an EOA), so every chain may fall back to the unprefixed variable.
const CHAIN_INDEPENDENT: &[&str] = &["PROFIT_RECEIVER"];

/// `<PREFIX>_<KEY>`, else the unprefixed `<KEY>` where that is safe; trimmed, empty = unset.
fn chain_var(prefix: &str, key: &str, get: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let clean = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    clean(get(&format!("{prefix}_{key}"))).or_else(|| {
        (prefix == LEGACY_PREFIX || CHAIN_INDEPENDENT.contains(&key))
            .then(|| clean(get(key)))
            .flatten()
    })
}

/// `<PREFIX>_<KEY>_2` .. `_9`, in order, skipping unset/empty ones.
fn numbered(prefix: &str, key: &str) -> Vec<String> {
    numbered_from(prefix, key, 2)
}

/// `<PREFIX>_<KEY>_<from>` .. `_9`, in order, skipping unset/empty ones.
fn numbered_from(prefix: &str, key: &str, from: u32) -> Vec<String> {
    (from..=9)
        .filter_map(|i| std::env::var(format!("{prefix}_{key}_{i}")).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_config() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml")
    }

    #[test]
    fn liquidation_shadow_defaults_off_and_arbitrum_bounds_it() {
        let d = LiquidationShadowConfig::default();
        assert!(!d.enabled && d.snapshot_db.is_none());
        assert_eq!(
            (d.recheck_concurrency, d.max_account_age_seconds, d.tick_ms),
            (2, 300, 5000)
        );
        let e: LiquidationShadowConfig = toml::from_str("enabled = true").unwrap();
        assert!(e.enabled && e.max_account_age_seconds == 300);
        // Off while the bot runs on free public RPCs (it duplicates the runner's sweeps); when
        // turned on, every input of a pass is bounded (dust cutoff, unknowns per pass, batched reads).
        let chain = ChainConfig::load(&repo_config()).unwrap();
        let c = chain.liquidation_shadow;
        assert!(!c.enabled);
        assert!(!chain.http_fallbacks.is_empty() && !chain.ws_fallbacks.is_empty());
        assert!(c.min_debt_usd > 0 && c.cold_max_age_seconds >= c.max_account_age_seconds);
        assert!(c.max_unknown_per_pass > 0 && c.max_unknown_per_pass <= 10_000);
        assert!(c.recheck_batch > 1 && c.reconcile_batch > 0);
    }

    #[test]
    fn shipped_base_config_runs_morpho_on_public_endpoints_only() {
        let c = ChainConfig::load(&repo_config().with_file_name("base.toml")).unwrap();
        assert_eq!(c.chain_id, 8453);
        assert!(c.sequencer_feed.is_none() && !c.atlas.enabled);
        assert!(!c.http_fallbacks.is_empty() && !c.ws_fallbacks.is_empty());
        let l = c.liquidation.as_ref().unwrap();
        assert!(l.enabled);
        let m = c.morpho.as_ref().unwrap();
        // The scan endpoint (Pocket) refuses getLogs over 50,000 blocks.
        assert!(m.enabled && m.scan_step <= 50_000);
        assert!(c.aave.is_none(), "Base Aave is won by SVR auctions");
        assert_eq!(c.symbol(c.token("cbBTC").unwrap()), "cbBTC");
    }

    #[test]
    fn shipped_arbitrum_config_is_valid() {
        let c = ChainConfig::load(&repo_config()).unwrap();
        assert_eq!(c.chain_id, 42161);
        assert!(!c.pairs.is_empty() && !c.dexes.is_empty());
        assert!(c.token("WETH").is_ok());
        assert_eq!(c.symbol(c.token("USDC").unwrap()), "USDC");
        assert!(c.liquidation.as_ref().is_some_and(|l| l.enabled));
        assert!(
            c.aave.is_some() && c.compound_v3.is_some() && c.morpho.is_some() && c.fluid.is_some()
        );
        assert_eq!(c.pair_addresses().unwrap().len(), c.pairs.len());
        assert_eq!(c.pga.max_tip_usd_per_tx, 1.0);
        assert!(c.pga.feed_min_bot_tip_gwei >= 0.02);
        assert!(
            !c.batch.enabled,
            "batching ships off until the executor with `batch` is deployed"
        );
        assert!(
            c.uniswap_v4.as_ref().is_some_and(|v| v.presimulate),
            "V4 routes are simulated before sending"
        );
        assert!(
            c.ticks.enabled && c.ticks.window_ticks > 0,
            "tick-aware pricing is on"
        );
        assert!(
            c.risk.arb_presimulate,
            "the pre-send simulation stays configured; fast_send bypasses it while there is no own node"
        );
        assert!(
            !c.risk.fast_send,
            "fast_send is off: live sends are checked before sending (DECISIONS #93)"
        );
        assert!(
            c.pga.market_quantile < c.pga.competition_quantile && c.pga.route_min_observations >= 1,
            "unknown routes are priced from the market median, known ones from their own winners"
        );
        assert!(c.max_cycles > 20_000, "long-tail cycles need headroom");
        assert!(c.auto_discovery.as_ref().is_some_and(|a| a.cross_pairs > 0));
        assert!(
            c.uniswap_v4
                .as_ref()
                .is_some_and(|v| v.enabled && !v.pool_manager.is_zero())
        );
        assert!(
            c.executor_v4 && c.executor_batch,
            "the redeployed executor has V4 hops and batch"
        );
        assert!(!c.atlas.bid, "bidding needs a funded solver: it ships off");
        assert!(
            c.atlas.solver.is_none(),
            "no solver address ships in the repo"
        );
    }

    #[test]
    fn atlas_bid_configuration_is_rejected_until_settlement_is_verified() {
        let path = std::env::temp_dir().join(format!("atlas-config-{}.toml", std::process::id()));
        let raw = std::fs::read_to_string(repo_config())
            .unwrap()
            .replace("bid = false", "bid = true");
        std::fs::write(&path, raw).unwrap();
        assert!(
            ChainConfig::load(&path)
                .unwrap_err()
                .to_string()
                .contains("escrow")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_token_in_pairs_is_rejected() {
        let raw = std::fs::read_to_string(repo_config()).unwrap();
        let bad = raw.replacen("[\"WETH\", \"USDC\"]", "[\"WETH\", \"NOPE\"]", 1);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.toml");
        std::fs::write(&p, bad).unwrap();
        let err = ChainConfig::load(&p).unwrap_err().to_string();
        assert!(
            err.contains("NOPE"),
            "error should name the bad symbol: {err}"
        );
    }

    #[test]
    fn nonfinite_and_negative_money_configuration_is_rejected() {
        let raw = std::fs::read_to_string(repo_config()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("invalid-numbers.toml");
        for value in ["nan", "inf", "-inf", "-1", "-0.1"] {
            std::fs::write(&p, format!("{raw}\n[audit-invalid]\namount = {value}\n")).unwrap();
            let error = ChainConfig::load(&p).unwrap_err().to_string();
            assert!(error.contains("nonnegative"), "{value}: {error}");
        }
    }

    #[test]
    fn unknown_symbol_falls_back_to_address() {
        let c = ChainConfig::load(&repo_config()).unwrap();
        let a = Address::with_last_byte(0x42);
        assert_eq!(c.symbol(a), a.to_string());
    }

    #[test]
    fn other_chains_never_read_arbitrums_unprefixed_endpoints_or_executor() {
        let env = |k: &str| {
            match k {
                "HTTP_URL" => Some("https://arb.example/key"),
                "EXECUTOR_ADDRESS" => Some("0x17357E3300000000000000000000000000000000"),
                "PROFIT_RECEIVER" => Some("0x1111111111111111111111111111111111111111"),
                "BASE_WS_URL" => Some(" wss://base.example "),
                "BASE_HTTP_URL" => Some("  "),
                _ => None,
            }
            .map(str::to_string)
        };
        assert_eq!(
            chain_var("ARBITRUM", "HTTP_URL", &env).as_deref(),
            Some("https://arb.example/key")
        );
        assert_eq!(chain_var("BASE", "HTTP_URL", &env), None);
        assert_eq!(chain_var("BASE", "EXECUTOR_ADDRESS", &env), None);
        assert!(chain_var("ARBITRUM", "EXECUTOR_ADDRESS", &env).is_some());
        assert!(chain_var("BASE", "PROFIT_RECEIVER", &env).is_some());
        assert_eq!(
            chain_var("BASE", "WS_URL", &env).as_deref(),
            Some("wss://base.example")
        );
    }

    #[test]
    fn secrets_debug_never_prints_the_private_key() {
        let s = Secrets {
            http_url: "http://x".into(),
            ws_url: "ws://x".into(),
            private_key: Some("0xdeadbeef".into()),
            profit_receiver: None,
            executor: None,
            extra_http_urls: vec!["http://y".into()],
            extra_ws_urls: vec![],
            sweep_urls: vec!["https://sweep.example/keyed0123456789abcdef".into()],
            presim_urls: vec![],
            fast_feed_key: Some("ffkey0123456789abcdef".into()),
        };
        let shown = format!("{s:?}");
        assert!(!shown.contains("deadbeef"));
        assert!(
            !shown.contains("keyed0123456789abcdef"),
            "keyed URLs never printed"
        );
        assert!(
            !shown.contains("ffkey0123456789abcdef"),
            "Fast Feed key never printed"
        );
        assert!(shown.contains("redacted"));
        assert_eq!(
            s.http_urls(),
            vec!["http://x".to_string(), "http://y".to_string()]
        );
        assert_eq!(s.ws_urls(), vec!["ws://x".to_string()]);
    }
}
