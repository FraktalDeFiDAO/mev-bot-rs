//! Euler V2 (Euler Vault Kit) liquidation adapter.
//!
//! - Vaults: every proxy the EVK factory lists; the ones that accept collateral (`LTVList` not
//!   empty) can be a borrower's controller and are watched.
//! - Accounts: borrowers from each watched vault's `Borrow(account, assets)` (account indexed);
//!   one [`AccountRef`] per (controller vault, account), since an EVC account has one controller.
//! - Health: `controller.accountLiquidity(account, true)` gives the liquidation-LTV-weighted
//!   collateral value and the liability value in the vault's unit of account; HF = collateral /
//!   liability, below 1 exactly when `checkLiquidation` offers a repay. An account that owes the
//!   vault nothing (`debtOf` = 0) is reported healthy without that call, which reverts for it.
//! - Plan: for each enabled collateral, `checkLiquidation(executor, violator, collateral)` gives
//!   the exact (max repay, max yield in collateral shares); the largest repay wins, capped by
//!   the collateral vault's cash (the shares are redeemed). The executor then calls the EVC twice:
//!   `enableController(executor, controller)`, and one `batch` (account checks deferred to its
//!   end) of `controller.liquidate(violator, collateral, repay, 0)` (the debt and the shares move to
//!   the executor), `collateral.redeem(max, executor, executor)`, `controller.repay(max, executor)`
//!   from the flash-borrowed debt asset, and `controller.disableController()`.
//!
//! Prices: each vault's own oracle router (`getQuote`) in its unit of account; USD-denominated
//! vaults use address(840), others a token priced through `usd_oracle` (Aave's). No price-feed
//! triggers yet: the routers' sources are not resolved, so accounts are polled.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use alloy::{
    primitives::{Address, Bytes, U256, address},
    providers::DynProvider,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use mev_bindings::IAaveOracle;
use mev_config::EulerConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, market_key, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::info;

pub const ID: ProtocolId = "euler-v2";

sol! {
    #[sol(rpc)]
    interface IFactory {
        function getProxyListLength() external view returns (uint256);
        function getProxyListSlice(uint256 start, uint256 end) external view returns (address[] memory);
    }

    #[sol(rpc)]
    interface IEVault {
        function asset() external view returns (address);
        function unitOfAccount() external view returns (address);
        function oracle() external view returns (address);
        function LTVList() external view returns (address[] memory);
        function debtOf(address account) external view returns (uint256);
        function cash() external view returns (uint256);
        function convertToAssets(uint256 shares) external view returns (uint256);
        function accountLiquidity(address account, bool liquidation) external view returns (uint256 collateralValue, uint256 liabilityValue);
        function checkLiquidation(address liquidator, address violator, address collateral) external view returns (uint256 maxRepay, uint256 maxYield);
        function liquidate(address violator, address collateral, uint256 repayAssets, uint256 minYieldBalance) external;
        function redeem(uint256 amount, address receiver, address owner) external returns (uint256);
        function repay(uint256 amount, address receiver) external returns (uint256);
        function disableController() external;
        event Borrow(address indexed account, uint256 assets);
    }

    #[sol(rpc)]
    interface IEVC {
        struct BatchItem {
            address targetContract;
            address onBehalfOfAccount;
            uint256 value;
            bytes data;
        }
        function getCollaterals(address account) external view returns (address[] memory);
        function enableController(address account, address vault) external payable;
        function batch(BatchItem[] calldata items) external payable;
    }

    #[sol(rpc)]
    interface IPriceOracle {
        function getQuote(uint256 inAmount, address base, address quote) external view returns (uint256 outAmount);
    }

    interface IDecimals {
        function decimals() external view returns (uint8);
    }
}

/// The unit of account of USD-denominated vaults (ISO 4217 code 840), 18 decimals.
const USD: Address = address!("0000000000000000000000000000000000000348");
/// Reads per multicall request.
const CHUNK: usize = 200;

#[derive(Debug, Clone)]
struct Vault {
    vault: Address,
    asset: Address,
    unit: Address,
    oracle: Address,
}

pub struct EulerAdapter {
    cfg: EulerConfig,
    rpc: AdapterRpc,
    /// Vaults that accept collateral (possible controllers), by address.
    vaults: HashMap<Address, Vault>,
    /// Every vault's underlying (collateral vaults included), for routes.
    assets: Vec<Address>,
    /// Decimals of non-USD units of account.
    unit_decimals: HashMap<Address, u8>,
    index: EventIndex,
}

impl EulerAdapter {
    pub async fn new(cfg: EulerConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let f = IFactory::new(cfg.vault_factory, rpc.fast.clone());
        let n = f
            .getProxyListLength()
            .call()
            .await
            .context("getProxyListLength")?;
        let all = f
            .getProxyListSlice(U256::ZERO, n)
            .call()
            .await
            .context("getProxyListSlice")?;
        let mut reads = Vec::new();
        for v in &all {
            reads.push(call(*v, IEVault::LTVListCall {}));
            reads.push(call(*v, IEVault::assetCall {}));
            reads.push(call(*v, IEVault::unitOfAccountCall {}));
            reads.push(call(*v, IEVault::oracleCall {}));
        }
        let res = multicall(&rpc.fast, &reads, CHUNK).await?;
        let mut vaults = HashMap::new();
        let mut assets = Vec::new();
        for (v, r) in all.iter().zip(res.chunks(4)) {
            let Some(asset) = decode::<IEVault::assetCall>(&r[1]) else {
                continue;
            };
            assets.push(asset);
            let ltv = decode::<IEVault::LTVListCall>(&r[0]).unwrap_or_default();
            let (Some(unit), Some(oracle)) = (
                decode::<IEVault::unitOfAccountCall>(&r[2]),
                decode::<IEVault::oracleCall>(&r[3]),
            ) else {
                continue;
            };
            if !ltv.is_empty() && !oracle.is_zero() {
                vaults.insert(
                    *v,
                    Vault {
                        vault: *v,
                        asset,
                        unit,
                        oracle,
                    },
                );
            }
        }
        anyhow::ensure!(!vaults.is_empty(), "euler: no vault accepts collateral");
        assets.sort_unstable();
        assets.dedup();
        let mut units: Vec<Address> = vaults
            .values()
            .map(|v| v.unit)
            .filter(|u| *u != USD)
            .collect();
        units.sort_unstable();
        units.dedup();
        let reads: Vec<_> = units
            .iter()
            .map(|u| call(*u, IDecimals::decimalsCall {}))
            .collect();
        let unit_decimals: HashMap<Address, u8> = units
            .iter()
            .zip(multicall(&rpc.fast, &reads, CHUNK).await?)
            .filter_map(|(u, r)| Some((*u, decode::<IDecimals::decimalsCall>(&r)?)))
            .collect();
        let mut watched: Vec<Address> = vaults.keys().copied().collect();
        watched.sort_unstable();
        let index = EventIndex::open(IndexConfig {
            addresses: watched,
            topics: vec![IEVault::Borrow::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        });
        info!(
            protocol = ID,
            vaults = all.len(),
            controllers = vaults.len(),
            borrowers = index.len(),
            "euler adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            vaults,
            assets,
            unit_decimals,
            index,
        })
    }

    /// USD per raw unit of each non-USD unit of account (through `usd_oracle`, 8 decimals).
    async fn unit_prices(&self, p: &DynProvider) -> Result<HashMap<Address, f64>> {
        let units: Vec<Address> = self.unit_decimals.keys().copied().collect();
        let reads: Vec<_> = units
            .iter()
            .map(|u| {
                call(
                    self.cfg.usd_oracle,
                    IAaveOracle::getAssetPriceCall { asset: *u },
                )
            })
            .collect();
        Ok(units
            .iter()
            .zip(multicall(p, &reads, CHUNK).await?)
            .filter_map(|(u, r)| {
                let px = decode::<IAaveOracle::getAssetPriceCall>(&r)?;
                let d = *self.unit_decimals.get(u)?;
                Some((*u, f64::from(px) / 1e8 / 10f64.powi(d as i32)))
            })
            .collect())
    }
}

/// USD of `value` in unit of account `unit`: 18-decimal USD, or a token at `prices[unit]` per raw
/// unit. `None` for a unit without a price.
fn to_usd(value: U256, unit: Address, prices: &HashMap<Address, f64>) -> Option<f64> {
    if unit == USD {
        return Some(f64::from(value) / 1e18);
    }
    prices.get(&unit).map(|p| f64::from(value) * p)
}

fn health_factor(collateral_value: U256, liability_value: U256) -> f64 {
    if liability_value.is_zero() {
        return f64::INFINITY;
    }
    f64::from(collateral_value) / f64::from(liability_value)
}

/// Repay scaled so the yield it buys can be redeemed from `cash` (yield is linear in the repay);
/// 1e-3 under, since `liquidate` is re-sized against the landing block's state.
fn repay_within_cash(repay: U256, yield_assets: U256, cash: U256) -> U256 {
    if yield_assets <= cash || yield_assets.is_zero() {
        return repay;
    }
    let r = repay * cash / yield_assets;
    r - r / U256::from(1_000u64)
}

/// Flash amount: the repay, and a 1e-4 (and one unit) margin; the rest is returned.
fn flash_for(repay: U256) -> U256 {
    repay + repay / U256::from(10_000u64) + U256::from(1u8)
}

#[async_trait]
impl LiquidationAdapter for EulerAdapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        self.index
            .step(&self.rpc.scan, |l| {
                topic_address(l, 1)
                    .map(|a| vec![(market_key(l.address()), a)])
                    .unwrap_or_default()
            })
            .await
    }

    fn accounts(&self) -> Vec<AccountRef> {
        let mut out: Vec<AccountRef> = self
            .index
            .accounts()
            .map(|(m, a)| AccountRef {
                market: Address::from_word(*m),
                account: *a,
                sub_id: 0,
            })
            .collect();
        out.sort_unstable_by_key(|a| (a.market, a.account));
        out.dedup();
        out
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let p = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let prices = self.unit_prices(p).await?;
        // debtOf first: accountLiquidity reverts for an account that owes the vault nothing.
        let reads: Vec<_> = accounts
            .iter()
            .map(|a| call(a.market, IEVault::debtOfCall { account: a.account }))
            .collect();
        let debts = multicall(p, &reads, CHUNK).await?;
        let owing: Vec<usize> = (0..accounts.len())
            .filter(|&i| decode::<IEVault::debtOfCall>(&debts[i]).is_some_and(|d| !d.is_zero()))
            .collect();
        let reads: Vec<_> = owing
            .iter()
            .map(|&i| {
                call(
                    accounts[i].market,
                    IEVault::accountLiquidityCall {
                        account: accounts[i].account,
                        liquidation: true,
                    },
                )
            })
            .collect();
        let liq = multicall(p, &reads, CHUNK).await?;
        let liq: HashMap<usize, IEVault::accountLiquidityReturn> = owing
            .iter()
            .zip(&liq)
            .filter_map(|(&i, r)| Some((i, decode::<IEVault::accountLiquidityCall>(r)?)))
            .collect();
        let mut out = Vec::with_capacity(accounts.len());
        for (i, a) in accounts.iter().enumerate() {
            let Some(debt) = decode::<IEVault::debtOfCall>(&debts[i]) else {
                continue; // read failed
            };
            if debt.is_zero() {
                out.push(Health {
                    account: *a,
                    health_factor: f64::INFINITY,
                    debt_usd: 0.0,
                    collateral_usd: 0.0,
                });
                continue;
            }
            let (Some(l), Some(v)) = (liq.get(&i), self.vaults.get(&a.market)) else {
                continue;
            };
            out.push(Health {
                account: *a,
                health_factor: health_factor(l.collateralValue, l.liabilityValue),
                debt_usd: to_usd(l.liabilityValue, v.unit, &prices).unwrap_or(0.0),
                collateral_usd: to_usd(l.collateralValue, v.unit, &prices).unwrap_or(0.0),
            });
        }
        Ok(out)
    }

    async fn plan(&self, h: &Health, executor: Address) -> Result<Option<Plan>> {
        let p = &self.rpc.fast;
        let who = h.account.account;
        let Some(ctl) = self.vaults.get(&h.account.market) else {
            return Ok(None);
        };
        let evc = IEVC::new(self.cfg.evc, p.clone());
        let collaterals = evc.getCollaterals(who).call().await?;
        let reads: Vec<_> = collaterals
            .iter()
            .map(|c| {
                call(
                    ctl.vault,
                    IEVault::checkLiquidationCall {
                        liquidator: executor,
                        violator: who,
                        collateral: *c,
                    },
                )
            })
            .collect();
        let res = multicall(p, &reads, CHUNK).await?;
        let Some((col, best)) = collaterals
            .iter()
            .zip(&res)
            .filter_map(|(c, r)| Some((*c, decode::<IEVault::checkLiquidationCall>(r)?)))
            .filter(|(_, x)| !x.maxRepay.is_zero() && !x.maxYield.is_zero())
            .max_by_key(|(_, x)| x.maxRepay)
        else {
            return Ok(None);
        };
        let reads = vec![
            call(col, IEVault::assetCall {}),
            call(col, IEVault::cashCall {}),
            call(
                col,
                IEVault::convertToAssetsCall {
                    shares: best.maxYield,
                },
            ),
        ];
        let r = multicall(p, &reads, CHUNK).await?;
        let (Some(col_asset), Some(cash), Some(yield_assets)) = (
            decode::<IEVault::assetCall>(&r[0]),
            decode::<IEVault::cashCall>(&r[1]),
            decode::<IEVault::convertToAssetsCall>(&r[2]),
        ) else {
            return Ok(None);
        };
        let repay = repay_within_cash(best.maxRepay, yield_assets, cash);
        if repay.is_zero() {
            return Ok(None);
        }
        let (yield_shares, yield_assets) = if repay == best.maxRepay {
            (best.maxYield, yield_assets)
        } else {
            (
                best.maxYield * repay / best.maxRepay,
                yield_assets * repay / best.maxRepay,
            )
        };
        // Bonus in the controller's unit of account, through its own oracle router.
        let quotes = vec![
            call(
                ctl.oracle,
                IPriceOracle::getQuoteCall {
                    inAmount: yield_shares,
                    base: col,
                    quote: ctl.unit,
                },
            ),
            call(
                ctl.oracle,
                IPriceOracle::getQuoteCall {
                    inAmount: repay,
                    base: ctl.asset,
                    quote: ctl.unit,
                },
            ),
        ];
        let q = multicall(p, &quotes, CHUNK).await?;
        let prices = self.unit_prices(p).await?;
        let value = |r: &Option<Bytes>| {
            decode::<IPriceOracle::getQuoteCall>(r).and_then(|v| to_usd(v, ctl.unit, &prices))
        };
        let bonus_usd = match (value(&q[0]), value(&q[1])) {
            (Some(y), Some(d)) => y - d,
            _ => 0.0,
        };
        let flash = flash_for(repay);
        let item = |target: Address, data: Vec<u8>| IEVC::BatchItem {
            targetContract: target,
            onBehalfOfAccount: executor,
            value: U256::ZERO,
            data: data.into(),
        };
        let items = vec![
            item(
                ctl.vault,
                IEVault::liquidateCall {
                    violator: who,
                    collateral: col,
                    repayAssets: repay,
                    minYieldBalance: U256::ZERO,
                }
                .abi_encode(),
            ),
            item(
                col,
                IEVault::redeemCall {
                    amount: U256::MAX,
                    receiver: executor,
                    owner: executor,
                }
                .abi_encode(),
            ),
            item(
                ctl.vault,
                IEVault::repayCall {
                    amount: U256::MAX,
                    receiver: executor,
                }
                .abi_encode(),
            ),
            item(ctl.vault, IEVault::disableControllerCall {}.abi_encode()),
        ];
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: ctl.asset,
            collateral_asset: col_asset,
            flash_amount: flash,
            dependencies: vec![],
            expected_collateral: f64::from(yield_assets),
            bonus_usd,
            calls: vec![
                ProtocolCall {
                    target: self.cfg.evc,
                    data: IEVC::enableControllerCall {
                        account: executor,
                        vault: ctl.vault,
                    }
                    .abi_encode()
                    .into(),
                    approve_spender: Address::ZERO,
                    approve_amount: U256::ZERO,
                },
                ProtocolCall {
                    target: self.cfg.evc,
                    data: IEVC::batchCall { items }.abi_encode().into(),
                    // `repay` pulls the debt asset from the executor.
                    approve_spender: ctl.vault,
                    approve_amount: flash,
                },
            ],
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        Vec::new()
    }

    fn targets(&self) -> Vec<Address> {
        std::iter::once(self.cfg.evc)
            .chain(self.vaults.keys().copied())
            .collect()
    }

    fn assets(&self) -> Vec<Address> {
        self.assets.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_units_are_18_decimals_and_tokens_use_their_price() {
        let one = U256::from(10u64).pow(U256::from(18u8));
        assert_eq!(
            to_usd(one * U256::from(5u8), USD, &HashMap::new()),
            Some(5.0)
        );
        let usdc = Address::with_last_byte(1);
        let px = HashMap::from([(usdc, 1e-6)]);
        assert_eq!(to_usd(U256::from(2_000_000u64), usdc, &px), Some(2.0));
        assert_eq!(
            to_usd(U256::from(1u8), Address::with_last_byte(2), &px),
            None
        );
    }

    #[test]
    fn health_is_collateral_over_liability() {
        assert!(health_factor(U256::from(99u8), U256::from(100u8)) < 1.0);
        assert!(health_factor(U256::from(1u8), U256::ZERO).is_infinite());
    }

    #[test]
    fn the_repay_shrinks_to_what_the_collateral_vault_can_pay_out() {
        let r = U256::from(1_000_000u64);
        assert_eq!(
            repay_within_cash(r, U256::from(500u64), U256::from(600u64)),
            r
        );
        // Half the yield is redeemable: half the repay, 0.1% under.
        assert_eq!(
            repay_within_cash(r, U256::from(1_000u64), U256::from(500u64)),
            U256::from(499_500u64)
        );
    }

    #[test]
    fn the_flash_loan_covers_the_repay_with_a_margin() {
        assert_eq!(
            flash_for(U256::from(1_000_000u64)),
            U256::from(1_000_101u64)
        );
    }
}
