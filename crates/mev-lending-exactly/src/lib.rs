//! Exactly Protocol liquidation adapter (Base).
//!
//! - Markets: `Auditor.allMarkets()`; each is an ERC-4626 vault over its `asset()` with floating
//!   and fixed-maturity borrows. Prices are the Auditor's own (`assetPrice(priceFeed)`, USD, 18
//!   decimals); the feeds are Chainlink proxies.
//! - Accounts: borrowers from every market's `Borrow` and `BorrowAtMaturity` events (the borrower
//!   is the third indexed topic in both); one [`AccountRef`] per borrower, since health is
//!   Auditor-wide.
//! - Health: `Auditor.accountLiquidity(account, 0, 0)` gives the adjusted collateral and debt; HF =
//!   collateral / debt, below 1 exactly when `checkLiquidation` stops reverting
//!   `InsufficientShortfall`. Unadjusted debt in USD comes from each market's `accountSnapshot`.
//! - Plan: repay in the market with the largest debt, seize from the entered collateral market with
//!   the most value the market can pay out (seizing withdraws the underlying, which reverts when
//!   the market's liquidity is lent out). Amounts are the Auditor's own: `checkLiquidation` caps the
//!   repay (close factor, seizable collateral), `calculateSeize` gives the lenders' share (paid by
//!   the liquidator on top of the repay) and the collateral seized. One call:
//!   `repayMarket.liquidate(borrower, repay, seizeMarket)`, which pulls `repay + lenders` of the
//!   debt asset and transfers the seized underlying to the executor.

use std::{collections::HashSet, path::PathBuf, time::Duration};

use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use mev_config::ExactlyConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId, WatchDetail,
};
use mev_lending_index::{BackgroundIndex, EventIndex, IndexConfig, market_key, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::{info, warn};

pub const ID: ProtocolId = "exactly";

sol! {
    #[sol(rpc)]
    interface IAuditor {
        function allMarkets() external view returns (address[] memory);
        function markets(address market) external view returns (uint128 adjustFactor, uint8 decimals, uint8 index, bool isListed, address priceFeed, bool nonCollateral);
        function accountMarkets(address account) external view returns (uint256);
        function accountLiquidity(address account, address marketToSimulate, uint256 withdrawAmount) external view returns (uint256 sumCollateral, uint256 sumDebtPlusEffects);
        function assetPrice(address priceFeed) external view returns (uint256);
        function liquidationIncentive() external view returns (uint128 liquidator, uint128 lenders);
        function checkLiquidation(address repayMarket, address seizeMarket, address borrower, uint256 maxLiquidatorAssets) external view returns (uint256 maxRepayAssets);
        function calculateSeize(address repayMarket, address seizeMarket, address borrower, uint256 actualRepayAssets) external view returns (uint256 lendersAssets, uint256 seizeAssets);
    }

    #[sol(rpc)]
    interface IMarket {
        function asset() external view returns (address);
        function accountSnapshot(address account) external view returns (uint256 collateral, uint256 debt);
        function floatingAssets() external view returns (uint256);
        function floatingDebt() external view returns (uint256);
        function floatingBackupBorrowed() external view returns (uint256);
        function liquidate(address borrower, uint256 maxAssets, address seizeMarket) external returns (uint256 repaidAssets);
        event Borrow(address indexed caller, address indexed receiver, address indexed borrower, uint256 assets, uint256 shares);
        event BorrowAtMaturity(uint256 indexed maturity, address caller, address indexed receiver, address indexed borrower, uint256 assets, uint256 fee);
    }
}

/// 1e18: Auditor prices, adjust factors and incentives.
const WAD: f64 = 1e18;
/// Reads per multicall request.
const CHUNK: usize = 200;
/// The Auditor's marker for a feed that is the base unit itself.
const BASE_FEED: Address = Address::new([0xee; 20]);

#[derive(Debug, Clone)]
struct Market {
    market: Address,
    asset: Address,
    decimals: u8,
    price_feed: Address,
    non_collateral: bool,
}

pub struct ExactlyAdapter {
    cfg: ExactlyConfig,
    rpc: AdapterRpc,
    markets: Vec<Market>,
    /// Liquidator and lenders incentives (fractions, 0.05 = 5%).
    incentive: (f64, f64),
    index: BackgroundIndex,
    feeds: Vec<Address>,
}

impl ExactlyAdapter {
    pub async fn new(cfg: ExactlyConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let auditor = IAuditor::new(cfg.auditor, rpc.fast.clone());
        let list = auditor.allMarkets().call().await.context("allMarkets")?;
        let inc = auditor
            .liquidationIncentive()
            .call()
            .await
            .context("liquidationIncentive")?;
        let mut reads = Vec::new();
        for m in &list {
            reads.push(call(cfg.auditor, IAuditor::marketsCall { market: *m }));
            reads.push(call(*m, IMarket::assetCall {}));
        }
        let res = multicall(&rpc.fast, &reads, CHUNK).await?;
        let markets: Vec<Market> = list
            .iter()
            .zip(res.chunks(2))
            .filter_map(|(m, r)| {
                let d = decode::<IAuditor::marketsCall>(&r[0])?;
                d.isListed.then_some(Market {
                    market: *m,
                    asset: decode::<IMarket::assetCall>(&r[1])?,
                    decimals: d.decimals,
                    price_feed: d.priceFeed,
                    non_collateral: d.nonCollateral,
                })
            })
            .collect();
        anyhow::ensure!(!markets.is_empty(), "exactly: no listed market answered");
        let index = BackgroundIndex::new(EventIndex::open(IndexConfig {
            addresses: markets.iter().map(|m| m.market).collect(),
            topics: vec![
                IMarket::Borrow::SIGNATURE_HASH,
                IMarket::BorrowAtMaturity::SIGNATURE_HASH,
            ],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        }));
        let sources: Vec<Address> = markets
            .iter()
            .map(|m| m.price_feed)
            .filter(|f| *f != BASE_FEED)
            .collect();
        let feeds = match mev_oracle::resolve_aggregators(&rpc.fast, &sources).await {
            Ok(a) => a.into_keys().collect(),
            Err(e) => {
                warn!(protocol = ID, error = %format!("{e:#}"), "price feeds not resolved; polling only");
                Vec::new()
            }
        };
        let incentive = (inc.liquidator as f64 / WAD, inc.lenders as f64 / WAD);
        info!(
            protocol = ID,
            markets = markets.len(),
            feeds = feeds.len(),
            liquidator_incentive = incentive.0,
            lenders_incentive = incentive.1,
            borrowers = index.len(),
            "exactly adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            markets,
            incentive,
            index,
            feeds,
        })
    }

    /// Auditor price (USD, 18 decimals) of every market's asset.
    async fn prices(&self, p: &DynProvider) -> Result<Vec<Option<U256>>> {
        let reads: Vec<_> = self
            .markets
            .iter()
            .map(|m| {
                call(
                    self.cfg.auditor,
                    IAuditor::assetPriceCall {
                        priceFeed: m.price_feed,
                    },
                )
            })
            .collect();
        Ok(multicall(p, &reads, CHUNK)
            .await?
            .iter()
            .map(|r| decode::<IAuditor::assetPriceCall>(r).filter(|x| !x.is_zero()))
            .collect())
    }
}

/// USD value of `raw` units of an asset with `decimals`, at an Auditor price (18 decimals).
fn usd(raw: U256, price: U256, decimals: u8) -> f64 {
    f64::from(raw) / 10f64.powi(decimals as i32) * f64::from(price) / WAD
}

/// Health factor from the Auditor's adjusted sums: below 1 exactly when collateral < debt.
fn health_factor(adjusted_collateral: U256, adjusted_debt: U256) -> f64 {
    if adjusted_debt.is_zero() {
        return f64::INFINITY;
    }
    f64::from(adjusted_collateral) / f64::from(adjusted_debt)
}

/// Underlying a market can pay out now: what it holds less what is lent (floating and backup).
fn payable(floating_assets: U256, floating_debt: U256, backup_borrowed: U256) -> U256 {
    floating_assets.saturating_sub(floating_debt + backup_borrowed)
}

/// Flash amount for a repay: `repay + lenders` is what `liquidate` pulls; a 1e-4 (and one unit)
/// margin covers interest accrued between the read and the landing. The rest is returned.
fn flash_for(repay: U256, lenders: U256) -> U256 {
    let owed = repay + lenders;
    owed + owed / U256::from(10_000u64) + U256::from(1u8)
}

#[async_trait]
impl LiquidationAdapter for ExactlyAdapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        // Borrow(caller, receiver, borrower, ...) and BorrowAtMaturity(maturity, receiver, borrower, ...):
        // the borrower is topic 3 in both.
        Ok(self.index.run(&self.rpc.scan, move |l| {
            topic_address(l, 3)
                .map(|b| vec![(market_key(l.address()), b)])
                .unwrap_or_default()
        }))
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let borrowers: HashSet<Address> = self.index.accounts().iter().map(|(_, a)| *a).collect();
        borrowers
            .into_iter()
            .map(|a| AccountRef {
                market: self.cfg.auditor,
                account: a,
                sub_id: 0,
            })
            .collect()
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let p = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let prices = self.prices(p).await?;
        let n = self.markets.len();
        let mut reads = Vec::with_capacity(accounts.len() * (n + 1));
        for a in accounts {
            reads.push(call(
                self.cfg.auditor,
                IAuditor::accountLiquidityCall {
                    account: a.account,
                    marketToSimulate: Address::ZERO,
                    withdrawAmount: U256::ZERO,
                },
            ));
            for m in &self.markets {
                reads.push(call(
                    m.market,
                    IMarket::accountSnapshotCall { account: a.account },
                ));
            }
        }
        let res = multicall(p, &reads, CHUNK).await?;
        let mut out = Vec::with_capacity(accounts.len());
        for (a, r) in accounts.iter().zip(res.chunks(n + 1)) {
            let Some(liq) = decode::<IAuditor::accountLiquidityCall>(&r[0]) else {
                continue;
            };
            let mut debt_usd = 0.0;
            for (k, m) in self.markets.iter().enumerate() {
                if let (Some(s), Some(px)) =
                    (decode::<IMarket::accountSnapshotCall>(&r[1 + k]), prices[k])
                {
                    debt_usd += usd(s.debt, px, m.decimals);
                }
            }
            out.push(Health {
                account: *a,
                health_factor: health_factor(liq.sumCollateral, liq.sumDebtPlusEffects),
                debt_usd,
                // Adjusted collateral (what the Auditor compares against the adjusted debt), USD.
                collateral_usd: f64::from(liq.sumCollateral) / WAD,
            });
        }
        Ok(out)
    }

    async fn plan(&self, h: &Health, _executor: Address) -> Result<Option<Plan>> {
        let p = &self.rpc.fast;
        let who = h.account.account;
        let auditor = IAuditor::new(self.cfg.auditor, p.clone());
        let prices = self.prices(p).await?;
        let entered = auditor.accountMarkets(who).call().await?;
        let mut reads = Vec::new();
        for m in &self.markets {
            reads.push(call(
                m.market,
                IMarket::accountSnapshotCall { account: who },
            ));
            reads.push(call(m.market, IMarket::floatingAssetsCall {}));
            reads.push(call(m.market, IMarket::floatingDebtCall {}));
            reads.push(call(m.market, IMarket::floatingBackupBorrowedCall {}));
        }
        let res = multicall(p, &reads, CHUNK).await?;
        // Largest debt; and the entered collateral market with the most value it can pay out.
        let mut debt: Option<(usize, f64)> = None;
        let mut coll: Option<(usize, f64)> = None;
        for (k, (m, r)) in self.markets.iter().zip(res.chunks(4)).enumerate() {
            let (Some(s), Some(px)) = (decode::<IMarket::accountSnapshotCall>(&r[0]), prices[k])
            else {
                continue;
            };
            let d = usd(s.debt, px, m.decimals);
            if d > debt.map_or(0.0, |x| x.1) {
                debt = Some((k, d));
            }
            let is_in = (entered >> k) & U256::from(1u8) == U256::from(1u8);
            if !is_in || m.non_collateral {
                continue;
            }
            let cash = match (
                decode::<IMarket::floatingAssetsCall>(&r[1]),
                decode::<IMarket::floatingDebtCall>(&r[2]),
                decode::<IMarket::floatingBackupBorrowedCall>(&r[3]),
            ) {
                (Some(a), Some(d), Some(b)) => payable(a, d, b),
                _ => continue,
            };
            let c = usd(s.collateral.min(cash), px, m.decimals);
            if c > coll.map_or(0.0, |x| x.1) {
                coll = Some((k, c));
            }
        }
        let (Some((dk, _)), Some((ck, _))) = (debt, coll) else {
            return Ok(None);
        };
        let (dm, cm) = (&self.markets[dk], &self.markets[ck]);
        // The Auditor's own cap (close factor, seizable collateral); reverts when not liquidatable.
        let Ok(mut repay) = auditor
            .checkLiquidation(dm.market, cm.market, who, U256::MAX)
            .call()
            .await
        else {
            return Ok(None);
        };
        let cash = {
            let r = &res[ck * 4..ck * 4 + 4];
            payable(
                decode::<IMarket::floatingAssetsCall>(&r[1]).unwrap_or_default(),
                decode::<IMarket::floatingDebtCall>(&r[2]).unwrap_or_default(),
                decode::<IMarket::floatingBackupBorrowedCall>(&r[3]).unwrap_or_default(),
            )
        };
        let seize_of = |repay: U256| {
            let req = auditor.calculateSeize(dm.market, cm.market, who, repay);
            async move { req.call().await.ok() }
        };
        let Some(mut s) = seize_of(repay).await else {
            return Ok(None);
        };
        if s.seizeAssets > cash {
            // Seize is linear in the repay: scale it to what the market can pay out, then re-ask.
            repay = repay * cash / s.seizeAssets;
            match seize_of(repay).await {
                Some(x) if x.seizeAssets <= cash => s = x,
                _ => return Ok(None),
            }
        }
        if repay.is_zero() || s.seizeAssets.is_zero() {
            return Ok(None);
        }
        let (Some(pd), Some(pc)) = (prices[dk], prices[ck]) else {
            return Ok(None);
        };
        let flash = flash_for(repay, s.lendersAssets);
        let bonus_usd =
            usd(s.seizeAssets, pc, cm.decimals) - usd(repay + s.lendersAssets, pd, dm.decimals);
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: dm.asset,
            collateral_asset: cm.asset,
            flash_amount: flash,
            dependencies: vec![],
            expected_collateral: f64::from(s.seizeAssets),
            bonus_usd,
            calls: vec![ProtocolCall {
                target: dm.market,
                data: IMarket::liquidateCall {
                    borrower: who,
                    maxAssets: repay,
                    seizeMarket: cm.market,
                }
                .abi_encode()
                .into(),
                approve_spender: dm.market,
                approve_amount: flash,
            }],
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.clone()
    }

    /// Collateral is pooled across markets: no single liquidation price. The liquidator seizes
    /// `repay × (1 + liquidator + lenders)` and pays `repay × (1 + lenders)`, so the bonus on the
    /// repay is `liquidator` (5% on Base, 2026-10-25).
    fn watch_detail(&self, h: &Health) -> WatchDetail {
        WatchDetail {
            max_repay_usd: Some(h.debt_usd),
            bonus_rate: Some(self.incentive.0),
            ..WatchDetail::default()
        }
    }

    fn targets(&self) -> Vec<Address> {
        self.markets.iter().map(|m| m.market).collect()
    }

    fn assets(&self) -> Vec<Address> {
        let mut a: Vec<Address> = self.markets.iter().map(|m| m.asset).collect();
        a.sort_unstable();
        a.dedup();
        a
    }

    /// A deposit into a collateral market restores the liquidity a seize withdraws.
    fn cash_markets(&self) -> Vec<Address> {
        self.markets.iter().map(|m| m.market).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(n: u64, d: u8) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(d))
    }

    #[test]
    fn health_is_below_one_exactly_when_adjusted_collateral_is_short() {
        assert!(health_factor(e(99, 18), e(100, 18)) < 1.0);
        assert!(health_factor(e(100, 18), e(100, 18)) >= 1.0);
        assert!(health_factor(e(1, 18), U256::ZERO).is_infinite());
    }

    #[test]
    fn usd_scales_by_decimals_and_an_18_decimal_price() {
        // 2.5 WETH at $2,000.
        assert_eq!(usd(e(25, 17), e(2_000, 18), 18), 5_000.0);
        // 1,000 USDC (6 decimals) at $1.
        assert_eq!(usd(e(1_000, 6), e(1, 18), 6), 1_000.0);
    }

    #[test]
    fn a_market_pays_out_what_is_not_lent() {
        assert_eq!(payable(e(100, 6), e(60, 6), e(10, 6)), e(30, 6));
        assert_eq!(payable(e(50, 6), e(60, 6), U256::ZERO), U256::ZERO);
    }

    #[test]
    fn the_flash_loan_covers_repay_and_lenders_share_with_a_margin() {
        let f = flash_for(U256::from(1_000_000u64), U256::from(2_500u64));
        assert_eq!(f, U256::from(1_002_500u64 + 100 + 1));
    }
}
