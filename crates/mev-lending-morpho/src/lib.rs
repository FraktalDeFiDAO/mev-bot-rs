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
use mev_bindings::{IAaveOracle, IERC20};
use mev_config::MorphoConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::info;

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
}

pub struct MorphoAdapter {
    cfg: MorphoConfig,
    rpc: AdapterRpc,
    index: EventIndex,
    params: Mutex<HashMap<B256, Params>>,
    decimals: Mutex<HashMap<Address, u8>>,
    /// Aggregator -> short ids of markets whose oracle reads it.
    feeds: HashMap<Address, HashSet<u64>>,
}

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
        info!(
            protocol = ID,
            markets = ids.len(),
            borrowers_cached = me.index.len(),
            "adapter ready"
        );
        Ok(Self { feeds, ..me })
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

    /// Fetch and cache market params + token decimals for ids not seen yet.
    async fn load_params(&self, ids: &[B256]) -> Result<()> {
        let missing: Vec<B256> = {
            let known = self.params.lock().unwrap();
            ids.iter()
                .filter(|i| !known.contains_key(*i))
                .copied()
                .collect()
        };
        if missing.is_empty() {
            return Ok(());
        }
        let calls: Vec<_> = missing
            .iter()
            .map(|id| call(self.cfg.morpho, IMorpho::idToMarketParamsCall { id: *id }))
            .collect();
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        let mut new_tokens = Vec::new();
        {
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
                    new_tokens.extend([p.loanToken, p.collateralToken]);
                }
            }
        }
        let calls: Vec<_> = new_tokens
            .iter()
            .map(|t| call(*t, IERC20::decimalsCall {}))
            .collect();
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        let mut dec = self.decimals.lock().unwrap();
        for (t, r) in new_tokens.iter().zip(res.iter()) {
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
                        loan_decimals: dec.get(&pm.loan).copied().unwrap_or(18),
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
        Ok(pairs
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
                Some(Health {
                    account: *a,
                    health_factor: math::health_factor(
                        collateral,
                        borrowed,
                        s.price,
                        self.lltv(id),
                    ),
                    debt_usd: loan_usd(borrowed, s),
                    collateral_usd: loan_usd(coll_in_loan, s),
                })
            })
            .collect())
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
        let borrowed = math::borrow_assets(
            pos.borrowShares as f64,
            s.total_borrow_assets,
            s.total_borrow_shares,
        );
        let lltv = f64::from(pm.lltv) / 1e18;
        if math::health_factor(pos.collateral as f64, borrowed, s.price, lltv) >= 1.0 {
            return Ok(None);
        }
        let Some(size) = math::size(pos.collateral as f64, borrowed, s.price, lltv) else {
            return Ok(None);
        };
        // Flash 1% over the computed repayment (rounding / interest accrual); excess is returned.
        let flash = U256::from((size.repaid * 1.01) as u128 + 1);
        let data = IMorpho::liquidateCall {
            marketParams: IMorpho::MarketParams {
                loanToken: pm.loan,
                collateralToken: pm.collateral,
                oracle: pm.oracle,
                irm: pm.irm,
                lltv: pm.lltv,
            },
            borrower: h.account.account,
            seizedAssets: U256::from(size.seized as u128),
            repaidShares: U256::ZERO,
            data: Default::default(),
        }
        .abi_encode();
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: pm.loan,
            collateral_asset: pm.collateral,
            flash_amount: flash,
            expected_collateral: size.seized,
            bonus_usd: loan_usd(size.bonus, s),
            calls: vec![ProtocolCall {
                target: self.cfg.morpho,
                data: data.into(),
                approve_spender: self.cfg.morpho,
                approve_amount: flash,
            }],
        }))
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
