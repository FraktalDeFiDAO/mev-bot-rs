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
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, topic_address};
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
    }
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
    index: EventIndex,
    params: Mutex<HashMap<B256, Params>>,
    decimals: Mutex<HashMap<Address, u8>>,
    /// Aggregator -> short ids of markets whose oracle reads it.
    feeds: HashMap<Address, HashSet<u64>>,
    /// Aggregator -> (market short id, exponent) for MorphoChainlinkOracleV2 markets: the oracle
    /// price is base1·base2 / (quote1·quote2), so a base feed moves it by ratio^+1 and a quote feed
    /// by ratio^-1. Markets with other oracle types are not predicted.
    feed_signs: HashMap<Address, Vec<(u64, f64)>>,
    /// Backrun prediction state, refreshed by sweeps and moved by each predicted update.
    snapshot: Mutex<Snapshot>,
}

#[derive(Default)]
struct Snapshot {
    /// Market short id -> (id, state).
    markets: HashMap<u64, (B256, MarketState)>,
    /// At-risk positions: (account, collateral raw, borrowed raw).
    positions: Vec<(AccountRef, f64, f64)>,
    /// Aggregator -> last known answer.
    answers: HashMap<Address, f64>,
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
        let index = EventIndex::open(IndexConfig {
            addresses: vec![cfg.morpho],
            topics: vec![IMorpho::Borrow::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        });
        let me = Self {
            cfg,
            rpc,
            index,
            params: Mutex::new(HashMap::new()),
            decimals: Mutex::new(HashMap::new()),
            feeds: HashMap::new(),
            feed_signs: HashMap::new(),
            snapshot: Mutex::new(Snapshot::default()),
        };
        let ids: Vec<B256> = me.market_ids();
        me.load_params(&ids).await?;
        let market_oracles: Vec<(u64, Address)> = me
            .params
            .lock()
            .unwrap()
            .iter()
            .map(|(id, p)| (short_id(id), p.oracle))
            .collect();
        let oracles: Vec<Address> = market_oracles.iter().map(|(_, o)| *o).collect();
        let aggregators = mev_oracle::resolve_aggregators(&me.rpc.fast, &oracles).await?;
        let feeds = mev_lending::feed_index(&aggregators, |oracle| {
            market_oracles
                .iter()
                .filter(|(_, o)| *o == oracle)
                .map(|(id, _)| *id)
                .collect()
        });
        let feed_signs = feed_signs(&me.rpc.fast, &market_oracles).await;
        info!(
            protocol = ID,
            markets = ids.len(),
            borrowers_cached = me.index.len(),
            predictable_feeds = feed_signs.len(),
            "adapter ready"
        );
        Ok(Self {
            feeds,
            feed_signs,
            ..me
        })
    }

    /// Track a (market, borrower) learned elsewhere (another indexer, a liquidation feed, tests).
    pub fn track(&mut self, id: B256, borrower: Address) {
        self.index.insert((id, borrower));
    }

    fn market_ids(&self) -> Vec<B256> {
        self.index
            .accounts()
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
        self.index
            .step(&self.rpc.scan, |l| {
                match (l.topics().get(1), topic_address(l, 2)) {
                    (Some(id), Some(user)) => vec![(*id, user)],
                    _ => vec![],
                }
            })
            .await
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let morpho = self.cfg.morpho;
        self.index
            .accounts()
            .map(|(id, u)| AccountRef {
                market: morpho,
                account: *u,
                sub_id: short_id(id),
            })
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
        let ids: Vec<B256> = pairs
            .iter()
            .map(|(_, id)| *id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let states = self.market_states(p, &ids).await?;
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
        let mut at_risk: Vec<(AccountRef, f64, f64)> = Vec::new();
        let out: Vec<Health> = pairs
            .iter()
            .zip(res.iter())
            .filter_map(|((a, id), r)| {
                let pos = decode::<IMorpho::positionCall>(r)?;
                let s = states.get(id)?;
                let borrowed = math::borrow_assets(
                    pos.borrowShares as f64,
                    s.total_borrow_assets,
                    s.total_borrow_shares,
                );
                if pos.borrowShares == 0 {
                    return None;
                }
                let collateral = pos.collateral as f64;
                let coll_in_loan = collateral * s.price / math::ORACLE_PRICE_SCALE;
                let health_factor =
                    math::health_factor(collateral, borrowed, s.price, self.lltv(id));
                if !fast && health_factor < SNAPSHOT_HF {
                    at_risk.push((*a, collateral, borrowed));
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
            let markets = states
                .into_iter()
                .map(|(id, st)| (short_id(&id), (id, st)))
                .collect();
            *self.snapshot.lock().unwrap() = Snapshot {
                markets,
                positions: at_risk,
                answers,
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
        let states = self.market_states(&self.rpc.fast, &[id]).await?;
        let Some(s) = states.get(&id) else {
            return Ok(None);
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
            return Ok(None);
        };
        // Health and sizing from the exact integer reads (the floats above only nominated this account).
        let collateral = U256::from(pos.collateral);
        let Some(borrowed) = math::exact::borrow_assets_up(
            U256::from(pos.borrowShares),
            s.total_borrow_assets_raw,
            s.total_borrow_shares_raw,
        ) else {
            return Ok(None);
        };
        if math::exact::is_healthy(collateral, borrowed, s.price_raw, pm.lltv) != Some(false) {
            return Ok(None);
        }
        let Some(size) = math::exact::size(collateral, borrowed, s.price_raw, pm.lltv) else {
            return Ok(None);
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
            for (acct, collateral, borrowed) in snap.positions.iter().filter(|p| p.0.sub_id == *sid)
            {
                let hf_before = math::health_factor(*collateral, *borrowed, before, lltv);
                let hf_after = math::health_factor(*collateral, *borrowed, s.price, lltv);
                if hf_before < PREDICT_HF_MAX || hf_after >= PREDICT_HF_MAX {
                    continue; // already the regular re-poll's job, or still healthy
                }
                plans.extend(self.build_plan(*acct, &pm, s, *collateral, *borrowed, None));
            }
        }
        plans
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.keys().copied().collect()
    }

    fn affected(&self, feed: Address, account: &AccountRef) -> bool {
        self.feeds
            .get(&feed)
            .is_none_or(|markets| markets.contains(&account.sub_id))
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
                .map(|(sid, _)| snap.positions.iter().filter(|p| p.0.sub_id == *sid).count())
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
        let (seized, repaid) = match exact {
            Some(x) => (x.seized, x.repaid),
            None => (
                U256::from(size.seized as u128),
                U256::from(size.repaid as u128),
            ),
        };
        // Flash 1% over the computed repayment (rounding / interest accrual); excess is returned.
        let flash = repaid.checked_mul(U256::from(101u8))? / U256::from(100u8) + U256::from(1u8);
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
