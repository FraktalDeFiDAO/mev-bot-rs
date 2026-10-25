//! Fluid vaults liquidation adapter.
//!
//! Fluid liquidates by price range (ticks), not per account: anyone can call
//! `vault.liquidate(debtAmt, colPerUnitDebt, to, absorb)` and buy discounted collateral for debt.
//! The official `VaultResolver.getVaultLiquidation(vault, 0)` reports how much debt can be paid
//! and how much collateral comes out right now, so each watched vault is one pseudo-account
//! (`AccountRef { market: vault, account: vault }`) — no event index needed.
//!
//! Supported: T1 vaults with ERC20 debt and ERC20 or native-ETH collateral (the executor wraps ETH
//! to WETH). Native-ETH debt needs `msg.value` and is skipped.

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::SolCall,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use mev_bindings::{IAaveOracle, IERC20};
use mev_config::FluidConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_rpc::{call, decode, multicall};
use std::collections::HashMap;
use tracing::info;

pub const ID: ProtocolId = "fluid";
/// Fluid's marker for the chain's native token.
pub const NATIVE: Address = address!("EeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");
/// Accept up to 1% less collateral per unit debt than quoted (state moves between quote and tx).
const SLIPPAGE: f64 = 0.99;
const SLIPPAGE_NUM: u64 = 99;
const SLIPPAGE_DEN: u64 = 100;

sol! {
    struct LiquidationStruct {
        address vault;
        address tokenIn;
        address tokenOut;
        uint256 tokenInAmtOne;
        uint256 tokenOutAmtOne;
        uint256 tokenInAmtTwo;
        uint256 tokenOutAmtTwo;
        bool absorbAvailable;
    }

    #[sol(rpc)]
    interface IFluidDexFactory {
        function totalDexes() external view returns (uint256);
        function getDexAddress(uint256 dexId) external view returns (address);
    }

    #[sol(rpc)]
    interface IFluidVaultResolver {
        function getVaultLiquidation(address vault, uint256 tokenInAmt) external returns (LiquidationStruct memory);
        function getAllVaultsAddresses() external view returns (address[] memory);
        function getVaultType(address vault) external view returns (uint256);
    }

    interface IFluidVaultT1 {
        function liquidate(uint256 debtAmt, uint256 colPerUnitDebt, address to, bool absorb) external payable returns (uint256, uint256);
    }
}

/// Every vault the resolver knows (all types), so their `LogLiquidate` / `LogAbsorb` events can be recorded even
/// where no liquidation is attempted.
pub async fn all_vaults(
    provider: &alloy::providers::DynProvider,
    resolver: Address,
) -> Result<Vec<Address>> {
    Ok(IFluidVaultResolver::new(resolver, provider)
        .getAllVaultsAddresses()
        .call()
        .await?)
}

/// Every Fluid DEX pool the factory lists (ids are 1-based and dense).
pub async fn all_dexes(
    provider: &alloy::providers::DynProvider,
    factory: Address,
) -> Result<Vec<Address>> {
    let f = IFluidDexFactory::new(factory, provider);
    let n = u64::try_from(f.totalDexes().call().await?)
        .unwrap_or(0)
        .min(1_000);
    let calls: Vec<_> = (1..=n)
        .map(|i| {
            call(
                factory,
                IFluidDexFactory::getDexAddressCall {
                    dexId: U256::from(i),
                },
            )
        })
        .collect();
    let out = multicall(provider, &calls, 200).await?;
    Ok(out
        .iter()
        .filter_map(decode::<IFluidDexFactory::getDexAddressCall>)
        .filter(|a| !a.is_zero())
        .collect())
}

/// Fluid vault type for a plain collateral / plain debt vault (smart-collateral and smart-debt vaults are 2-4).
const VAULT_TYPE_T1: u64 = 10_000;

/// Every T1 vault the resolver lists (the only type `liquidate(debtAmt, colPerUnitDebt, to, absorb)` fits).
pub async fn t1_vaults(
    provider: &alloy::providers::DynProvider,
    resolver: Address,
) -> Result<Vec<Address>> {
    let all = all_vaults(provider, resolver).await?;
    let calls: Vec<_> = all
        .iter()
        .map(|v| {
            call(
                resolver,
                IFluidVaultResolver::getVaultTypeCall { vault: *v },
            )
        })
        .collect();
    let types = multicall(provider, &calls, 200).await?;
    Ok(all
        .into_iter()
        .zip(types.iter())
        .filter(|(_, t)| {
            decode::<IFluidVaultResolver::getVaultTypeCall>(t)
                .is_some_and(|t| t == U256::from(VAULT_TYPE_T1))
        })
        .map(|(v, _)| v)
        .collect())
}

/// ERC20 used for routing/accounting for a Fluid token: native ETH arrives as ETH and the executor
/// wraps it, so it is the chain's `weth`.
pub fn erc20(token: Address, weth: Address) -> Address {
    if token == NATIVE { weth } else { token }
}

/// Minimum collateral per unit of debt, 1e18-scaled, with the slippage guard applied.
/// Exact `colPerUnitDebt` (1e18-scaled collateral per unit of debt) with the 1% slippage guard, rounded down.
pub fn col_per_unit_debt_exact(debt_in: U256, col_out: U256) -> Option<U256> {
    if debt_in.is_zero() {
        return None;
    }
    col_out
        .checked_mul(U256::from(10u8).pow(U256::from(18u8)))?
        .checked_mul(U256::from(SLIPPAGE_NUM))?
        .checked_div(debt_in.checked_mul(U256::from(SLIPPAGE_DEN))?)
}

pub fn col_per_unit_debt(debt_in: f64, col_out: f64) -> f64 {
    if debt_in <= 0.0 {
        0.0
    } else {
        col_out * 1e18 / debt_in * SLIPPAGE
    }
}

#[derive(Debug, Clone, Copy)]
struct Quote {
    debt_token: Address,
    col_token: Address,
    debt_in: f64,
    col_out: f64,
    /// The resolver's amounts as exact integers; calldata is built from these.
    debt_in_raw: U256,
    col_out_raw: U256,
}

pub struct FluidAdapter {
    cfg: FluidConfig,
    rpc: AdapterRpc,
    decimals: HashMap<Address, u8>,
    tokens: Vec<Address>,
    feeds: Vec<Address>,
    /// The chain's wrapped native token (what native collateral is wrapped into).
    weth: Address,
}

impl FluidAdapter {
    pub async fn new(mut cfg: FluidConfig, rpc: AdapterRpc, weth: Address) -> Result<Self> {
        if cfg.auto_vaults {
            match t1_vaults(&rpc.fast, cfg.vault_resolver).await {
                Ok(found) => {
                    for v in found {
                        if !cfg.vaults.contains(&v) {
                            cfg.vaults.push(v);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "fluid: could not list vaults; using the configured ones")
                }
            }
        }
        let mut me = Self {
            cfg,
            rpc,
            decimals: HashMap::new(),
            tokens: vec![],
            feeds: vec![],
            weth,
        };
        let quotes = me.quotes(true).await.context("Fluid resolver")?;
        let mut tokens: Vec<Address> = quotes
            .values()
            .flat_map(|q| [erc20(q.debt_token, weth), erc20(q.col_token, weth)])
            .collect();
        tokens.sort();
        tokens.dedup();
        let dec = multicall(
            &me.rpc.fast,
            &tokens
                .iter()
                .map(|t| call(*t, IERC20::decimalsCall {}))
                .collect::<Vec<_>>(),
            200,
        )
        .await?;
        me.decimals = tokens
            .iter()
            .zip(dec.iter())
            .filter_map(|(t, r)| decode::<IERC20::decimalsCall>(r).map(|d| (*t, d)))
            .collect();
        if me.decimals.len() < tokens.len() {
            tracing::warn!(
                known = me.decimals.len(),
                tokens = tokens.len(),
                "fluid: some token decimals unreadable; those vaults are valued at 0 until a restart"
            );
        }
        // Trigger on the same Chainlink aggregators the vault oracles read, found via the USD oracle's sources.
        let src_calls: Vec<_> = tokens
            .iter()
            .map(|t| {
                call(
                    me.cfg.usd_oracle,
                    IAaveOracle::getSourceOfAssetCall { asset: *t },
                )
            })
            .collect();
        let sources: Vec<Address> = multicall(&me.rpc.fast, &src_calls, 200)
            .await?
            .iter()
            .filter_map(decode::<IAaveOracle::getSourceOfAssetCall>)
            .filter(|a| !a.is_zero())
            .collect();
        me.feeds = mev_oracle::resolve_aggregators(&me.rpc.fast, &sources)
            .await?
            .into_keys()
            .collect();
        me.tokens = tokens;
        info!(
            protocol = ID,
            vaults = me.cfg.vaults.len(),
            price_feeds = me.feeds.len(),
            "adapter ready"
        );
        Ok(me)
    }

    async fn quotes(&self, fast: bool) -> Result<HashMap<Address, Quote>> {
        let p = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let calls: Vec<_> = self
            .cfg
            .vaults
            .iter()
            .map(|v| {
                call(
                    self.cfg.vault_resolver,
                    IFluidVaultResolver::getVaultLiquidationCall {
                        vault: *v,
                        tokenInAmt: U256::ZERO,
                    },
                )
            })
            .collect();
        let res = multicall(p, &calls, 50).await?;
        Ok(self
            .cfg
            .vaults
            .iter()
            .zip(res.iter())
            .filter_map(|(v, r)| {
                let l = decode::<IFluidVaultResolver::getVaultLiquidationCall>(r)?;
                Some((
                    *v,
                    Quote {
                        debt_token: l.tokenIn,
                        col_token: l.tokenOut,
                        debt_in: f64::from(l.tokenInAmtOne),
                        col_out: f64::from(l.tokenOutAmtOne),
                        debt_in_raw: l.tokenInAmtOne,
                        col_out_raw: l.tokenOutAmtOne,
                    },
                ))
            })
            .collect())
    }

    async fn usd_prices(&self) -> Result<HashMap<Address, f64>> {
        let calls: Vec<_> = self
            .tokens
            .iter()
            .map(|t| {
                call(
                    self.cfg.usd_oracle,
                    IAaveOracle::getAssetPriceCall { asset: *t },
                )
            })
            .collect();
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        Ok(self
            .tokens
            .iter()
            .zip(res.iter())
            .filter_map(|(t, r)| {
                decode::<IAaveOracle::getAssetPriceCall>(r).map(|p| (*t, f64::from(p) / 1e8))
            })
            .collect())
    }

    fn usd(&self, token: Address, raw: f64, prices: &HashMap<Address, f64>) -> f64 {
        let t = erc20(token, self.weth);
        // Unknown decimals: value it at 0 (unpriced, so it is skipped), never assume 18.
        let Some(dec) = self.decimals.get(&t).copied() else {
            return 0.0;
        };
        raw / 10f64.powi(dec as i32) * prices.get(&t).copied().unwrap_or(0.0)
    }
}

#[async_trait]
impl LiquidationAdapter for FluidAdapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        Ok(true) // vaults come from config; the resolver needs no account index
    }

    fn accounts(&self) -> Vec<AccountRef> {
        self.cfg
            .vaults
            .iter()
            .map(|v| AccountRef {
                market: *v,
                account: *v,
                sub_id: 0,
            })
            .collect()
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let quotes = self.quotes(fast).await?;
        let prices = self.usd_prices().await?;
        Ok(accounts
            .iter()
            .filter_map(|a| {
                let q = quotes.get(&a.market)?;
                let debt_usd = self.usd(q.debt_token, q.debt_in, &prices);
                let collateral_usd = self.usd(q.col_token, q.col_out, &prices);
                // Liquidation available now <=> resolver quotes a non-zero debt amount.
                let health_factor = if q.debt_in > 0.0 && q.debt_token != NATIVE {
                    0.5
                } else {
                    2.0
                };
                Some(Health {
                    account: *a,
                    health_factor,
                    debt_usd,
                    collateral_usd,
                })
            })
            .collect())
    }

    async fn plan(&self, h: &Health, executor: Address) -> Result<Option<Plan>> {
        let quotes = self.quotes(true).await?;
        let Some(q) = quotes.get(&h.account.market).copied() else {
            return Ok(None);
        };
        if q.debt_in_raw.is_zero() || q.debt_token == NATIVE {
            return Ok(None);
        }
        let Some(col_per_unit) = col_per_unit_debt_exact(q.debt_in_raw, q.col_out_raw) else {
            return Ok(None);
        };
        let prices = self.usd_prices().await?;
        let debt_amt = q.debt_in_raw;
        let data = IFluidVaultT1::liquidateCall {
            debtAmt: debt_amt,
            colPerUnitDebt: col_per_unit,
            to: executor,
            absorb: false,
        }
        .abi_encode();
        let bonus_usd =
            self.usd(q.col_token, q.col_out, &prices) - self.usd(q.debt_token, q.debt_in, &prices);
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: q.debt_token,
            collateral_asset: erc20(q.col_token, self.weth),
            flash_amount: debt_amt,
            dependencies: vec![],
            expected_collateral: q.col_out,
            bonus_usd,
            calls: vec![ProtocolCall {
                target: h.account.market,
                data: data.into(),
                approve_spender: h.account.market,
                approve_amount: debt_amt,
            }],
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.clone()
    }

    fn targets(&self) -> Vec<Address> {
        self.cfg.vaults.clone()
    }

    fn assets(&self) -> Vec<Address> {
        self.tokens.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_eth_routes_as_weth() {
        let weth = address!("4200000000000000000000000000000000000006");
        assert_eq!(erc20(NATIVE, weth), weth);
        let usdc = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
        assert_eq!(erc20(usdc, weth), usdc);
    }

    #[test]
    fn col_per_unit_debt_applies_slippage_guard() {
        // 2,000 debt units for 1 collateral unit (1e18-scaled): 1e18/2000 * 0.99.
        let v = col_per_unit_debt(2_000.0, 1.0);
        assert!((v - 1e18 / 2_000.0 * 0.99).abs() < 1.0);
        assert_eq!(col_per_unit_debt(0.0, 1.0), 0.0);
    }

    #[test]
    fn exact_col_per_unit_debt_matches_the_float_form_and_keeps_every_unit() {
        let v = col_per_unit_debt_exact(
            U256::from(2_000u64),
            U256::from(10u64).pow(U256::from(18u8)),
        )
        .unwrap();
        assert_eq!(
            v,
            U256::from(10u64).pow(U256::from(36u8)) * U256::from(99u8) / U256::from(200_000u64)
        );
        // Above f64 precision: 2^70 + 1 debt units.
        let debt = (U256::from(1u8) << 70) + U256::from(1u8);
        assert!(col_per_unit_debt_exact(debt, debt).unwrap() > U256::from(0u8));
        assert_eq!(col_per_unit_debt_exact(U256::ZERO, U256::from(1u8)), None);
    }
}
