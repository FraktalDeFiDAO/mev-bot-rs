//! Per-chain TOML config plus secrets from the environment (never from files in the repo).

use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, anyhow};
use mev_types::Address;
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
}

/// One Compound III (Comet) market.
#[derive(Debug, Clone, Deserialize)]
pub struct CometConfig {
    pub address: Address,
    /// Chainlink feed converting the Comet's price unit to USD (only for non-USD-priced Comets,
    /// e.g. the WETH Comet prices in ETH).
    pub usd_feed: Option<Address>,
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
    /// Quantile of the bot-like feed tips used as the competition estimate.
    pub feed_quantile: f64,
    /// Quantile of recently observed winning tips used as the competition estimate.
    pub competition_quantile: f64,
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
            feed_quantile: 0.9,
            competition_quantile: 0.9,
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
    /// Send ready trades together as one transaction (needs `executor_batch`). Off by default.
    #[serde(default)]
    pub batch: BatchConfig,
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
    pub liquidation: Option<LiquidationConfig>,
    pub aave: Option<AaveConfig>,
    pub compound_v3: Option<CompoundV3Config>,
    pub morpho: Option<MorphoConfig>,
    pub fluid: Option<FluidConfig>,
    #[serde(default)]
    pub presim: Option<PresimConfig>,
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
        let cfg: ChainConfig =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
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
            .collect()
    }

    /// Primary WS URL followed by the extras.
    pub fn ws_urls(&self) -> Vec<String> {
        std::iter::once(self.ws_url.clone())
            .chain(self.extra_ws_urls.iter().cloned())
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
        let var = |k: &str| std::env::var(format!("{prefix}_{k}")).or_else(|_| std::env::var(k));
        let addr = |k: &str| -> Result<Option<Address>> {
            match std::env::var(k) {
                Ok(v) if !v.trim().is_empty() => Ok(Some(
                    v.trim()
                        .parse()
                        .with_context(|| format!("{k} is not an address"))?,
                )),
                _ => Ok(None),
            }
        };
        Ok(Self {
            http_url: var("HTTP_URL").with_context(|| format!("{prefix}_HTTP_URL not set"))?,
            ws_url: var("WS_URL").with_context(|| format!("{prefix}_WS_URL not set"))?,
            private_key: std::env::var("PRIVATE_KEY")
                .ok()
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
            fast_feed_key: var("FAST_FEED_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
        })
    }
}

fn default_max_concurrent_sends() -> u32 {
    1
}

fn default_max_bonus_usd() -> f64 {
    50_000.0
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
        assert!(c.max_cycles > 20_000, "long-tail cycles need headroom");
        assert!(c.auto_discovery.as_ref().is_some_and(|a| a.cross_pairs > 0));
        assert!(!c.atlas.bid, "bidding needs a funded solver: it ships off");
        assert!(
            c.atlas.solver.is_none(),
            "no solver address ships in the repo"
        );
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
    fn unknown_symbol_falls_back_to_address() {
        let c = ChainConfig::load(&repo_config()).unwrap();
        let a = Address::with_last_byte(0x42);
        assert_eq!(c.symbol(a), a.to_string());
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
