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

#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub balancer_vault: Address,
    /// Extra endpoint that receives raw txs directly (Arbitrum sequencer).
    pub sequencer_rpc: Option<String>,
    pub min_pool_liquidity_usd: f64,
    pub gas_limit_estimate: u64,
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
        })
    }
}
