//! Morpho Blue liquidation adapter.
//!
//! - Accounts: `(market id, borrower)` from `Borrow` events (shared index, bytes32 market keys).
//!   `AccountRef { market: morpho, account: borrower, sub_id: first 8 bytes of the market id }`.
//! - Health: Morpho's own condition (`math`), using the market oracle (1e36-scaled price).
//! - Plan: one `liquidate(marketParams, borrower, seizedAssets, 0, "")`, approving the loan token
//!   to Morpho (it pulls repayment after sending collateral). No close factor.
//! - USD values: loan-token prices from `usd_oracle` (the Aave oracle on Arbitrum). Markets whose
//!   loan token it doesn't price report $0 and are filtered out by the runner's minimums.

pub mod math;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};

use alloy::{
    primitives::{Address, B256, U256},
    providers::DynProvider,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::Result;
use async_trait::async_trait;
use mev_bindings::{IAaveOracle, IERC20, IPriceSource};
use mev_config::MorphoConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId, WatchDetail,
};
use mev_lending_index::{BackgroundIndex, EventIndex, IndexConfig, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::{info, warn};

pub const ID: ProtocolId = "morpho-blue";

sol! {
    #[sol(rpc)]
    interface IMorpho {
        struct MarketParams { address loanToken; address collateralToken; address oracle; address irm; uint256 lltv; }
        function idToMarketParams(bytes32 id) external view returns (address loanToken, address collateralToken, address oracle, address irm, uint256 lltv);
        function position(bytes32 id, address user) external view returns (uint256 supplyShares, uint128 borrowShares, uint128 collateral);
        function market(bytes32 id) external view returns (uint128 totalSupplyAssets, uint128 totalSupplyShares, uint128 totalBorrowAssets, uint128 totalBorrowShares, uint128 lastUpdate, uint128 fee);
        function liquidate(MarketParams memory marketParams, address borrower, uint256 seizedAssets, uint256 repaidShares, bytes memory data) external returns (uint256, uint256);
        event Borrow(bytes32 indexed id, address caller, address indexed onBehalf, address indexed receiver, uint256 assets, uint256 shares);
    }

    #[sol(rpc)]
    interface IMorphoOracle {
        function price() external view returns (uint256);
        function SCALE_FACTOR() external view returns (uint256);
        function BASE_VAULT() external view returns (address);
        function QUOTE_VAULT() external view returns (address);
    }
}

/// A `MorphoChainlinkOracleV2` whose price can be recomputed exactly from Chainlink answers:
/// no ERC-4626 vaults (their conversion moves every block) and only plain Chainlink proxies
/// (`aggregator()` set; the proxy answer is the aggregator's). `price()` is
/// `SCALE_FACTOR.mulDiv(base1 × base2, quote1 × quote2)` over these aggregators' answers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExactOracle {
    scale: U256,
    base: Vec<Address>,
    quote: Vec<Address>,
}

impl ExactOracle {
    fn price(&self, answers: &HashMap<Address, U256>) -> Option<U256> {
        let prod = |aggs: &[Address]| {
            aggs.iter()
                .try_fold(U256::from(1u8), |acc, a| acc.checked_mul(*answers.get(a)?))
        };
        let (num, den) = (prod(&self.base)?, prod(&self.quote)?);
        if den.is_zero() {
            return None;
        }
        Some(self.scale.checked_mul(num)? / den)
    }
}

/// An at-risk position kept for prediction: estimates (raw units as f64) for the float path and
/// the exact collateral and borrow shares for the exact one.
#[derive(Debug, Clone, Copy)]
struct SnapPos {
    account: AccountRef,
    collateral: f64,
    borrowed: f64,
    collateral_raw: U256,
    borrow_shares: U256,
}

#[derive(Debug, Clone, Copy)]
struct Params {
    loan: Address,
    collateral: Address,
    oracle: Address,
    irm: Address,
    lltv: U256,
}

/// Live state of one market needed for health/sizing.
#[derive(Debug, Clone, Copy)]
struct MarketState {
    price: f64,
    total_borrow_assets: f64,
    total_borrow_shares: f64,
    loan_usd: f64,
    loan_decimals: u8,
    /// The same oracle price and market totals as exact integers; amounts sent on-chain use these.
    price_raw: U256,
    total_borrow_assets_raw: U256,
    total_borrow_shares_raw: U256,
}

pub struct MorphoAdapter {
    cfg: MorphoConfig,
    rpc: AdapterRpc,
    index: BackgroundIndex,
    params: Mutex<HashMap<B256, Params>>,
    decimals: Mutex<HashMap<Address, u8>>,
    /// Aggregator -> short ids of markets whose oracle reads it.
    feeds: HashMap<Address, HashSet<u64>>,
    /// Aggregator -> (market short id, exponent) for MorphoChainlinkOracleV2 markets: the oracle
    /// price is base1·base2 / (quote1·quote2), so a base feed moves it by ratio^+1 and a quote feed
    /// by ratio^-1. Markets with other oracle types are not predicted.
    feed_signs: HashMap<Address, Vec<(u64, f64)>>,
    /// Oracles (by address) whose price can be recomputed exactly from Chainlink answers.
    exact_oracles: HashMap<Address, ExactOracle>,
    /// Backrun prediction state, refreshed by sweeps and moved by each predicted update.
    snapshot: Mutex<Snapshot>,
    /// Markets (short ids) whose state could not be read on the last sweep (oracle reverts, loan
    /// decimals unknown): their borrowers are left out of `accounts()`, so one broken market does
    /// not fail every sweep. Re-probed on each sweep; logged when the set changes.
    unpriced: Mutex<HashSet<u64>>,
    /// Markets the price feeds were last resolved for, and when: indexing keeps finding markets
    /// (a fresh start knows none), so feeds are re-resolved as it does (at most every
    /// `FEED_REFRESH`).
    feeds_markets: usize,
    feeds_at: Option<std::time::Instant>,
}

/// Re-resolve price feeds at most this often while indexing finds new markets.
const FEED_REFRESH: std::time::Duration = std::time::Duration::from_secs(300);

/// Feed index (aggregator -> market short ids), prediction signs, exactly predictable oracles.
type Feeds = (
    HashMap<Address, HashSet<u64>>,
    HashMap<Address, Vec<(u64, f64)>>,
    HashMap<Address, ExactOracle>,
);

#[derive(Default)]
struct Snapshot {
    /// Market short id -> (id, state).
    markets: HashMap<u64, (B256, MarketState)>,
    /// At-risk positions.
    positions: Vec<SnapPos>,
    /// Aggregator -> last known answer.
    answers: HashMap<Address, f64>,
    /// Aggregator -> last known answer, exact (aggregators of `exact_oracles`).
    answers_exact: HashMap<Address, U256>,
}

/// Sweeps keep positions below this health factor for prediction.
const SNAPSHOT_HF: f64 = 1.10;
/// Predicted health below this triggers a backrun (margin against rounding).
const PREDICT_HF_MAX: f64 = 0.9995;

/// Short id stored in `AccountRef::sub_id`.
pub fn short_id(id: &B256) -> u64 {
    u64::from_be_bytes(id[..8].try_into().unwrap())
}

impl MorphoAdapter {
    pub async fn new(cfg: MorphoConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let index = BackgroundIndex::new(EventIndex::open(IndexConfig {
            addresses: vec![cfg.morpho],
            topics: vec![IMorpho::Borrow::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        }));
        let me = Self {
            cfg,
            rpc,
            index,
            params: Mutex::new(HashMap::new()),
            decimals: Mutex::new(HashMap::new()),
            feeds: HashMap::new(),
            feed_signs: HashMap::new(),
            exact_oracles: HashMap::new(),
            snapshot: Mutex::new(Snapshot::default()),
            unpriced: Mutex::new(HashSet::new()),
            feeds_markets: 0,
            feeds_at: None,
        };
        let ids: Vec<B256> = me.market_ids();
        let (feeds, feed_signs, exact_oracles) = me.resolve_feeds(&ids).await?;
        info!(
            protocol = ID,
            markets = ids.len(),
            borrowers_cached = me.index.len(),
            predictable_feeds = feed_signs.len(),
            exact_oracles = exact_oracles.len(),
            "adapter ready"
        );
        Ok(Self {
            feeds,
            feed_signs,
            exact_oracles,
            feeds_markets: ids.len(),
            feeds_at: Some(std::time::Instant::now()),
            ..me
        })
    }

    /// Price feeds of the markets `ids`: the aggregators behind each market's oracle, and the
    /// prediction signs of the Chainlink-oracle markets. Feeds that hold public updates back
    /// (Chainlink SVR) stay watched for re-polls but are never predicted: a backrun of their
    /// public report would see the old price.
    async fn resolve_feeds(&self, ids: &[B256]) -> Result<Feeds> {
        self.load_params(ids).await?;
        let market_oracles: Vec<(u64, Address)> = self
            .params
            .lock()
            .unwrap()
            .iter()
            .map(|(id, p)| (short_id(id), p.oracle))
            .collect();
        let oracles: Vec<Address> = market_oracles.iter().map(|(_, o)| *o).collect();
        let aggregators = mev_oracle::resolve_aggregators(&self.rpc.fast, &oracles).await?;
        let feeds = mev_lending::feed_index(&aggregators, |oracle| {
            market_oracles
                .iter()
                .filter(|(_, o)| *o == oracle)
                .map(|(id, _)| *id)
                .collect()
        });
        let mut feed_signs = feed_signs(&self.rpc.fast, &market_oracles).await;
        let signed: Vec<Address> = feed_signs.keys().copied().collect();
        match mev_oracle::held_aggregators(&self.rpc.fast, &signed).await {
            Ok(held) => feed_signs.retain(|a, _| !held.contains(a)),
            Err(e) => warn!(protocol = ID, error = %format!("{e:#}"), "SVR feed check failed"),
        }
        let mut unique: Vec<Address> = oracles.clone();
        unique.sort_unstable();
        unique.dedup();
        let exact = exact_oracles(&self.rpc.fast, &unique).await;
        Ok((feeds, feed_signs, exact))
    }

    /// Track a (market, borrower) learned elsewhere (another indexer, a liquidation feed, tests).
    pub fn track(&mut self, id: B256, borrower: Address) {
        self.index.insert((id, borrower));
    }

    fn market_ids(&self) -> Vec<B256> {
        self.index
            .accounts()
            .iter()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Fetch and cache market params for ids not seen yet, then the decimals of every token those
    /// markets use that we don't have (a failed `decimals()` is retried on the next call, never
    /// guessed).
    async fn load_params(&self, ids: &[B256]) -> Result<()> {
        let missing: Vec<B256> = {
            let known = self.params.lock().unwrap();
            ids.iter()
                .filter(|i| !known.contains_key(*i))
                .copied()
                .collect()
        };
        if !missing.is_empty() {
            let calls: Vec<_> = missing
                .iter()
                .map(|id| call(self.cfg.morpho, IMorpho::idToMarketParamsCall { id: *id }))
                .collect();
            let res = multicall(&self.rpc.fast, &calls, 200).await?;
            let mut known = self.params.lock().unwrap();
            for (id, r) in missing.iter().zip(res.iter()) {
                if let Some(p) = decode::<IMorpho::idToMarketParamsCall>(r) {
                    known.insert(
                        *id,
                        Params {
                            loan: p.loanToken,
                            collateral: p.collateralToken,
                            oracle: p.oracle,
                            irm: p.irm,
                            lltv: p.lltv,
                        },
                    );
                }
            }
        }
        let need: Vec<Address> = {
            let known = self.params.lock().unwrap();
            let dec = self.decimals.lock().unwrap();
            let mut t: Vec<Address> = ids
                .iter()
                .filter_map(|id| known.get(id))
                .flat_map(|p| [p.loan, p.collateral])
                .filter(|t| !dec.contains_key(t))
                .collect();
            t.sort();
            t.dedup();
            t
        };
        if need.is_empty() {
            return Ok(());
        }
        let calls: Vec<_> = need
            .iter()
            .map(|t| call(*t, IERC20::decimalsCall {}))
            .collect();
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        let mut dec = self.decimals.lock().unwrap();
        for (t, r) in need.iter().zip(res.iter()) {
            if let Some(d) = decode::<IERC20::decimalsCall>(r) {
                dec.insert(*t, d);
            }
        }
        Ok(())
    }

    async fn market_states(
        &self,
        p: &DynProvider,
        ids: &[B256],
    ) -> Result<HashMap<B256, MarketState>> {
        self.load_params(ids).await?;
        let params: Vec<(B256, Params)> = {
            let known = self.params.lock().unwrap();
            ids.iter()
                .filter_map(|id| known.get(id).map(|p| (*id, *p)))
                .collect()
        };
        let mut calls = Vec::with_capacity(params.len() * 3);
        for (id, pm) in &params {
            calls.push(call(self.cfg.morpho, IMorpho::marketCall { id: *id }));
            calls.push(call(pm.oracle, IMorphoOracle::priceCall {}));
            calls.push(call(
                self.cfg.usd_oracle,
                IAaveOracle::getAssetPriceCall { asset: pm.loan },
            ));
        }
        let res = multicall(p, &calls, 200).await?;
        let dec = self.decimals.lock().unwrap();
        Ok(params
            .iter()
            .enumerate()
            .filter_map(|(i, (id, pm))| {
                let m = decode::<IMorpho::marketCall>(&res[3 * i])?;
                let price = decode::<IMorphoOracle::priceCall>(&res[3 * i + 1])?;
                // Unknown loan decimals: skip the market until they are known, never assume 18.
                let loan_decimals = *dec.get(&pm.loan)?;
                let usd = decode::<IAaveOracle::getAssetPriceCall>(&res[3 * i + 2])
                    .map(f64::from)
                    .unwrap_or(0.0)
                    / 1e8;
                Some((
                    *id,
                    MarketState {
                        price: f64::from(price),
                        total_borrow_assets: m.totalBorrowAssets as f64,
                        total_borrow_shares: m.totalBorrowShares as f64,
                        loan_usd: usd,
                        loan_decimals,
                        price_raw: price,
                        total_borrow_assets_raw: U256::from(m.totalBorrowAssets),
                        total_borrow_shares_raw: U256::from(m.totalBorrowShares),
                    },
                ))
            })
            .collect())
    }

    fn lltv(&self, id: &B256) -> f64 {
        self.params
            .lock()
            .unwrap()
            .get(id)
            .map(|p| f64::from(p.lltv) / 1e18)
            .unwrap_or(0.0)
    }
}

fn loan_usd(raw: f64, s: &MarketState) -> f64 {
    raw / 10f64.powi(s.loan_decimals as i32) * s.loan_usd
}

#[async_trait]
impl LiquidationAdapter for MorphoAdapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        // Borrow(id indexed, caller, onBehalf indexed, receiver indexed, ...): debt is onBehalf's.
        let done = self.index.run(&self.rpc.scan, move |l| {
            match (l.topics().get(1), topic_address(l, 2)) {
                (Some(id), Some(user)) => vec![(*id, user)],
                _ => vec![],
            }
        });
        let ids = self.market_ids();
        let due = self.feeds_at.is_none_or(|t| t.elapsed() >= FEED_REFRESH);
        if ids.len() > self.feeds_markets && due {
            self.feeds_at = Some(std::time::Instant::now());
            match self.resolve_feeds(&ids).await {
                Ok((feeds, signs, exact)) => {
                    info!(
                        protocol = ID,
                        markets = ids.len(),
                        feeds = feeds.len(),
                        predictable_feeds = signs.len(),
                        "price feeds refreshed"
                    );
                    self.feeds = feeds;
                    self.feed_signs = signs;
                    self.exact_oracles = exact;
                    self.feeds_markets = ids.len();
                }
                Err(e) => {
                    warn!(protocol = ID, error = %format!("{e:#}"), "price feed refresh failed")
                }
            }
        }
        Ok(done)
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let morpho = self.cfg.morpho;
        let unpriced = self.unpriced.lock().unwrap().clone();
        self.index
            .accounts()
            .iter()
            .map(|(id, u)| AccountRef {
                market: morpho,
                account: *u,
                sub_id: short_id(id),
            })
            .filter(|a| !unpriced.contains(&a.sub_id))
            .collect()
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let p = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let by_short: HashMap<u64, B256> = self
            .market_ids()
            .into_iter()
            .map(|id| (short_id(&id), id))
            .collect();
        let pairs: Vec<(AccountRef, B256)> = accounts
            .iter()
            .filter_map(|a| by_short.get(&a.sub_id).map(|id| (*a, *id)))
            .collect();
        let mut ids: HashSet<B256> = pairs.iter().map(|(_, id)| *id).collect();
        let was_unpriced = self.unpriced.lock().unwrap().clone();
        if !fast {
            // Re-probe markets left out last time so they rejoin once readable.
            ids.extend(was_unpriced.iter().filter_map(|s| by_short.get(s)));
        }
        let ids: Vec<B256> = ids.into_iter().collect();
        let states = self.market_states(p, &ids).await?;
        if !fast {
            let now: HashSet<u64> = ids
                .iter()
                .filter(|id| !states.contains_key(*id))
                .map(short_id)
                .collect();
            if now != was_unpriced {
                warn!(
                    protocol = ID,
                    unpriced = now.len(),
                    markets = ids.len(),
                    "markets whose state cannot be read are left out of sweeps"
                );
            }
            *self.unpriced.lock().unwrap() = now;
        }
        let calls: Vec<_> = pairs
            .iter()
            .map(|(a, id)| {
                call(
                    self.cfg.morpho,
                    IMorpho::positionCall {
                        id: *id,
                        user: a.account,
                    },
                )
            })
            .collect();
        let res = multicall(p, &calls, 200).await?;
        let mut at_risk: Vec<SnapPos> = Vec::new();
        let out: Vec<Health> = pairs
            .iter()
            .zip(res.iter())
            .filter_map(|((a, id), r)| {
                let pos = decode::<IMorpho::positionCall>(r)?;
                if pos.borrowShares == 0 {
                    // Repaid: reported (the runner requires one answer per account and makes
                    // the account dormant), not dropped.
                    return Some(Health {
                        account: *a,
                        health_factor: f64::INFINITY,
                        debt_usd: 0.0,
                        collateral_usd: 0.0,
                    });
                }
                let s = states.get(id)?;
                let borrowed = math::borrow_assets(
                    pos.borrowShares as f64,
                    s.total_borrow_assets,
                    s.total_borrow_shares,
                );
                let collateral = pos.collateral as f64;
                let coll_in_loan = collateral * s.price / math::ORACLE_PRICE_SCALE;
                let health_factor =
                    math::health_factor(collateral, borrowed, s.price, self.lltv(id));
                if !fast && health_factor < SNAPSHOT_HF {
                    at_risk.push(SnapPos {
                        account: *a,
                        collateral,
                        borrowed,
                        collateral_raw: U256::from(pos.collateral),
                        borrow_shares: U256::from(pos.borrowShares),
                    });
                }
                Some(Health {
                    account: *a,
                    health_factor,
                    debt_usd: loan_usd(borrowed, s),
                    collateral_usd: loan_usd(coll_in_loan, s),
                })
            })
            .collect();
        if !fast {
            // Prediction snapshot (sweep RPC; direct eth_calls for access-controlled aggregators).
            let aggs: Vec<Address> = self.feed_signs.keys().copied().collect();
            let answers = mev_oracle::latest_answers(p, &aggs).await;
            let mut exact_aggs: Vec<Address> = self
                .exact_oracles
                .values()
                .flat_map(|o| o.base.iter().chain(&o.quote).copied())
                .collect();
            exact_aggs.sort_unstable();
            exact_aggs.dedup();
            let answers_exact = mev_oracle::latest_answers_exact(p, &exact_aggs).await;
            let markets = states
                .into_iter()
                .map(|(id, st)| (short_id(&id), (id, st)))
                .collect();
            *self.snapshot.lock().unwrap() = Snapshot {
                markets,
                positions: at_risk,
                answers,
                answers_exact,
            };
        }
        Ok(out)
    }

    async fn plan(&self, h: &Health, _executor: Address) -> Result<Option<Plan>> {
        let Some(id) = self
            .market_ids()
            .into_iter()
            .find(|id| short_id(id) == h.account.sub_id)
        else {
            return Ok(None);
        };
        let Some(pm) = self.params.lock().unwrap().get(&id).copied() else {
            return Ok(None);
        };
        // Each early return names its reason: an unplanned liquidatable account (`liq_no_plan`) is
        // otherwise indistinguishable from a race lost before the re-read.
        let no_plan = |reason: &str| {
            info!(protocol = ID, account = %h.account.account, market = %id, hf = h.health_factor, reason, "no plan");
            Ok(None)
        };
        let states = self.market_states(&self.rpc.fast, &[id]).await?;
        let Some(s) = states.get(&id) else {
            return no_plan("market state unreadable");
        };
        let res = multicall(
            &self.rpc.fast,
            &[call(
                self.cfg.morpho,
                IMorpho::positionCall {
                    id,
                    user: h.account.account,
                },
            )],
            1,
        )
        .await?;
        let Some(pos) = decode::<IMorpho::positionCall>(&res[0]) else {
            return no_plan("position unreadable");
        };
        // Health and sizing from the exact integer reads (the floats above only nominated this account).
        let collateral = U256::from(pos.collateral);
        let Some(borrowed) = math::exact::borrow_assets_up(
            U256::from(pos.borrowShares),
            s.total_borrow_assets_raw,
            s.total_borrow_shares_raw,
        ) else {
            return no_plan("borrow assets overflow");
        };
        match math::exact::is_healthy(collateral, borrowed, s.price_raw, pm.lltv) {
            Some(false) => {}
            Some(true) if pos.borrowShares == 0 => {
                return no_plan("repaid or liquidated before the re-read");
            }
            Some(true) => return no_plan("healthy at the exact re-read"),
            None => return no_plan("exact health overflow"),
        }
        let Some(size) = math::exact::size(
            collateral,
            U256::from(pos.borrowShares),
            s.total_borrow_assets_raw,
            s.total_borrow_shares_raw,
            s.price_raw,
            pm.lltv,
        ) else {
            return no_plan("exact size failed");
        };
        let borrowed_f = f64::from(borrowed);
        Ok(self.build_plan(
            h.account,
            &pm,
            s,
            f64::from(collateral),
            borrowed_f,
            Some(size),
        ))
    }

    fn predict(&self, feed: Address, answer: f64, _executor: Address) -> Vec<Plan> {
        let Some(links) = self.feed_signs.get(&feed) else {
            return vec![];
        };
        let mut guard = self.snapshot.lock().unwrap();
        let snap = &mut *guard;
        let Some(old) = snap.answers.insert(feed, answer).filter(|o| *o != 0.0) else {
            return vec![]; // first sighting: nothing to compare against
        };
        let ratio = answer / old;
        if !(ratio.is_finite() && ratio > 0.0) {
            return vec![];
        }
        let params = self.params.lock().unwrap();
        let mut plans = Vec::new();
        for (sid, sign) in links {
            let Some((id, s)) = snap.markets.get_mut(sid) else {
                continue;
            };
            let Some(pm) = params.get(id).copied() else {
                continue;
            };
            let lltv = f64::from(pm.lltv) / 1e18;
            let before = s.price;
            s.price *= ratio.powf(*sign);
            for sp in snap.positions.iter().filter(|p| p.account.sub_id == *sid) {
                let hf_before = math::health_factor(sp.collateral, sp.borrowed, before, lltv);
                let hf_after = math::health_factor(sp.collateral, sp.borrowed, s.price, lltv);
                if hf_before < PREDICT_HF_MAX || hf_after >= PREDICT_HF_MAX {
                    continue; // already the regular re-poll's job, or still healthy
                }
                plans.extend(self.build_plan(sp.account, &pm, s, sp.collateral, sp.borrowed, None));
            }
        }
        plans
    }

    /// Exact prediction: for each market priced by `feed` through an `ExactOracle`, the new
    /// oracle price from the exact answers, then each at-risk position's exact health before and
    /// after and, where the update makes it liquidatable, the exact size. Markets whose oracle
    /// is not exactly computable get no plan (the post-update polls cover them).
    fn predict_exact(&self, feed: Address, answer: U256, _executor: Address) -> Option<Vec<Plan>> {
        let links = self.feed_signs.get(&feed)?;
        let mut guard = self.snapshot.lock().unwrap();
        let snap = &mut *guard;
        snap.answers_exact.insert(feed, answer);
        let params = self.params.lock().unwrap();
        let mut plans = Vec::new();
        for (sid, _) in links {
            let Some(pm) = snap
                .markets
                .get(sid)
                .and_then(|(id, _)| params.get(id).copied())
            else {
                continue;
            };
            let Some(price) = self
                .exact_oracles
                .get(&pm.oracle)
                .and_then(|o| o.price(&snap.answers_exact))
            else {
                continue;
            };
            let Some((_, s)) = snap.markets.get_mut(sid) else {
                continue;
            };
            let before = s.price_raw;
            s.price_raw = price;
            s.price = f64::from(price);
            let s = *s;
            for sp in snap.positions.iter().filter(|p| p.account.sub_id == *sid) {
                let Some(borrowed) = math::exact::borrow_assets_up(
                    sp.borrow_shares,
                    s.total_borrow_assets_raw,
                    s.total_borrow_shares_raw,
                ) else {
                    continue;
                };
                let healthy =
                    |px| math::exact::is_healthy(sp.collateral_raw, borrowed, px, pm.lltv);
                if healthy(before) != Some(true) || healthy(price) != Some(false) {
                    continue; // already the regular re-poll's job, or still healthy
                }
                let Some(size) = math::exact::size(
                    sp.collateral_raw,
                    sp.borrow_shares,
                    s.total_borrow_assets_raw,
                    s.total_borrow_shares_raw,
                    price,
                    pm.lltv,
                ) else {
                    continue;
                };
                plans.extend(self.build_plan(
                    sp.account,
                    &pm,
                    &s,
                    sp.collateral,
                    sp.borrowed,
                    Some(size),
                ));
            }
        }
        Some(plans)
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.keys().copied().collect()
    }

    fn affected(&self, feed: Address, account: &AccountRef) -> bool {
        self.feeds
            .get(&feed)
            .is_none_or(|markets| markets.contains(&account.sub_id))
    }

    /// One collateral and one loan token per market: the position becomes liquidatable when the
    /// collateral price falls by its health factor (loan price unchanged), and Morpho lets a
    /// liquidator repay all of the debt at the market's incentive, up to what the collateral covers.
    fn watch_detail(&self, h: &Health) -> WatchDetail {
        let snap = self.snapshot.lock().unwrap();
        let Some((id, st)) = snap.markets.get(&h.account.sub_id) else {
            return WatchDetail::default();
        };
        let Some(pm) = self.params.lock().unwrap().get(id).copied() else {
            return WatchDetail::default();
        };
        let coll_dec = self.decimals.lock().unwrap().get(&pm.collateral).copied();
        // Oracle price: loan base units per collateral base unit, scaled by 1e36.
        let price_usd = coll_dec.map(|d| {
            st.price / math::ORACLE_PRICE_SCALE * 10f64.powi(d as i32)
                / 10f64.powi(st.loan_decimals as i32)
                * st.loan_usd
        });
        let hf = h.health_factor;
        let incentive = math::incentive_factor(f64::from(pm.lltv) / 1e18);
        WatchDetail {
            collateral: Some(pm.collateral),
            debt: Some(pm.loan),
            collateral_price_usd: price_usd,
            liquidation_price_usd: price_usd
                .filter(|_| hf.is_finite() && hf > 0.0)
                .map(|p| p / hf),
            max_repay_usd: Some(math::max_repay(h.debt_usd, h.collateral_usd, incentive)),
            bonus_rate: Some(incentive - 1.0),
        }
    }

    fn targets(&self) -> Vec<Address> {
        vec![self.cfg.morpho]
    }

    fn assets(&self) -> Vec<Address> {
        self.params
            .lock()
            .unwrap()
            .values()
            .flat_map(|p| [p.loan, p.collateral])
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
}

impl MorphoAdapter {
    /// The aggregator answer the snapshot last saw (sweep baseline or a predicted update).
    pub fn last_answer(&self, feed: Address) -> Option<f64> {
        self.snapshot.lock().unwrap().answers.get(&feed).copied()
    }

    /// At-risk snapshot positions in markets `feed` moves (diagnostics).
    pub fn exposed_positions(&self, feed: Address) -> usize {
        let snap = self.snapshot.lock().unwrap();
        self.feed_signs.get(&feed).map_or(0, |links| {
            links
                .iter()
                .map(|(sid, _)| {
                    snap.positions
                        .iter()
                        .filter(|p| p.account.sub_id == *sid)
                        .count()
                })
                .sum()
        })
    }

    /// Aggregators whose updates can be predicted (MorphoChainlinkOracleV2 feeds).
    pub fn predictable_feeds(&self) -> Vec<Address> {
        self.feed_signs.keys().copied().collect()
    }

    /// Prediction state: (predictable aggregators, snapshot markets, at-risk positions, answers).
    pub fn snapshot_summary(&self) -> (usize, usize, usize, usize) {
        let s = self.snapshot.lock().unwrap();
        (
            self.feed_signs.len(),
            s.markets.len(),
            s.positions.len(),
            s.answers.len(),
        )
    }

    /// The executor plan liquidating `collateral` / `borrowed` (raw) of `account` at state `s`.
    fn build_plan(
        &self,
        account: AccountRef,
        pm: &Params,
        s: &MarketState,
        collateral: f64,
        borrowed: f64,
        exact: Option<math::exact::Size>,
    ) -> Option<Plan> {
        let lltv = f64::from(pm.lltv) / 1e18;
        let size = math::size(collateral, borrowed, s.price, lltv)?;
        // Amounts that reach calldata come from the integer size when the caller has exact reads
        // (the `plan` path); the speculative `predict` path only has estimates and keeps its haircut.
        let (seized, flash) = match exact {
            Some(x) => (x.seized, x.flash),
            // Speculative (predicted price, no re-read): estimates, flashing 1% over the estimated
            // repayment. Never simulated; the executor's on-chain profit check guards it.
            None => (
                U256::from(size.seized as u128),
                U256::from(size.repaid as u128).checked_mul(U256::from(101u8))? / U256::from(100u8)
                    + U256::from(1u8),
            ),
        };
        let data = IMorpho::liquidateCall {
            marketParams: IMorpho::MarketParams {
                loanToken: pm.loan,
                collateralToken: pm.collateral,
                oracle: pm.oracle,
                irm: pm.irm,
                lltv: pm.lltv,
            },
            borrower: account.account,
            seizedAssets: seized,
            repaidShares: U256::ZERO,
            data: Default::default(),
        }
        .abi_encode();
        Some(Plan {
            protocol: ID,
            account,
            debt_asset: pm.loan,
            collateral_asset: pm.collateral,
            flash_amount: flash,
            dependencies: vec![],
            expected_collateral: f64::from(seized),
            bonus_usd: loan_usd(size.bonus, s),
            calls: vec![ProtocolCall {
                target: self.cfg.morpho,
                data: data.into(),
                approve_spender: self.cfg.morpho,
                approve_amount: flash,
            }],
        })
    }
}

/// The oracles among `oracles` whose price is exactly computable from Chainlink answers (see
/// `ExactOracle`). Anything unreadable or of another shape is left out (never estimated).
async fn exact_oracles(p: &DynProvider, oracles: &[Address]) -> HashMap<Address, ExactOracle> {
    let mut calls = Vec::with_capacity(oracles.len() * 7);
    for o in oracles {
        calls.push(call(*o, IMorphoOracle::SCALE_FACTORCall {}));
        calls.push(call(*o, IMorphoOracle::BASE_VAULTCall {}));
        calls.push(call(*o, IMorphoOracle::QUOTE_VAULTCall {}));
        calls.push(call(*o, IPriceSource::BASE_FEED_1Call {}));
        calls.push(call(*o, IPriceSource::BASE_FEED_2Call {}));
        calls.push(call(*o, IPriceSource::QUOTE_FEED_1Call {}));
        calls.push(call(*o, IPriceSource::QUOTE_FEED_2Call {}));
    }
    let Ok(res) = multicall(p, &calls, 200).await else {
        return HashMap::new();
    };
    // (oracle, scale, base proxies, quote proxies) for vault-free oracles.
    let mut shapes: Vec<(Address, U256, Vec<Address>, Vec<Address>)> = Vec::new();
    for (i, o) in oracles.iter().enumerate() {
        let r = &res[7 * i..7 * i + 7];
        let (Some(scale), Some(bv), Some(qv), Some(b1), Some(b2), Some(q1), Some(q2)) = (
            decode::<IMorphoOracle::SCALE_FACTORCall>(&r[0]),
            decode::<IMorphoOracle::BASE_VAULTCall>(&r[1]),
            decode::<IMorphoOracle::QUOTE_VAULTCall>(&r[2]),
            decode::<IPriceSource::BASE_FEED_1Call>(&r[3]),
            decode::<IPriceSource::BASE_FEED_2Call>(&r[4]),
            decode::<IPriceSource::QUOTE_FEED_1Call>(&r[5]),
            decode::<IPriceSource::QUOTE_FEED_2Call>(&r[6]),
        ) else {
            continue;
        };
        if !bv.is_zero() || !qv.is_zero() || scale.is_zero() {
            continue;
        }
        let nz = |v: [Address; 2]| v.into_iter().filter(|a| !a.is_zero()).collect::<Vec<_>>();
        shapes.push((*o, scale, nz([b1, b2]), nz([q1, q2])));
    }
    let proxies: Vec<Address> = shapes
        .iter()
        .flat_map(|(_, _, b, q)| b.iter().chain(q).copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let calls: Vec<_> = proxies
        .iter()
        .map(|a| call(*a, IPriceSource::aggregatorCall {}))
        .collect();
    let Ok(res) = multicall(p, &calls, 200).await else {
        return HashMap::new();
    };
    let agg: HashMap<Address, Address> = proxies
        .iter()
        .zip(&res)
        .filter_map(|(p, r)| {
            let a = decode::<IPriceSource::aggregatorCall>(r)?;
            (!a.is_zero()).then_some((*p, a))
        })
        .collect();
    shapes
        .into_iter()
        .filter_map(|(o, scale, b, q)| {
            let base = b
                .iter()
                .map(|p| agg.get(p).copied())
                .collect::<Option<Vec<_>>>()?;
            let quote = q
                .iter()
                .map(|p| agg.get(p).copied())
                .collect::<Option<Vec<_>>>()?;
            Some((o, ExactOracle { scale, base, quote }))
        })
        .collect()
}

/// For each aggregator, the MorphoChainlinkOracleV2 markets it prices and the exponent of its
/// effect (+1 via BASE_FEED_*, −1 via QUOTE_FEED_*). Oracles of other types are left out.
async fn feed_signs(
    p: &DynProvider,
    market_oracles: &[(u64, Address)],
) -> HashMap<Address, Vec<(u64, f64)>> {
    let oracles: Vec<Address> = market_oracles
        .iter()
        .map(|(_, o)| *o)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut calls = Vec::with_capacity(oracles.len() * 4);
    for o in &oracles {
        calls.push(call(*o, IPriceSource::BASE_FEED_1Call {}));
        calls.push(call(*o, IPriceSource::BASE_FEED_2Call {}));
        calls.push(call(*o, IPriceSource::QUOTE_FEED_1Call {}));
        calls.push(call(*o, IPriceSource::QUOTE_FEED_2Call {}));
    }
    let res = match multicall(p, &calls, 200).await {
        Ok(r) => r,
        Err(e) => {
            warn!(protocol = ID, error = %format!("{e:#}"), "oracle feeds unreadable: no prediction");
            return HashMap::new();
        }
    };
    // (feed proxy, oracle, sign)
    let mut proxies: Vec<(Address, Address, f64)> = Vec::new();
    for (i, o) in oracles.iter().enumerate() {
        let r = &res[4 * i..4 * i + 4];
        let base = [
            decode::<IPriceSource::BASE_FEED_1Call>(&r[0]),
            decode::<IPriceSource::BASE_FEED_2Call>(&r[1]),
        ];
        let quote = [
            decode::<IPriceSource::QUOTE_FEED_1Call>(&r[2]),
            decode::<IPriceSource::QUOTE_FEED_2Call>(&r[3]),
        ];
        proxies.extend(
            base.into_iter()
                .flatten()
                .filter(|a| !a.is_zero())
                .map(|a| (a, *o, 1.0)),
        );
        proxies.extend(
            quote
                .into_iter()
                .flatten()
                .filter(|a| !a.is_zero())
                .map(|a| (a, *o, -1.0)),
        );
    }
    let sources: Vec<Address> = proxies
        .iter()
        .map(|(a, _, _)| *a)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let aggs = match mev_oracle::resolve_aggregators(p, &sources).await {
        Ok(a) => a,
        Err(e) => {
            warn!(protocol = ID, error = %format!("{e:#}"), "oracle feeds unresolved: no prediction");
            return HashMap::new();
        }
    };
    // aggregator -> market -> summed exponent (a feed in both numerator and denominator cancels)
    let mut acc: HashMap<Address, HashMap<u64, f64>> = HashMap::new();
    for (agg, srcs) in &aggs {
        for src in srcs {
            for (proxy, oracle, sign) in proxies.iter().filter(|(p, _, _)| p == src) {
                let _ = proxy;
                for (sid, _) in market_oracles.iter().filter(|(_, o)| o == oracle) {
                    *acc.entry(*agg).or_default().entry(*sid).or_default() += sign;
                }
            }
        }
    }
    acc.into_iter()
        .map(|(agg, m)| {
            (
                agg,
                m.into_iter().filter(|(_, e)| *e != 0.0).collect::<Vec<_>>(),
            )
        })
        .filter(|(_, v): &(Address, Vec<(u64, f64)>)| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod decimals_tests {
    use super::*;

    fn market(loan_usd: f64, loan_decimals: u8) -> MarketState {
        MarketState {
            price: 1e36,
            total_borrow_assets: 0.0,
            total_borrow_shares: 0.0,
            loan_usd,
            loan_decimals,
            price_raw: U256::ZERO,
            total_borrow_assets_raw: U256::ZERO,
            total_borrow_shares_raw: U256::ZERO,
        }
    }

    #[test]
    fn loan_amounts_are_scaled_by_the_loan_tokens_own_decimals() {
        // 1.5 USDC (6 decimals), 0.5 WBTC (8), 2 WETH (18): same code, three scales.
        assert!((loan_usd(1_500_000.0, &market(1.0, 6)) - 1.5).abs() < 1e-9);
        assert!((loan_usd(50_000_000.0, &market(86_000.0, 8)) - 43_000.0).abs() < 1e-6);
        assert!((loan_usd(2e18, &market(2_700.0, 18)) - 5_400.0).abs() < 1e-6);
    }

    #[test]
    fn assuming_18_for_a_6_decimal_token_would_be_off_by_a_trillion() {
        // The reason unknown decimals must skip the market instead of defaulting to 18.
        let right = loan_usd(1_500_000.0, &market(1.0, 6));
        let wrong = loan_usd(1_500_000.0, &market(1.0, 18));
        assert!((right / wrong - 1e12).abs() / 1e12 < 1e-9);
    }
}

#[cfg(test)]
mod exact_oracle_tests {
    use super::*;
    use alloy::providers::{Provider, ProviderBuilder};

    #[test]
    fn exact_price_is_scale_mul_div_of_the_answers() {
        let (b, q) = (Address::with_last_byte(1), Address::with_last_byte(2));
        let o = ExactOracle {
            scale: U256::from(10u64).pow(U256::from(26u8)),
            base: vec![b],
            quote: vec![q],
        };
        let answers: HashMap<Address, U256> = [
            (b, U256::from(300_000_000_000u64)),
            (q, U256::from(100_000_000u64)),
        ]
        .into();
        // 1e26 x 3000e8 / 1e8 = 3e29.
        assert_eq!(
            o.price(&answers),
            Some(U256::from(3u64) * U256::from(10u64).pow(U256::from(29u8)))
        );
        assert_eq!(
            o.price(&HashMap::new()),
            None,
            "missing answer: no price, never a guess"
        );
    }

    /// For the 20 most-borrowed Base markets: every oracle `exact_oracles` accepts reproduces
    /// the on-chain `price()` exactly from its aggregators' answers, at one pinned block.
    #[tokio::test]
    #[ignore = "network"]
    async fn exact_oracles_reproduce_base_morpho_prices() {
        let p = ProviderBuilder::new()
            .connect_http("https://base.gateway.tenderly.co".parse().unwrap())
            .erased();
        let morpho = IMorpho::new(
            "0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb"
                .parse()
                .unwrap(),
            p.clone(),
        );
        let ids = [
            "0x9103c3b4e834476c9a62ea009ba2c884ee42e94e6e314a26f04d312434191836", // market id
            "0xd4a903dc6d949519060c7707f9604fdc9772c046e05c2e3a8fce0bd7196e4109", // market id
            "0x8793cf302b8ffd655ab97bd1c695dbd967807e8367a65cb2f4edaf1380ba1bda", // market id
            "0x0ca10126f6c94cbd9cf0a48cc9516ae5e3dec5aa68303e6d988ee37c5149bf0d", // market id
            "0x7dc02ff6c536b1d49d7fba770438d79f5bd1f1c78884629b7d1aaee19675782b", // market id
            "0x09276541cfecb6920a80679a1deced4dde3ae64bf5fc2c9c1f9c21e0c152e1a5", // market id
            "0x73527ddd796e6d4f48387adaae36f6f3d49d606d7f2a15eb0c931416a58875d8", // market id
            "0xd7520ad198b497b6eb75bc690268f4597630dbc12e305e9d4105843bab36e41d", // market id
            "0x9125d0fa03c3137166df68bcc72283477830de2a4a5536512374c573ad4583c3", // market id
            "0xe458a501c1103ca3b256f2bcb45d223263fb34b6349d82675494ce3a94ce1327", // market id
            "0x76629002c9fdb20fb2092053a4f22c5c65e04a81887218b9546f2a2d82b4c0a8", // market id
            "0x78d11c03944e0dc298398f0545dc8195ad201a18b0388cb8058b1bcb89440971", // market id
            "0x84662b4f95b85d6b082b68d32cf71bb565b3f22f216a65509cc2ede7dccdfe8c", // market id
            "0x3b3769cfca57be2eaed03fcc5299c25691b77781a1e124e7a8d520eb9a7eabb5", // market id
            "0x13c42741a359ac4a8aa8287d2be109dcf28344484f91185f9a79bd5a805a55ae", // market id
            "0xdaa04f6819210b11fe4e3b65300c725c32e55755e3598671559b9ae3bac453d7", // market id
            "0x67ebd84b2fb39e3bc5a13d97e4c07abe1ea617e40654826e9abce252e95f049e", // market id
            "0x3a4048c64ba1b375330d376b1ce40e4047d03b47ab4d48af484edec9fec801ba", // market id
            "0x54cf9be57fdfa6457a660991907434ff9d295c465a603a50126ff647d50b7354", // market id
            "0x1c21c59df9db44bf6f645d854ee710a8ca17b479451447e9f56758aee10a2fad", // market id
        ];
        let mut oracles = Vec::new();
        for id in ids {
            let m = morpho
                .idToMarketParams(id.parse().unwrap())
                .call()
                .await
                .unwrap();
            oracles.push(m.oracle);
        }
        oracles.sort_unstable();
        oracles.dedup();
        let exact = exact_oracles(&p, &oracles).await;
        let mut checked = 0;
        for (addr, o) in &exact {
            // One Multicall3 request, so the answers and price() come from the same node and block.
            // Aggregators refuse contract callers; their (public) proxies serve the same answer.
            let feeds = multicall(
                &p,
                &[
                    call(*addr, IPriceSource::BASE_FEED_1Call {}),
                    call(*addr, IPriceSource::BASE_FEED_2Call {}),
                    call(*addr, IPriceSource::QUOTE_FEED_1Call {}),
                    call(*addr, IPriceSource::QUOTE_FEED_2Call {}),
                ],
                200,
            )
            .await
            .unwrap();
            let proxies: Vec<Address> = [
                decode::<IPriceSource::BASE_FEED_1Call>(&feeds[0]),
                decode::<IPriceSource::BASE_FEED_2Call>(&feeds[1]),
                decode::<IPriceSource::QUOTE_FEED_1Call>(&feeds[2]),
                decode::<IPriceSource::QUOTE_FEED_2Call>(&feeds[3]),
            ]
            .into_iter()
            .flatten()
            .filter(|a| !a.is_zero())
            .collect();
            let mut calls = Vec::new();
            for px in &proxies {
                calls.push(call(*px, IPriceSource::aggregatorCall {}));
                calls.push(call(*px, IPriceSource::latestAnswerCall {}));
            }
            calls.push(call(*addr, IMorphoOracle::priceCall {}));
            let res = multicall(&p, &calls, 200).await.unwrap();
            let answers: HashMap<Address, U256> = res
                .chunks(2)
                .take(proxies.len())
                .map(|r| {
                    let a = decode::<IPriceSource::aggregatorCall>(&r[0]).unwrap();
                    let v = decode::<IPriceSource::latestAnswerCall>(&r[1]).unwrap();
                    (a, U256::from(v.unsigned_abs()))
                })
                .collect();
            let onchain = decode::<IMorphoOracle::priceCall>(res.last().unwrap()).unwrap();
            assert_eq!(o.price(&answers), Some(onchain), "oracle {addr}");
            checked += 1;
        }
        println!("exact oracles: {checked} of {}", oracles.len());
        assert!(checked > 0);
    }
}
