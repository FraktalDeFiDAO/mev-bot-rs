//! Compound III (Comet) liquidation adapter.
//!
//! Two opportunity shapes, both settled by `buyCollateral` at the storefront discount
//! (`storeFrontPriceFactor × (1 − liquidationFactor)`, e.g. 6% for ARB on the USDC Comet):
//! 1. **Absorb**: an undercollateralized borrower is `absorb`ed (anyone may call; the protocol takes
//!    the collateral), then that collateral is bought back at the discount — one atomic plan.
//! 2. **Storefront**: while reserves are below target, the protocol sells existing collateral
//!    reserves at the same discount to anyone. Modeled as pseudo-accounts
//!    (`AccountRef { market: comet, account: asset, sub_id: STOREFRONT }`) so the same runner
//!    sweeps and polls them.
//!
//! Health = Σ(collateral × price × liquidateCollateralFactor) / (borrow × basePrice), which is
//! exactly Comet's `isLiquidatable` condition. Accounts come from `Withdraw` events (base
//! withdrawals create borrows; pure lenders are filtered out by zero borrow balance).

pub mod math;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};

use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use mev_config::CompoundV3Config;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, market_key, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::{info, warn};

pub const ID: ProtocolId = "compound-v3";
/// `AccountRef::sub_id` values.
pub const BORROWER: u64 = 0;
pub const STOREFRONT: u64 = 1;

sol! {
    #[sol(rpc)]
    interface IAggregator {
        function latestAnswer() external view returns (int256);
    }

    #[sol(rpc)]
    interface IComet {
        struct AssetInfo { uint8 offset; address asset; address priceFeed; uint64 scale; uint64 borrowCollateralFactor; uint64 liquidateCollateralFactor; uint64 liquidationFactor; uint128 supplyCap; }
        function baseToken() external view returns (address);
        function baseTokenPriceFeed() external view returns (address);
        function baseScale() external view returns (uint256);
        function numAssets() external view returns (uint8);
        function getAssetInfo(uint8 i) external view returns (AssetInfo memory);
        function storeFrontPriceFactor() external view returns (uint256);
        function getPrice(address priceFeed) external view returns (uint256);
        function borrowBalanceOf(address account) external view returns (uint256);
        function userCollateral(address account, address asset) external view returns (uint128 balance, uint128 reserved);
        function getReserves() external view returns (int256);
        function targetReserves() external view returns (uint256);
        function getCollateralReserves(address asset) external view returns (uint256);
        function absorb(address absorber, address[] calldata accounts) external;
        function buyCollateral(address asset, uint256 minAmount, uint256 baseAmount, address recipient) external;
        event Withdraw(address indexed src, address indexed to, uint256 amount);
    }

    #[sol(rpc)]
    interface IChainlink {
        function latestAnswer() external view returns (int256);
    }
}

#[derive(Debug, Clone)]
struct Asset {
    asset: Address,
    feed: Address,
    scale: f64,
    liquidate_cf: f64,
    liquidation_factor: f64,
}

#[derive(Debug, Clone)]
struct Comet {
    address: Address,
    base: Address,
    base_feed: Address,
    base_scale: f64,
    storefront_factor: f64,
    usd_feed: Option<Address>,
    assets: Vec<Asset>,
}

/// Prices for one Comet, in its own unit (USD or ETH, 8 decimals as f64) plus unit→USD.
#[derive(Debug, Clone)]
struct Prices {
    base: f64,
    assets: HashMap<Address, f64>,
    usd_per_unit: f64,
}

pub struct CompoundV3Adapter {
    cfg: CompoundV3Config,
    rpc: AdapterRpc,
    comets: Vec<Comet>,
    index: EventIndex,
    /// Aggregator -> Comets whose collateral it prices.
    feeds: HashMap<Address, HashSet<Address>>,
    /// Aggregator -> (Comet, collateral asset) it prices (for repricing on a predicted update).
    feed_assets: HashMap<Address, HashSet<(Address, Address)>>,
    /// Backrun prediction state, refreshed by sweeps and moved by each predicted update.
    snapshot: Mutex<Snapshot>,
}

/// Per Comet: market (prices, factors) and at-risk borrowers' positions, as of the last sweep.
#[derive(Default)]
struct Snapshot {
    comets: HashMap<Address, CometSnap>,
    /// Aggregator -> last known answer.
    answers: HashMap<Address, f64>,
    /// How each (aggregator, Comet, asset) price follows its aggregator, learned across sweeps.
    elasticity: HashMap<(Address, Address, Address), Elasticity>,
}

/// An asset price's response to its aggregator's answer: `price ∝ answer^e`. Wrappers can rescale
/// (SVR), compose (CAPO: rate × ETH/USD) or invert (USDC priced in ETH) a feed, so `e` is measured,
/// never assumed: +1 when price and answer differ by a power of ten, otherwise classified from
/// two sweeps between which the answer moved. Unknown ⇒ the asset is not repriced.
#[derive(Debug, Clone, Copy)]
struct Elasticity {
    price: f64,
    answer: f64,
    e: Option<f64>,
}

impl Elasticity {
    fn first(price: f64, answer: f64) -> Self {
        // A plain or rescaled proxy (e.g. SVR's 18-decimal aggregator behind an 8-decimal feed):
        // price and answer differ by an exact power of ten.
        let k = (answer / price).log10();
        let plain = k.is_finite() && (k - k.round()).abs() < 1e-9;
        Self {
            price,
            answer,
            e: plain.then_some(1.0),
        }
    }

    /// Re-measure against a new sample. The old sample is kept until the answer has moved enough
    /// to measure (tiny moves would make the estimate noise).
    fn update(self, price: f64, answer: f64) -> Self {
        let now = Self::first(price, answer);
        if now.e.is_some() {
            return now;
        }
        let da = (answer / self.answer).ln();
        if !(da.is_finite() && da.abs() > ELASTICITY_MIN_MOVE) {
            return self;
        }
        let est = (price / self.price).ln() / da;
        Self {
            e: [1.0, -1.0].into_iter().find(|c| (est - c).abs() < 0.15),
            ..now
        }
    }
}

struct CometSnap {
    market: math::Market,
    for_sale: bool,
    positions: Vec<(Address, math::Position)>,
}

/// What a plan buys: `amount` of collateral `asset`, after `absorb`ing the borrower if set.
/// On-chain slippage floor as a fraction of the quoted collateral. The base we pay is already priced at
/// 0.999 of the quote (`QUOTE_HAIRCUT`), so the contract's collateral lands near 0.999 x amount; a floor of
/// 0.999 would sit exactly on that edge and revert on float noise. Keep the floor a full 0.1% below it.
const MIN_AMOUNT_FRACTION: f64 = 0.998;

struct Buy {
    asset: Address,
    amount: f64,
    absorb: bool,
}

/// Per Comet from a sweep: market and at-risk borrowers' positions.
type SweepSnap = Vec<(Address, math::Market, Vec<(Address, math::Position)>)>;

/// Smallest aggregator move (log) that classifies an elasticity (0.05 %).
const ELASTICITY_MIN_MOVE: f64 = 5e-4;
/// Predicted health below this triggers a backrun (margin against rounding).
const PREDICT_HF_MAX: f64 = 0.9995;
/// Sweeps keep positions of borrowers below this health factor for prediction.
const SNAPSHOT_HF: f64 = 1.10;

impl CompoundV3Adapter {
    pub async fn new(cfg: CompoundV3Config, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let mut comets = Vec::new();
        for c in &cfg.comets {
            let comet = IComet::new(c.address, &rpc.fast);
            let n = comet.numAssets().call().await.context("Comet numAssets")?;
            let mut assets = Vec::new();
            for i in 0..n {
                let a = comet.getAssetInfo(i).call().await?;
                assets.push(Asset {
                    asset: a.asset,
                    feed: a.priceFeed,
                    scale: a.scale as f64,
                    liquidate_cf: a.liquidateCollateralFactor as f64 / 1e18,
                    liquidation_factor: a.liquidationFactor as f64 / 1e18,
                });
            }
            comets.push(Comet {
                address: c.address,
                base: comet.baseToken().call().await?,
                base_feed: comet.baseTokenPriceFeed().call().await?,
                base_scale: f64::from(comet.baseScale().call().await?),
                storefront_factor: f64::from(comet.storeFrontPriceFactor().call().await?) / 1e18,
                usd_feed: c.usd_feed,
                assets,
            });
        }
        let sources: Vec<Address> = comets
            .iter()
            .flat_map(|c| c.assets.iter().map(|a| a.feed))
            .collect();
        let aggregators = mev_oracle::resolve_aggregators(&rpc.fast, &sources).await?;
        let feeds = mev_lending::feed_index(&aggregators, |src| {
            comets
                .iter()
                .filter(|c| c.assets.iter().any(|a| a.feed == src))
                .map(|c| c.address)
                .collect()
        });
        let feed_assets = mev_lending::feed_index(&aggregators, |src| {
            comets
                .iter()
                .flat_map(|c| {
                    c.assets
                        .iter()
                        .filter(move |a| a.feed == src)
                        .map(move |a| (c.address, a.asset))
                })
                .collect()
        });
        let index = EventIndex::open(IndexConfig {
            addresses: comets.iter().map(|c| c.address).collect(),
            topics: vec![IComet::Withdraw::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        });
        info!(
            protocol = ID,
            comets = comets.len(),
            collateral_assets = sources.len(),
            price_feeds = feeds.len(),
            "adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            comets,
            index,
            feeds,
            feed_assets,
            snapshot: Mutex::new(Snapshot::default()),
        })
    }

    /// Prediction snapshot overview: (Comet, at-risk positions, storefront open), aggregator
    /// answers known, and (aggregator, Comet, asset) links with a measured elasticity / all links.
    pub fn snapshot_summary(&self) -> (Vec<(Address, usize, bool)>, usize, usize, usize) {
        let s = self.snapshot.lock().unwrap();
        (
            s.comets
                .iter()
                .map(|(a, c)| (*a, c.positions.len(), c.for_sale))
                .collect(),
            s.answers.len(),
            s.elasticity.values().filter(|e| e.e.is_some()).count(),
            s.elasticity.len(),
        )
    }

    /// The executor plan for buying `amount` of `asset` (after `absorb` of `account` if set).
    fn build_plan(
        &self,
        c: &Comet,
        market: &math::Market,
        account: AccountRef,
        buy: Buy,
        executor: Address,
    ) -> Option<Plan> {
        let Buy {
            asset,
            amount,
            absorb,
        } = buy;
        let q = market.quote(asset, amount)?;
        if q.base_amount < 1.0 {
            return None;
        }
        let base_amount = U256::from(q.base_amount as u128);
        let mut calls = Vec::new();
        if absorb {
            calls.push(ProtocolCall {
                target: c.address,
                data: IComet::absorbCall {
                    absorber: executor,
                    accounts: vec![account.account],
                }
                .abi_encode()
                .into(),
                approve_spender: Address::ZERO,
                approve_amount: U256::ZERO,
            });
        }
        calls.push(ProtocolCall {
            target: c.address,
            data: IComet::buyCollateralCall {
                asset,
                minAmount: U256::from((amount * MIN_AMOUNT_FRACTION) as u128),
                baseAmount: base_amount,
                recipient: executor,
            }
            .abi_encode()
            .into(),
            approve_spender: c.address,
            approve_amount: base_amount,
        });
        Some(Plan {
            protocol: ID,
            account,
            debt_asset: c.base,
            collateral_asset: asset,
            flash_amount: base_amount,
            dependencies: vec![],
            expected_collateral: amount,
            bonus_usd: q.discount_usd,
            calls,
        })
    }

    /// Keep at-risk borrowers' positions, each Comet's market and storefront state, and the
    /// aggregators' latest answers for backrun prediction (sweep RPC; not latency-critical).
    async fn refresh_snapshot(&self, sweep: SweepSnap) {
        let p = &self.rpc.sweep;
        let mut comets = HashMap::new();
        for (addr, market, positions) in sweep {
            let Some(c) = self.comet(addr) else { continue };
            let for_sale = match self.storefront_state(p, c).await {
                Ok((f, _)) => f,
                Err(e) => {
                    warn!(protocol = ID, error = %format!("{e:#}"), "prediction snapshot: storefront");
                    false
                }
            };
            comets.insert(
                addr,
                CometSnap {
                    market,
                    for_sale,
                    positions,
                },
            );
        }
        // Direct eth_calls, not Multicall3: access-controlled aggregators only serve reads where
        // msg.sender == tx.origin.
        let aggs: Vec<Address> = self.feed_assets.keys().copied().collect();
        let answers: HashMap<Address, f64> = mev_oracle::latest_answers(p, &aggs).await;
        let mut snap = self.snapshot.lock().unwrap();
        for (agg, hits) in &self.feed_assets {
            let Some(&a) = answers.get(agg) else { continue };
            for (comet, asset) in hits {
                let Some(price) = comets.get(comet).and_then(|c: &CometSnap| {
                    c.market
                        .assets
                        .iter()
                        .find(|x| x.asset == *asset)
                        .map(|x| x.price)
                }) else {
                    continue;
                };
                let k = (*agg, *comet, *asset);
                let next = match snap.elasticity.get(&k) {
                    Some(e) => e.update(price, a),
                    None => Elasticity::first(price, a),
                };
                snap.elasticity.insert(k, next);
            }
        }
        snap.comets = comets;
        snap.answers = answers;
    }

    fn comet(&self, address: Address) -> Option<&Comet> {
        self.comets.iter().find(|c| c.address == address)
    }

    async fn prices(&self, p: &DynProvider, c: &Comet) -> Result<Prices> {
        let mut calls = vec![call(
            c.address,
            IComet::getPriceCall {
                priceFeed: c.base_feed,
            },
        )];
        calls.extend(
            c.assets
                .iter()
                .map(|a| call(c.address, IComet::getPriceCall { priceFeed: a.feed })),
        );
        if let Some(f) = c.usd_feed {
            calls.push(call(f, IChainlink::latestAnswerCall {}));
        }
        let res = multicall(p, &calls, 200).await?;
        let px = |i: usize| {
            decode::<IComet::getPriceCall>(&res[i])
                .map(f64::from)
                .unwrap_or(0.0)
                / 1e8
        };
        let assets = c
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| (a.asset, px(i + 1)))
            .collect();
        let usd_per_unit = match c.usd_feed {
            Some(_) => decode::<IChainlink::latestAnswerCall>(&res[c.assets.len() + 1])
                .map(|v| f64::from(v.max(alloy::primitives::I256::ZERO).into_raw()) / 1e8)
                .unwrap_or(0.0),
            None => 1.0,
        };
        Ok(Prices {
            base: px(0),
            assets,
            usd_per_unit,
        })
    }

    /// Market state relevant to storefront sales: reserves < target, and each asset's reserves.
    async fn storefront_state(
        &self,
        p: &DynProvider,
        c: &Comet,
    ) -> Result<(bool, HashMap<Address, f64>)> {
        let mut calls = vec![
            call(c.address, IComet::getReservesCall {}),
            call(c.address, IComet::targetReservesCall {}),
        ];
        calls.extend(c.assets.iter().map(|a| {
            call(
                c.address,
                IComet::getCollateralReservesCall { asset: a.asset },
            )
        }));
        let res = multicall(p, &calls, 200).await?;
        let reserves = decode::<IComet::getReservesCall>(&res[0]);
        let target = decode::<IComet::targetReservesCall>(&res[1]);
        let for_sale = match (reserves, target) {
            (Some(r), Some(t)) => {
                r < alloy::primitives::I256::try_from(t).unwrap_or(alloy::primitives::I256::MAX)
            }
            _ => false,
        };
        let amounts = c
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| {
                (
                    a.asset,
                    decode::<IComet::getCollateralReservesCall>(&res[i + 2])
                        .map(f64::from)
                        .unwrap_or(0.0),
                )
            })
            .collect();
        Ok((for_sale, amounts))
    }

    /// Positions of `users` that currently owe something, plus the users read as owing nothing.
    /// Two phases: one cheap `borrowBalanceOf` per account, then the per-asset collateral reads
    /// only for debtors (most indexed `Withdraw` sources are lenders, so this cuts sweep calls
    /// ~10x). A user whose read failed is in neither list.
    async fn debtor_positions(
        &self,
        p: &DynProvider,
        c: &Comet,
        users: &[Address],
    ) -> Result<(Vec<(Address, math::Position)>, Vec<Address>)> {
        let calls: Vec<_> = users
            .iter()
            .map(|u| call(c.address, IComet::borrowBalanceOfCall { account: *u }))
            .collect();
        let res = multicall(p, &calls, 500).await?;
        let mut debtors = Vec::new();
        let mut repaid = Vec::new();
        for (u, r) in users.iter().zip(res.iter()) {
            match decode::<IComet::borrowBalanceOfCall>(r) {
                Some(b) if b.is_zero() => repaid.push(*u),
                Some(_) => debtors.push(*u),
                None => {}
            }
        }
        if debtors.is_empty() {
            return Ok((vec![], repaid));
        }
        let positions = debtors
            .iter()
            .zip(self.positions(p, c, &debtors).await?)
            .filter_map(|(u, pos)| pos.map(|pos| (*u, pos)))
            .collect();
        Ok((positions, repaid))
    }

    /// Borrow balance and per-asset collateral for borrowers of one Comet.
    async fn positions(
        &self,
        p: &DynProvider,
        c: &Comet,
        users: &[Address],
    ) -> Result<Vec<Option<math::Position>>> {
        let per = c.assets.len() + 1;
        let mut calls = Vec::with_capacity(users.len() * per);
        for u in users {
            calls.push(call(c.address, IComet::borrowBalanceOfCall { account: *u }));
            for a in &c.assets {
                calls.push(call(
                    c.address,
                    IComet::userCollateralCall {
                        account: *u,
                        asset: a.asset,
                    },
                ));
            }
        }
        let res = multicall(p, &calls, 200).await?;
        Ok(users
            .iter()
            .enumerate()
            .map(|(k, _)| {
                let r = &res[k * per..(k + 1) * per];
                let borrow = decode::<IComet::borrowBalanceOfCall>(&r[0]).map(f64::from)?;
                let collateral = c
                    .assets
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        (
                            a.asset,
                            decode::<IComet::userCollateralCall>(&r[i + 1])
                                .map(|x| x.balance as f64)
                                .unwrap_or(0.0),
                        )
                    })
                    .collect();
                Some(math::Position { borrow, collateral })
            })
            .collect())
    }

    fn market(&self, c: &Comet, prices: &Prices) -> math::Market {
        math::Market {
            base_scale: c.base_scale,
            base_price: prices.base,
            usd_per_unit: prices.usd_per_unit,
            storefront_factor: c.storefront_factor,
            assets: c
                .assets
                .iter()
                .map(|a| math::AssetParams {
                    asset: a.asset,
                    scale: a.scale,
                    price: prices.assets.get(&a.asset).copied().unwrap_or(0.0),
                    liquidate_cf: a.liquidate_cf,
                    liquidation_factor: a.liquidation_factor,
                })
                .collect(),
        }
    }
}

#[async_trait]
impl LiquidationAdapter for CompoundV3Adapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        // Withdraw(src indexed, to indexed, amount): base withdrawals create borrows for `src`.
        self.index
            .step(&self.rpc.scan, |l| {
                topic_address(l, 1)
                    .map(|src| vec![(market_key(l.address()), src)])
                    .unwrap_or_default()
            })
            .await
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let borrowers = self.index.accounts().map(|(m, a)| AccountRef {
            market: Address::from_word(*m),
            account: *a,
            sub_id: BORROWER,
        });
        let storefronts = self.comets.iter().flat_map(|c| {
            c.assets.iter().map(|a| AccountRef {
                market: c.address,
                account: a.asset,
                sub_id: STOREFRONT,
            })
        });
        borrowers.chain(storefronts).collect()
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let p = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let mut out = Vec::new();
        let mut snap = Vec::new();
        for c in &self.comets {
            let mine: Vec<&AccountRef> =
                accounts.iter().filter(|a| a.market == c.address).collect();
            if mine.is_empty() {
                continue;
            }
            let prices = self.prices(p, c).await?;
            let market = self.market(c, &prices);
            let users: Vec<Address> = mine
                .iter()
                .filter(|a| a.sub_id == BORROWER)
                .map(|a| a.account)
                .collect();
            let mut at_risk = Vec::new();
            if !users.is_empty() {
                let (positions, repaid) = self.debtor_positions(p, c, &users).await?;
                // Repaid borrowers are reported (the runner requires one answer per account and
                // makes them dormant), not dropped.
                out.extend(repaid.into_iter().map(|u| Health {
                    account: AccountRef {
                        market: c.address,
                        account: u,
                        sub_id: BORROWER,
                    },
                    health_factor: f64::INFINITY,
                    debt_usd: 0.0,
                    collateral_usd: 0.0,
                }));
                for (u, pos) in positions {
                    let u = &u;
                    if pos.borrow <= 0.0 {
                        out.push(Health {
                            account: AccountRef {
                                market: c.address,
                                account: *u,
                                sub_id: BORROWER,
                            },
                            health_factor: f64::INFINITY,
                            debt_usd: 0.0,
                            collateral_usd: 0.0,
                        });
                        continue;
                    }
                    let h = market.health(&pos);
                    if !fast && h.health_factor < SNAPSHOT_HF {
                        at_risk.push((*u, pos.clone()));
                    }
                    out.push(Health {
                        account: AccountRef {
                            market: c.address,
                            account: *u,
                            sub_id: BORROWER,
                        },
                        health_factor: h.health_factor,
                        debt_usd: h.debt_usd,
                        collateral_usd: h.collateral_usd,
                    });
                }
            }
            if !fast {
                snap.push((c.address, market.clone(), at_risk));
            }
            if mine.iter().any(|a| a.sub_id == STOREFRONT) {
                let (for_sale, reserves) = self.storefront_state(p, c).await?;
                for a in mine.iter().filter(|a| a.sub_id == STOREFRONT) {
                    let amount = reserves.get(&a.account).copied().unwrap_or(0.0);
                    let s = market.storefront(a.account, amount);
                    let worth = for_sale && s.discount_usd >= self.cfg.min_storefront_usd;
                    // Normalized so the runner treats a worthwhile sale as "liquidatable".
                    out.push(Health {
                        account: **a,
                        health_factor: if worth { 0.5 } else { 2.0 },
                        debt_usd: s.value_usd,
                        collateral_usd: s.value_usd,
                    });
                }
            }
        }
        if !fast {
            self.refresh_snapshot(snap).await;
        }
        Ok(out)
    }

    async fn plan(&self, h: &Health, executor: Address) -> Result<Option<Plan>> {
        let Some(c) = self.comet(h.account.market) else {
            return Ok(None);
        };
        let p = &self.rpc.fast;
        let prices = self.prices(p, c).await?;
        let market = self.market(c, &prices);
        let (for_sale, reserves) = self.storefront_state(p, c).await?;
        if !for_sale {
            return Ok(None); // buyCollateral reverts NotForSale while reserves >= target
        }
        let (asset, amount, absorb) = if h.account.sub_id == STOREFRONT {
            (
                h.account.account,
                reserves.get(&h.account.account).copied().unwrap_or(0.0),
                false,
            )
        } else {
            let Some(Some(pos)) = self
                .positions(p, c, &[h.account.account])
                .await?
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            if market.health(&pos).health_factor >= 1.0 {
                return Ok(None);
            }
            let Some((asset, amount)) = market.best_collateral(&pos) else {
                return Ok(None);
            };
            (asset, amount, true)
        };
        Ok(self.build_plan(
            c,
            &market,
            h.account,
            Buy {
                asset,
                amount,
                absorb,
            },
            executor,
        ))
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.keys().copied().collect()
    }

    fn predict(&self, feed: Address, answer: f64, executor: Address) -> Vec<Plan> {
        let Some(hit) = self.feed_assets.get(&feed) else {
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
        let mut plans = Vec::new();
        let elasticity = &snap.elasticity;
        for (comet_addr, cs) in snap.comets.iter_mut() {
            // Assets this aggregator prices on this Comet, with a measured response to it.
            let assets: Vec<(Address, f64)> = hit
                .iter()
                .filter(|(c, _)| c == comet_addr)
                .filter_map(|(_, a)| Some((*a, elasticity.get(&(feed, *comet_addr, *a))?.e?)))
                .collect();
            if assets.is_empty() {
                continue;
            }
            let before: Vec<f64> = cs
                .positions
                .iter()
                .map(|(_, p)| cs.market.health(p).health_factor)
                .collect();
            for a in cs.market.assets.iter_mut() {
                if let Some((_, e)) = assets.iter().find(|(x, _)| *x == a.asset) {
                    a.price *= ratio.powf(*e);
                }
            }
            if !cs.for_sale {
                continue; // buyCollateral would revert NotForSale
            }
            let Some(c) = self.comet(*comet_addr) else {
                continue;
            };
            for ((user, pos), hf_before) in cs.positions.iter().zip(before) {
                let exposed = pos
                    .collateral
                    .iter()
                    .any(|(a, amt)| *amt > 0.0 && assets.iter().any(|(x, _)| x == a));
                if !exposed || hf_before < PREDICT_HF_MAX {
                    continue; // unaffected, or already the regular re-poll's job
                }
                if cs.market.health(pos).health_factor >= PREDICT_HF_MAX {
                    continue;
                }
                let Some((asset, amount)) = cs.market.best_collateral(pos) else {
                    continue;
                };
                let account = AccountRef {
                    market: *comet_addr,
                    account: *user,
                    sub_id: BORROWER,
                };
                plans.extend(self.build_plan(
                    c,
                    &cs.market,
                    account,
                    Buy {
                        asset,
                        amount,
                        absorb: true,
                    },
                    executor,
                ));
            }
        }
        plans
    }

    fn affected(&self, feed: Address, account: &AccountRef) -> bool {
        self.feeds
            .get(&feed)
            .is_none_or(|comets| comets.contains(&account.market))
    }

    fn targets(&self) -> Vec<Address> {
        self.comets.iter().map(|c| c.address).collect()
    }

    fn assets(&self) -> Vec<Address> {
        self.comets
            .iter()
            .flat_map(|c| std::iter::once(c.base).chain(c.assets.iter().map(|a| a.asset)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Elasticity;

    #[test]
    fn plain_proxy_is_proportional_from_the_first_sample() {
        assert_eq!(Elasticity::first(2727.3, 272730000000.0).e, Some(1.0));
        assert_eq!(
            Elasticity::first(2727.3, 2.7273e21).e,
            Some(1.0),
            "rescaled proxy"
        );
        assert_eq!(
            Elasticity::first(3312.9, 2727.3e8).e,
            None,
            "composite: needs a second sample"
        );
    }

    #[test]
    fn learns_proportional_inverse_and_unrelated_responses() {
        // CAPO-style composite: price = rate × answer, rate ≈ 1.2147.
        let e = Elasticity::first(1.2147 * 2700.0, 2700e8).update(1.2147 * 2754.0, 2754e8);
        assert_eq!(e.e, Some(1.0));
        // Inverted (USDC priced in ETH from ETH/USD): ETH +2 % ⇒ USDC −1.96 %.
        let e = Elasticity::first(1e18 / 2700.0, 2700e8).update(1e18 / 2754.0, 2754e8);
        assert_eq!(e.e, Some(-1.0));
        // Unrelated move: no class.
        let e = Elasticity::first(1.0e8, 2700e8).update(1.001e8, 2754e8);
        assert_eq!(e.e, None);
    }

    #[test]
    fn tiny_moves_keep_the_old_sample() {
        let e = Elasticity::first(1.2147 * 2700.0, 2700e8);
        let e = e.update(1.2147 * 2700.1, 2700.1e8); // 0.004 %: below the measuring threshold
        assert_eq!((e.price, e.e), (1.2147 * 2700.0, None));
        assert_eq!(e.update(1.2147 * 2754.0, 2754e8).e, Some(1.0));
    }
}

#[cfg(test)]
mod slippage_floor_tests {
    use super::*;

    // The base is priced at QUOTE_HAIRCUT of the quote, so received collateral is about
    // QUOTE_HAIRCUT x amount; the floor must leave room for rounding on both sides.
    const _: () = assert!(MIN_AMOUNT_FRACTION < math::QUOTE_HAIRCUT - 1e-4);
}
