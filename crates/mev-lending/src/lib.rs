//! Protocol-neutral liquidation interface.
//!
//! Every lending venue (Aave, Silo, Compound III, Fluid, Morpho, ...) implements
//! [`LiquidationAdapter`]. The generic runner (`mev-liquidate`) drives indexing, health sweeps,
//! at-risk polling, and sequencer-triggered re-checks through this trait; the engine turns a
//! [`Plan`] into an executor transaction. Adapters own protocol rules and encoding; nothing
//! protocol-specific leaks out of their crate.

use alloy::primitives::Address;
use async_trait::async_trait;
pub use mev_types::{AccountRef, Plan, ProtocolCall};
use serde::Serialize;

/// Stable adapter id, e.g. `"aave-v3"`.
pub type ProtocolId = &'static str;

/// Whether an adapter may be used for live liquidations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AdapterStatus {
    /// Verified on-chain and enabled.
    Live,
    /// Contracts exist but the integration has not been verified end to end yet.
    AuditPending(&'static str),
    /// Not usable on this chain (paused, deprecated, or not deployed).
    Inactive(&'static str),
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Health {
    pub account: AccountRef,
    /// < 1.0 means liquidatable (protocols with other scales normalize to this).
    pub health_factor: f64,
    pub debt_usd: f64,
    pub collateral_usd: f64,
}

impl Health {
    pub fn liquidatable(&self) -> bool {
        self.health_factor < 1.0
    }
}

/// What the liquidation watchlist can say about a position beyond its health, from state the
/// last sweep cached (no RPC). Whatever an adapter can't know stays `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WatchDetail {
    /// The position's single collateral and debt assets, when it has one of each (Morpho).
    pub collateral: Option<Address>,
    pub debt: Option<Address>,
    /// Collateral price now and the price at which the position becomes liquidatable, in USD per
    /// whole token, with every other price unchanged.
    pub collateral_price_usd: Option<f64>,
    pub liquidation_price_usd: Option<f64>,
    /// Debt one liquidation may repay (USD) and the bonus on it (0.10 = 10%).
    pub max_repay_usd: Option<f64>,
    pub bonus_rate: Option<f64>,
}

/// RPC handles an adapter may use. Fast = low-latency (paid/WS-backed); scan = complete wide-range
/// getLogs; sweep = heavy eth_call batches. Keeps the paid quota for latency-critical reads.
#[derive(Clone)]
pub struct AdapterRpc {
    pub fast: alloy::providers::DynProvider,
    pub scan: alloy::providers::DynProvider,
    pub sweep: alloy::providers::DynProvider,
}

/// Invert `aggregator -> price sources` (from `mev_oracle::resolve_aggregators`) into
/// `aggregator -> keys`, where `keys_of(source)` names what each source prices (assets, markets).
pub fn feed_index<K: Eq + std::hash::Hash + Copy>(
    aggregators: &std::collections::HashMap<Address, Vec<Address>>,
    keys_of: impl Fn(Address) -> Vec<K>,
) -> std::collections::HashMap<Address, std::collections::HashSet<K>> {
    aggregators
        .iter()
        .map(|(agg, sources)| (*agg, sources.iter().flat_map(|s| keys_of(*s)).collect()))
        .collect()
}

#[async_trait]
pub trait LiquidationAdapter: Send + Sync {
    fn id(&self) -> ProtocolId;

    fn status(&self) -> AdapterStatus;

    /// Advance account discovery (bounded work per call). Returns true once history is complete.
    async fn index(&mut self) -> anyhow::Result<bool>;

    /// All accounts currently known (may include ones without debt).
    fn accounts(&self) -> Vec<AccountRef>;

    /// Health of `accounts`. `fast` selects the low-latency RPC (at-risk polling) vs the sweep RPC.
    /// One answer per account whose state was read, including accounts that owe nothing (zero
    /// debt, infinite health factor): the runner treats a sweep with missing answers as failed.
    /// Leave an account out only when its read failed.
    async fn health(&self, accounts: &[AccountRef], fast: bool) -> anyhow::Result<Vec<Health>>;

    /// Build a liquidation for an unhealthy account using current protocol state and prices.
    /// `None` when nothing worth taking (bad debt, dust, below close-factor limits).
    async fn plan(&self, health: &Health, executor: Address) -> anyhow::Result<Option<Plan>>;

    /// Price-feed contracts whose updates can change account health. The runner watches the
    /// sequencer feed for transactions to these and re-checks at-risk accounts immediately.
    fn price_feeds(&self) -> Vec<Address>;

    /// Whether an update of `feed` (one of `price_feeds`) can move `account`'s health. Lets the
    /// runner re-poll only exposed accounts after a feed update. Must answer `true` when unsure.
    fn affected(&self, _feed: Address, _account: &AccountRef) -> bool {
        true
    }

    /// Backrun hook: `feed` (one of `price_feeds`) is about to report `answer` — seen in the
    /// sequencer feed, not yet in a block. Return plans for positions that price makes
    /// liquidatable, from cached state only (no RPC: this runs on the latency-critical path).
    /// They can't be simulated (the chain doesn't have the price yet); the executor's on-chain
    /// profit check is the guard. Default: no prediction.
    fn predict(&self, _feed: Address, _answer: f64, _executor: Address) -> Vec<Plan> {
        Vec::new()
    }

    /// How long after a price round's timestamp the protocol starts using it. Moonwell's
    /// Chainlink OEV wrappers keep serving the previous round for `maxRoundDelay` (10 s on Base)
    /// unless a liquidator pays to unlock it early, so re-polls must land after that.
    fn feed_unlock_delay(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }

    /// Contracts whose transactions can add the cash a stuck liquidation needs (Moonwell
    /// mTokens: a mint or repay into a collateral market whose cash is lent out). When one is
    /// seen in the sequencer feed, the runner re-plans this venue's liquidatable accounts past
    /// their re-plan cooldown. Default: none.
    fn cash_markets(&self) -> Vec<Address> {
        Vec::new()
    }

    /// Watchlist details for `health`, a position the last sweep read. Cached state only.
    fn watch_detail(&self, _health: &Health) -> WatchDetail {
        WatchDetail::default()
    }

    /// `predict` with the report's exact integer answer. `Some`: this adapter predicts `feed`
    /// exactly (possibly no plans, where it can't predict exactly rather than estimate); `None`:
    /// no exact model for `feed`, and the runner falls back to `predict`.
    fn predict_exact(
        &self,
        _feed: Address,
        _answer: alloy::primitives::U256,
        _executor: Address,
    ) -> Option<Vec<Plan>> {
        None
    }

    /// Contracts the executor must allowlist (call targets and approval spenders).
    fn targets(&self) -> Vec<Address>;

    /// Tokens that can be debt or collateral here (so swap routes for them get discovered).
    fn assets(&self) -> Vec<Address>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_index_maps_aggregators_to_what_their_sources_price() {
        let (agg_eth, agg_btc) = (Address::with_last_byte(0xe0), Address::with_last_byte(0xb0));
        let (src_eth, src_steth, src_btc) = (
            Address::with_last_byte(1),
            Address::with_last_byte(2),
            Address::with_last_byte(3),
        );
        let aggs = std::collections::HashMap::from([
            (agg_eth, vec![src_eth, src_steth]),
            (agg_btc, vec![src_btc]),
        ]);
        // Source -> markets it prices; the stETH source prices two markets.
        let idx = feed_index(&aggs, |s| match s {
            s if s == src_eth => vec![10u64],
            s if s == src_steth => vec![11, 12],
            _ => vec![20],
        });
        assert_eq!(idx[&agg_eth], [10, 11, 12].into_iter().collect());
        assert_eq!(idx[&agg_btc], [20].into_iter().collect());
    }
}
