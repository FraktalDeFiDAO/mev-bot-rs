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

use std::{collections::HashMap, path::PathBuf, time::Duration};

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
use tracing::info;

pub const ID: ProtocolId = "compound-v3";
/// `AccountRef::sub_id` values.
pub const BORROWER: u64 = 0;
pub const STOREFRONT: u64 = 1;

sol! {
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
    feeds: Vec<Address>,
}

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
        let feeds = mev_oracle::resolve_aggregators(&rpc.fast, &sources)
            .await?
            .into_keys()
            .collect::<Vec<_>>();
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
        })
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
            if !users.is_empty() {
                for (u, pos) in users.iter().zip(self.positions(p, c, &users).await?) {
                    let Some(pos) = pos else { continue };
                    if pos.borrow <= 0.0 {
                        continue;
                    }
                    let h = market.health(&pos);
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
        let Some(q) = market.quote(asset, amount) else {
            return Ok(None);
        };
        if q.base_amount < 1.0 {
            return Ok(None);
        }
        let base_amount = U256::from(q.base_amount as u128);
        let mut calls = Vec::new();
        if absorb {
            calls.push(ProtocolCall {
                target: c.address,
                data: IComet::absorbCall {
                    absorber: executor,
                    accounts: vec![h.account.account],
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
                minAmount: U256::from((amount * 0.999) as u128),
                baseAmount: base_amount,
                recipient: executor,
            }
            .abi_encode()
            .into(),
            approve_spender: c.address,
            approve_amount: base_amount,
        });
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: c.base,
            collateral_asset: asset,
            flash_amount: base_amount,
            expected_collateral: amount,
            bonus_usd: q.discount_usd,
            calls,
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.clone()
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
