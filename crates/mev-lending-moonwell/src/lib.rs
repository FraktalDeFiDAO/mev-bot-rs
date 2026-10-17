//! Moonwell (Compound v2 fork) liquidation adapter.
//!
//! - Accounts: borrowers from every market's `Borrow` event (non-indexed: the borrower is the first
//!   data word), keyed by the borrowed market; one [`AccountRef`] per borrower, since health is
//!   comptroller-wide.
//! - Health: `getAccountLiquidity` gives the risk-weighted surplus (`liquidity`) or deficit
//!   (`shortfall`) in USD; debt is `Σ borrowBalanceStored × getUnderlyingPrice` over the markets the
//!   borrower borrowed from. HF = (debt + liquidity − shortfall) / debt, so HF < 1 exactly when the
//!   comptroller allows a liquidation.
//! - Plan: repay the largest debt (up to the close factor) with flash-borrowed underlying through
//!   `liquidateBorrow`, seizing mTokens of the largest collateral, then `redeem` them for the
//!   underlying that the engine swaps back. The seized amount comes from the comptroller itself
//!   (`liquidateCalculateSeizeTokens`), less the protocol's seize share.
//!
//! Prices are Moonwell's own oracle (`getUnderlyingPrice`, scaled 1e(36 − underlying decimals)).
//! Base has no public sequencer feed, so there are no price-feed triggers: the runner polls.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use alloy::{
    primitives::{Address, Bytes, U256},
    providers::DynProvider,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use mev_config::MoonwellConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, market_key};
use mev_rpc::{call, decode, multicall};
use tracing::info;

pub const ID: ProtocolId = "moonwell";

sol! {
    #[sol(rpc)]
    interface IComptroller {
        function getAllMarkets() external view returns (address[] memory);
        function oracle() external view returns (address);
        function closeFactorMantissa() external view returns (uint256);
        function liquidationIncentiveMantissa() external view returns (uint256);
        function getAccountLiquidity(address account) external view returns (uint256 err, uint256 liquidity, uint256 shortfall);
        function getAssetsIn(address account) external view returns (address[] memory);
        function liquidateCalculateSeizeTokens(address mTokenBorrowed, address mTokenCollateral, uint256 repayAmount) external view returns (uint256 err, uint256 seizeTokens);
    }

    #[sol(rpc)]
    interface IMToken {
        function underlying() external view returns (address);
        function protocolSeizeShareMantissa() external view returns (uint256);
        function borrowBalanceStored(address account) external view returns (uint256);
        function getAccountSnapshot(address account) external view returns (uint256 err, uint256 balance, uint256 borrowed, uint256 exchangeRate);
        function getCash() external view returns (uint256);
        function liquidateBorrow(address borrower, uint256 repayAmount, address mTokenCollateral) external returns (uint256);
        function redeem(uint256 redeemTokens) external returns (uint256);
        event Borrow(address borrower, uint256 borrowAmount, uint256 accountBorrows, uint256 totalBorrows);
    }

    #[sol(rpc)]
    interface IOracle {
        function getUnderlyingPrice(address mToken) external view returns (uint256);
    }
}

/// 1e18 (Compound mantissa).
const MANTISSA: f64 = 1e18;
/// `getUnderlyingPrice` × raw amount / this = USD.
const PRICE_SCALE: f64 = 1e36;
/// Repay this share of the computed maximum (interest accrues between read and send).
const REPAY_HAIRCUT_BPS: u64 = 9_950;
/// Redeem this share of the seized mTokens: the collateral market accrues interest inside
/// `liquidateBorrow`, so the exact seize can differ slightly from the read; the dust stays in the
/// executor rather than reverting the redeem.
const REDEEM_HAIRCUT_BPS: u64 = 9_990;
const BPS: u64 = 10_000;
/// Share of a market's cash a redeem may take: interest and other users move it before we land.
const CASH_USE_BPS: f64 = 9_900.0;
/// Health reads per multicall request.
const CHUNK: usize = 200;

#[derive(Debug, Clone)]
struct Market {
    mtoken: Address,
    underlying: Address,
    protocol_seize_share: U256,
}

pub struct MoonwellAdapter {
    cfg: MoonwellConfig,
    rpc: AdapterRpc,
    markets: Vec<Market>,
    oracle: Address,
    close_factor: U256,
    incentive: U256,
    index: EventIndex,
}

impl MoonwellAdapter {
    pub async fn new(cfg: MoonwellConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let c = IComptroller::new(cfg.comptroller, rpc.fast.clone());
        let mtokens = c.getAllMarkets().call().await.context("getAllMarkets")?;
        let oracle = c.oracle().call().await.context("oracle")?;
        let close_factor = c.closeFactorMantissa().call().await?;
        let incentive = c.liquidationIncentiveMantissa().call().await?;
        let mut reads = Vec::new();
        for m in &mtokens {
            reads.push(call(*m, IMToken::underlyingCall {}));
            reads.push(call(*m, IMToken::protocolSeizeShareMantissaCall {}));
        }
        let res = multicall(&rpc.fast, &reads, CHUNK).await?;
        let markets: Vec<Market> = mtokens
            .iter()
            .zip(res.chunks(2))
            .filter_map(|(m, r)| {
                Some(Market {
                    mtoken: *m,
                    underlying: decode::<IMToken::underlyingCall>(&r[0])?,
                    protocol_seize_share: decode::<IMToken::protocolSeizeShareMantissaCall>(&r[1])
                        .unwrap_or_default(),
                })
            })
            .collect();
        anyhow::ensure!(!markets.is_empty(), "moonwell: no market answered");
        let index = EventIndex::open(IndexConfig {
            addresses: markets.iter().map(|m| m.mtoken).collect(),
            topics: vec![IMToken::Borrow::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        });
        info!(
            protocol = ID,
            markets = markets.len(),
            skipped = mtokens.len() - markets.len(),
            close_factor = %close_factor,
            incentive = %incentive,
            borrowers = index.len(),
            "moonwell adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            markets,
            oracle,
            close_factor,
            incentive,
            index,
        })
    }

    fn market(&self, mtoken: Address) -> Option<&Market> {
        self.markets.iter().find(|m| m.mtoken == mtoken)
    }

    /// Borrower -> markets it has borrowed from (from the index).
    fn borrowed(&self) -> HashMap<Address, Vec<Address>> {
        let mut out: HashMap<Address, Vec<Address>> = HashMap::new();
        for (m, a) in self.index.accounts() {
            out.entry(*a).or_default().push(Address::from_word(*m));
        }
        out
    }

    async fn prices(&self, p: &DynProvider, mtokens: &[Address]) -> Result<HashMap<Address, U256>> {
        let reads: Vec<_> = mtokens
            .iter()
            .map(|m| call(self.oracle, IOracle::getUnderlyingPriceCall { mToken: *m }))
            .collect();
        let res = multicall(p, &reads, CHUNK).await?;
        Ok(mtokens
            .iter()
            .zip(&res)
            .filter_map(|(m, r)| Some((*m, decode::<IOracle::getUnderlyingPriceCall>(r)?)))
            .filter(|(_, px)| !px.is_zero())
            .collect())
    }
}

/// USD value of `raw` underlying at an oracle price scaled 1e(36 − decimals).
fn usd(raw: U256, price: U256) -> f64 {
    f64::from(raw) * f64::from(price) / PRICE_SCALE
}

/// HF from the comptroller's view: (debt + liquidity − shortfall) / debt; no debt = infinite.
fn health_factor(debt_usd: f64, liquidity_usd: f64, shortfall_usd: f64) -> f64 {
    if debt_usd <= 0.0 {
        return f64::INFINITY;
    }
    ((debt_usd + liquidity_usd - shortfall_usd) / debt_usd).max(0.0)
}

/// Underlying to repay: the close-factor share of the debt, capped so the seized collateral
/// (repay × incentive, in USD) fits in the collateral, then shaved by the haircut.
fn repay_amount(
    borrowed: U256,
    close_factor: U256,
    debt_price: U256,
    collateral_usd: f64,
    incentive: U256,
) -> U256 {
    let by_close = borrowed * close_factor / U256::from(10u64).pow(U256::from(18u8));
    let incentive = f64::from(incentive) / MANTISSA;
    let debt_px = f64::from(debt_price) / PRICE_SCALE;
    let by_collateral = if debt_px > 0.0 && incentive > 0.0 {
        U256::from((collateral_usd / incentive / debt_px).max(0.0) as u128)
    } else {
        U256::ZERO
    };
    by_close.min(by_collateral) * U256::from(REPAY_HAIRCUT_BPS) / U256::from(BPS)
}

/// Collateral value a liquidation can turn into the underlying: at most a share of the market's cash.
fn redeemable_usd(collateral_usd: f64, cash_usd: f64) -> f64 {
    collateral_usd.min(cash_usd * CASH_USE_BPS / BPS as f64)
}

/// mTokens the liquidator keeps of `seize` (the protocol takes its seize share) and redeems.
fn redeem_tokens(seize: U256, protocol_share: U256) -> U256 {
    let one = U256::from(10u64).pow(U256::from(18u8));
    let kept = seize - seize * protocol_share / one;
    kept * U256::from(REDEEM_HAIRCUT_BPS) / U256::from(BPS)
}

#[async_trait]
impl LiquidationAdapter for MoonwellAdapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        // Shadow-first: no executor is deployed on Base, so plans are recorded, never sent.
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        // Borrow(borrower, borrowAmount, accountBorrows, totalBorrows): nothing indexed.
        self.index
            .step(&self.rpc.scan, |l| {
                let d = &l.data().data;
                if d.len() < 32 {
                    return Vec::new();
                }
                let borrower = Address::from_slice(&d[12..32]);
                vec![(market_key(l.address()), borrower)]
            })
            .await
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let borrowers: HashSet<Address> = self.index.accounts().map(|(_, a)| *a).collect();
        borrowers
            .into_iter()
            .map(|a| AccountRef {
                market: self.cfg.comptroller,
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
        let borrowed = self.borrowed();
        let all: Vec<Address> = self.markets.iter().map(|m| m.mtoken).collect();
        let prices = self.prices(p, &all).await?;
        // Per account: getAccountLiquidity, then borrowBalanceStored on each borrowed market.
        let mut reads = Vec::new();
        let mut layout = Vec::with_capacity(accounts.len());
        for a in accounts {
            let ms: Vec<Address> = borrowed.get(&a.account).cloned().unwrap_or_default();
            reads.push(call(
                self.cfg.comptroller,
                IComptroller::getAccountLiquidityCall { account: a.account },
            ));
            for m in &ms {
                reads.push(call(
                    *m,
                    IMToken::borrowBalanceStoredCall { account: a.account },
                ));
            }
            layout.push(ms);
        }
        let res = multicall(p, &reads, CHUNK).await?;
        let mut out = Vec::with_capacity(accounts.len());
        let mut i = 0;
        for (a, ms) in accounts.iter().zip(layout) {
            let liq = decode::<IComptroller::getAccountLiquidityCall>(&res[i]);
            let mut debt_usd = 0.0;
            for (k, m) in ms.iter().enumerate() {
                if let (Some(b), Some(px)) = (
                    decode::<IMToken::borrowBalanceStoredCall>(&res[i + 1 + k]),
                    prices.get(m),
                ) {
                    debt_usd += usd(b, *px);
                }
            }
            i += 1 + ms.len();
            let Some(l) = liq.filter(|l| l.err.is_zero()) else {
                continue;
            };
            let (liquidity, shortfall) = (
                f64::from(l.liquidity) / MANTISSA,
                f64::from(l.shortfall) / MANTISSA,
            );
            out.push(Health {
                account: *a,
                health_factor: health_factor(debt_usd, liquidity, shortfall),
                debt_usd,
                // Risk-weighted collateral (what the comptroller compares against the debt).
                collateral_usd: (debt_usd + liquidity - shortfall).max(0.0),
            });
        }
        Ok(out)
    }

    async fn plan(&self, h: &Health, _executor: Address) -> Result<Option<Plan>> {
        let p = &self.rpc.fast;
        let who = h.account.account;
        let c = IComptroller::new(self.cfg.comptroller, p.clone());
        let assets_in = c.getAssetsIn(who).call().await?;
        let mut ms: Vec<Address> = assets_in;
        ms.extend(self.borrowed().remove(&who).unwrap_or_default());
        ms.sort_unstable();
        ms.dedup();
        ms.retain(|m| self.market(*m).is_some());
        let prices = self.prices(p, &ms).await?;
        let mut reads: Vec<(Address, Bytes)> = ms
            .iter()
            .map(|m| call(*m, IMToken::getAccountSnapshotCall { account: who }))
            .collect();
        reads.extend(ms.iter().map(|m| call(*m, IMToken::getCashCall {})));
        let res = multicall(p, &reads, CHUNK).await?;
        let (snaps, cash) = res.split_at(ms.len());
        // Largest debt, and the collateral with the most value we can actually redeem: the seized
        // mTokens are redeemed for the underlying, which fails when the market's cash is lent out
        // (Base cbETH 2026-10-17: $20k collateral, zero cash, liquidatable for hours, untouched).
        let mut debt: Option<(Address, U256, f64)> = None;
        let mut coll: Option<(Address, f64)> = None;
        for (k, (m, r)) in ms.iter().zip(snaps).enumerate() {
            let (Some(s), Some(px)) = (decode::<IMToken::getAccountSnapshotCall>(r), prices.get(m))
            else {
                continue;
            };
            if !s.err.is_zero() {
                continue;
            }
            let d = usd(s.borrowed, *px);
            if d > debt.map_or(0.0, |x| x.2) {
                debt = Some((*m, s.borrowed, d));
            }
            let underlying = s.balance * s.exchangeRate / U256::from(10u64).pow(U256::from(18u8));
            let cash_usd = decode::<IMToken::getCashCall>(&cash[k]).map_or(0.0, |c| usd(c, *px));
            let cu = redeemable_usd(usd(underlying, *px), cash_usd);
            if cu > coll.map_or(0.0, |x| x.1) {
                coll = Some((*m, cu));
            }
        }
        let (Some((dm, borrowed, _)), Some((cm, coll_usd))) = (debt, coll) else {
            return Ok(None);
        };
        let repay = repay_amount(
            borrowed,
            self.close_factor,
            prices[&dm],
            coll_usd,
            self.incentive,
        );
        if repay.is_zero() {
            return Ok(None);
        }
        let seize = c
            .liquidateCalculateSeizeTokens(dm, cm, repay)
            .call()
            .await
            .context("liquidateCalculateSeizeTokens")?;
        if !seize.err.is_zero() || seize.seizeTokens.is_zero() {
            return Ok(None);
        }
        let (Some(debt_m), Some(coll_m)) = (self.market(dm), self.market(cm)) else {
            return Ok(None);
        };
        let redeem = redeem_tokens(seize.seizeTokens, coll_m.protocol_seize_share);
        let rate = snaps[ms.iter().position(|m| *m == cm).unwrap_or_default()]
            .as_ref()
            .and_then(|r| IMToken::getAccountSnapshotCall::abi_decode_returns(r).ok())
            .map(|s| s.exchangeRate)
            .unwrap_or_default();
        let collateral_out = redeem * rate / U256::from(10u64).pow(U256::from(18u8));
        let bonus_usd = usd(collateral_out, prices[&cm]) - usd(repay, prices[&dm]);
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: debt_m.underlying,
            collateral_asset: coll_m.underlying,
            flash_amount: repay,
            dependencies: vec![],
            expected_collateral: f64::from(collateral_out),
            bonus_usd,
            calls: vec![
                ProtocolCall {
                    target: dm,
                    data: IMToken::liquidateBorrowCall {
                        borrower: who,
                        repayAmount: repay,
                        mTokenCollateral: cm,
                    }
                    .abi_encode()
                    .into(),
                    approve_spender: dm,
                    approve_amount: repay,
                },
                ProtocolCall {
                    target: cm,
                    data: IMToken::redeemCall {
                        redeemTokens: redeem,
                    }
                    .abi_encode()
                    .into(),
                    approve_spender: Address::ZERO,
                    approve_amount: U256::ZERO,
                },
            ],
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        Vec::new()
    }

    fn targets(&self) -> Vec<Address> {
        self.markets.iter().map(|m| m.mtoken).collect()
    }

    fn assets(&self) -> Vec<Address> {
        let mut a: Vec<Address> = self.markets.iter().map(|m| m.underlying).collect();
        a.sort_unstable();
        a.dedup();
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e18(x: u64) -> U256 {
        U256::from(x) * U256::from(10u64).pow(U256::from(18u8))
    }

    #[test]
    fn health_is_below_one_exactly_when_the_comptroller_reports_a_shortfall() {
        assert!(health_factor(1000.0, 0.0, 10.0) < 1.0);
        assert!(health_factor(1000.0, 10.0, 0.0) > 1.0);
        assert_eq!(health_factor(1000.0, 0.0, 0.0), 1.0);
        assert_eq!(health_factor(0.0, 50.0, 0.0), f64::INFINITY);
        assert_eq!(health_factor(100.0, 0.0, 500.0), 0.0);
    }

    #[test]
    fn usd_uses_the_oracle_scale_of_36_minus_decimals() {
        // 1 WETH (18 decimals) at $2,425: price scaled 1e18.
        assert!(
            (usd(
                e18(1),
                U256::from(2425u64) * U256::from(10u64).pow(U256::from(18u8))
            ) - 2425.0)
                .abs()
                < 1e-6
        );
        // 1,000 USDC (6 decimals) at $1: price scaled 1e30.
        let usdc_px = U256::from(10u64).pow(U256::from(30u8));
        assert!((usd(U256::from(1_000_000_000u64), usdc_px) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn repay_is_half_the_debt_unless_the_collateral_is_smaller() {
        let half = U256::from(5u64) * U256::from(10u64).pow(U256::from(17u8));
        let usdc_px = U256::from(10u64).pow(U256::from(30u8));
        let inc = U256::from(11u64) * U256::from(10u64).pow(U256::from(17u8));
        // 1,000 USDC debt, $5,000 collateral: close factor binds (500 USDC), less 0.5%.
        let r = repay_amount(U256::from(1_000_000_000u64), half, usdc_px, 5000.0, inc);
        assert_eq!(r, U256::from(497_500_000u64));
        // $110 collateral at a 10% incentive covers only 100 USDC of repayment.
        let r = repay_amount(U256::from(1_000_000_000u64), half, usdc_px, 110.0, inc);
        assert!(r <= U256::from(99_500_000u64) && r > U256::from(99_400_000u64));
    }

    #[test]
    fn collateral_is_capped_by_the_markets_cash() {
        assert_eq!(redeemable_usd(20_000.0, 0.0), 0.0);
        assert_eq!(redeemable_usd(500.0, 1_000_000.0), 500.0);
        assert!((redeemable_usd(20_000.0, 1_000.0) - 990.0).abs() < 1e-9);
        // A cash-capped collateral also caps the repayment (seize = repay x incentive).
        let half = U256::from(5u64) * U256::from(10u64).pow(U256::from(17u8));
        let usdc_px = U256::from(10u64).pow(U256::from(30u8));
        let inc = U256::from(11u64) * U256::from(10u64).pow(U256::from(17u8));
        let r = repay_amount(
            U256::from(10_000_000_000u64),
            half,
            usdc_px,
            redeemable_usd(20_000.0, 1_100.0),
            inc,
        );
        assert!(r < U256::from(990_000_000u64));
    }

    #[test]
    fn the_protocol_seize_share_is_not_redeemed() {
        // 3% protocol share, then the 0.1% redeem haircut.
        let share = U256::from(3u64) * U256::from(10u64).pow(U256::from(16u8));
        assert_eq!(
            redeem_tokens(U256::from(1_000_000u64), share),
            U256::from(969_030u64)
        );
    }
}
