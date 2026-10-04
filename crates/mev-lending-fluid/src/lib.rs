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
/// Arbitrum WETH (native collateral arrives as ETH and is wrapped by the executor).
pub const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
/// Accept up to 1% less collateral per unit debt than quoted (state moves between quote and tx).
const SLIPPAGE: f64 = 0.99;

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
    interface IFluidVaultResolver {
        function getVaultLiquidation(address vault, uint256 tokenInAmt) external returns (LiquidationStruct memory);
    }

    interface IFluidVaultT1 {
        function liquidate(uint256 debtAmt, uint256 colPerUnitDebt, address to, bool absorb) external payable returns (uint256, uint256);
    }
}

/// ERC20 used for routing/accounting for a Fluid token (native ETH -> WETH).
pub fn erc20(token: Address) -> Address {
    if token == NATIVE { WETH } else { token }
}

/// Minimum collateral per unit of debt, 1e18-scaled, with the slippage guard applied.
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
}

pub struct FluidAdapter {
    cfg: FluidConfig,
    rpc: AdapterRpc,
    decimals: HashMap<Address, u8>,
    tokens: Vec<Address>,
    feeds: Vec<Address>,
}

impl FluidAdapter {
    pub async fn new(cfg: FluidConfig, rpc: AdapterRpc) -> Result<Self> {
        let mut me = Self {
            cfg,
            rpc,
            decimals: HashMap::new(),
            tokens: vec![],
            feeds: vec![],
        };
        let quotes = me.quotes(true).await.context("Fluid resolver")?;
        let mut tokens: Vec<Address> = quotes
            .values()
            .flat_map(|q| [erc20(q.debt_token), erc20(q.col_token)])
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
        let t = erc20(token);
        let dec = self.decimals.get(&t).copied().unwrap_or(18);
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
        if q.debt_in <= 0.0 || q.debt_token == NATIVE {
            return Ok(None);
        }
        let prices = self.usd_prices().await?;
        let debt_amt = U256::from(q.debt_in as u128);
        let data = IFluidVaultT1::liquidateCall {
            debtAmt: debt_amt,
            colPerUnitDebt: U256::from(col_per_unit_debt(q.debt_in, q.col_out) as u128),
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
            collateral_asset: erc20(q.col_token),
            flash_amount: debt_amt,
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
        assert_eq!(erc20(NATIVE), WETH);
        let usdc = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
        assert_eq!(erc20(usdc), usdc);
    }

    #[test]
    fn col_per_unit_debt_applies_slippage_guard() {
        // 2,000 debt units for 1 collateral unit (1e18-scaled): 1e18/2000 * 0.99.
        let v = col_per_unit_debt(2_000.0, 1.0);
        assert!((v - 1e18 / 2_000.0 * 0.99).abs() < 1.0);
        assert_eq!(col_per_unit_debt(0.0, 1.0), 0.0);
    }
}
