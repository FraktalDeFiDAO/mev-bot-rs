//! Aave v3 liquidation adapter.
//!
//! - Accounts: borrowers from `Borrow` events (newest-first back-fill + forward), cached on disk.
//! - Health: `getUserAccountData` (HF, debt, collateral in USD).
//! - Plan: best collateral/debt pair under Aave v3 close-factor rules ([`math`]), executed as one
//!   `liquidationCall(collateral, debt, user, type(uint256).max, false)` with an exact approval.
//! - Price feeds: Aave oracle sources resolved to Chainlink aggregators (sequencer triggers).

pub mod math;

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    time::Duration,
};

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider},
    rpc::types::Filter,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use math::{Position, ReserveInfo};
use mev_bindings::{IAaveDataProvider, IAaveOracle, IAavePool, IERC20};
use mev_config::AaveConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_rpc::{call, decode, multicall};
use serde::{Deserialize, Serialize};
use tracing::info;

alloy::sol! {
    function liquidationCall(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken);
}

pub const ID: ProtocolId = "aave-v3";
/// getUserAccountData walks every reserve + oracle: keep batches small.
const ACCOUNT_DATA_CHUNK: usize = 50;
const MAX_SCAN_CHUNKS_PER_CALL: usize = 20;
const SCAN_THROTTLE: Duration = Duration::from_millis(500);
/// Flash this much more than the user's debt: Aave rounds variable debt up; excess is returned.
const FLASH_BUFFER: f64 = 1.002;

/// Borrowers seen in Borrow events, scanned newest-first so recent borrowers are found first.
#[derive(Default, Serialize, Deserialize)]
struct BorrowerIndex {
    next_block: u64,
    back_block: u64,
    borrowers: BTreeSet<Address>,
}

pub struct AaveV3Adapter {
    cfg: AaveConfig,
    rpc: AdapterRpc,
    cache_path: PathBuf,
    index: BorrowerIndex,
    reserves: Vec<Address>,
    decimals: HashMap<Address, u8>,
    feeds: Vec<Address>,
}

impl AaveV3Adapter {
    pub async fn new(cfg: AaveConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        let index = std::fs::read(&cache_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<BorrowerIndex>(&b).ok())
            .unwrap_or_default();
        let reserves = IAavePool::new(cfg.pool, &rpc.fast)
            .getReservesList()
            .call()
            .await
            .context("Aave reserves")?;
        let dec_calls: Vec<_> = reserves
            .iter()
            .map(|a| call(*a, IERC20::decimalsCall {}))
            .collect();
        let dec_res = multicall(&rpc.fast, &dec_calls, 200).await?;
        let decimals = reserves
            .iter()
            .zip(dec_res.iter())
            .filter_map(|(a, r)| decode::<IERC20::decimalsCall>(r).map(|d| (*a, d)))
            .collect();
        let src_calls: Vec<_> = reserves
            .iter()
            .map(|a| call(cfg.oracle, IAaveOracle::getSourceOfAssetCall { asset: *a }))
            .collect();
        let sources: Vec<Address> = multicall(&rpc.fast, &src_calls, 200)
            .await?
            .iter()
            .filter_map(decode::<IAaveOracle::getSourceOfAssetCall>)
            .filter(|a| !a.is_zero())
            .collect();
        let feeds: Vec<Address> = mev_oracle::resolve_aggregators(&rpc.fast, &sources)
            .await?
            .into_keys()
            .collect();
        info!(
            protocol = ID,
            reserves = reserves.len(),
            price_feeds = feeds.len(),
            borrowers_cached = index.borrowers.len(),
            "adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            cache_path,
            index,
            reserves,
            decimals,
            feeds,
        })
    }

    fn account(&self, user: Address) -> AccountRef {
        AccountRef {
            market: self.cfg.pool,
            account: user,
            sub_id: 0,
        }
    }

    async fn get_borrows(&mut self, from: u64, to: u64) -> Result<()> {
        tokio::time::sleep(SCAN_THROTTLE).await;
        let filter = Filter::new()
            .address(self.cfg.pool)
            .event_signature(IAavePool::Borrow::SIGNATURE_HASH)
            .from_block(from)
            .to_block(to);
        for l in self
            .rpc
            .scan
            .get_logs(&filter)
            .await
            .context("Borrow log scan")?
        {
            if let Some(t) = l.topics().get(2) {
                self.index.borrowers.insert(Address::from_word(*t));
            }
        }
        Ok(())
    }

    /// Scan [from, to], shrinking on non-rate-limit errors. Returns the range actually scanned.
    async fn scan(&mut self, mut from: u64, mut to: u64, backward: bool) -> Result<(u64, u64)> {
        loop {
            match self.get_borrows(from, to).await {
                Ok(()) => return Ok((from, to)),
                Err(e) if to > from + 1_000 && !mev_rpc::is_rate_limit(&e) => {
                    if backward {
                        from = to - (to - from) / 4;
                    } else {
                        to = from + (to - from) / 4;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn reserves_info(&self) -> Result<Vec<ReserveInfo>> {
        let mut calls = Vec::new();
        for a in &self.reserves {
            calls.push(call(
                self.cfg.data_provider,
                IAaveDataProvider::getReserveConfigurationDataCall { asset: *a },
            ));
            calls.push(call(
                self.cfg.data_provider,
                IAaveDataProvider::getLiquidationProtocolFeeCall { asset: *a },
            ));
            calls.push(call(
                self.cfg.oracle,
                IAaveOracle::getAssetPriceCall { asset: *a },
            ));
        }
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        Ok(self
            .reserves
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let c = decode::<IAaveDataProvider::getReserveConfigurationDataCall>(&res[3 * i])?;
                let fee =
                    decode::<IAaveDataProvider::getLiquidationProtocolFeeCall>(&res[3 * i + 1])?;
                let px = decode::<IAaveOracle::getAssetPriceCall>(&res[3 * i + 2])?;
                Some(ReserveInfo {
                    asset: *a,
                    decimals: self
                        .decimals
                        .get(a)
                        .copied()
                        .unwrap_or(u8::try_from(c.decimals).unwrap_or(18)),
                    liquidation_bonus_bps: u32::try_from(c.liquidationBonus).unwrap_or(0),
                    protocol_fee_bps: u32::try_from(fee).unwrap_or(0),
                    price_usd: f64::from(px) / 1e8,
                })
            })
            .collect())
    }

    async fn position(&self, user: Address, health_factor: f64) -> Result<Position> {
        let calls: Vec<_> = self
            .reserves
            .iter()
            .map(|a| {
                call(
                    self.cfg.data_provider,
                    IAaveDataProvider::getUserReserveDataCall { asset: *a, user },
                )
            })
            .collect();
        let res = multicall(&self.rpc.fast, &calls, 200).await?;
        let mut pos = Position {
            user,
            health_factor,
            ..Default::default()
        };
        for (a, r) in self.reserves.iter().zip(res.iter()) {
            let Some(d) = decode::<IAaveDataProvider::getUserReserveDataCall>(r) else {
                continue;
            };
            let coll = f64::from(d.currentATokenBalance);
            let debt = f64::from(d.currentVariableDebt) + f64::from(d.currentStableDebt);
            if coll > 0.0 && d.usageAsCollateralEnabled {
                pos.collateral.push((*a, coll));
            }
            if debt > 0.0 {
                pos.debt.push((*a, debt));
            }
        }
        Ok(pos)
    }
}

#[async_trait]
impl LiquidationAdapter for AaveV3Adapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::Live
    }

    async fn index(&mut self) -> Result<bool> {
        let head = self.rpc.scan.get_block_number().await?;
        if self.index.next_block == 0 {
            self.index.next_block = head + 1;
            self.index.back_block = head + 1;
        }
        let step = self.cfg.scan_step.max(1);
        let mut chunks = 0;
        while self.index.next_block <= head && chunks < MAX_SCAN_CHUNKS_PER_CALL {
            let from = self.index.next_block;
            let (_, to) = self.scan(from, (from + step - 1).min(head), false).await?;
            self.index.next_block = to + 1;
            chunks += 1;
        }
        while self.index.back_block > self.cfg.from_block && chunks < MAX_SCAN_CHUNKS_PER_CALL {
            let to = self.index.back_block - 1;
            let (from, _) = self
                .scan(
                    to.saturating_sub(step - 1).max(self.cfg.from_block),
                    to,
                    true,
                )
                .await?;
            self.index.back_block = from;
            chunks += 1;
        }
        if let Some(dir) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.cache_path, serde_json::to_vec(&self.index)?)?;
        let done = self.index.back_block <= self.cfg.from_block;
        info!(
            protocol = ID,
            borrowers = self.index.borrowers.len(),
            history_left_blocks = self.index.back_block.saturating_sub(self.cfg.from_block),
            done,
            "index step"
        );
        Ok(done)
    }

    fn accounts(&self) -> Vec<AccountRef> {
        self.index
            .borrowers
            .iter()
            .map(|u| self.account(*u))
            .collect()
    }

    async fn health(&self, accounts: &[AccountRef], fast: bool) -> Result<Vec<Health>> {
        let p: &DynProvider = if fast {
            &self.rpc.fast
        } else {
            &self.rpc.sweep
        };
        let calls: Vec<_> = accounts
            .iter()
            .map(|a| {
                call(
                    self.cfg.pool,
                    IAavePool::getUserAccountDataCall { user: a.account },
                )
            })
            .collect();
        let res = multicall(p, &calls, ACCOUNT_DATA_CHUNK).await?;
        Ok(accounts
            .iter()
            .zip(res.iter())
            .filter_map(|(a, r)| {
                decode::<IAavePool::getUserAccountDataCall>(r).map(|d| Health {
                    account: *a,
                    debt_usd: f64::from(d.totalDebtBase) / 1e8,
                    collateral_usd: f64::from(d.totalCollateralBase) / 1e8,
                    health_factor: f64::from(d.healthFactor.min(U256::from(u128::MAX))) / 1e18,
                })
            })
            .collect())
    }

    async fn plan(&self, h: &Health, _executor: Address) -> Result<Option<Plan>> {
        let reserves = self.reserves_info().await?;
        let pos = self.position(h.account.account, h.health_factor).await?;
        let Some(p) = math::plan(&pos, &reserves) else {
            return Ok(None);
        };
        let debt_raw = pos
            .debt
            .iter()
            .find(|(a, _)| *a == p.debt)
            .map(|(_, d)| *d)
            .unwrap_or(0.0);
        let flash = U256::from((debt_raw * FLASH_BUFFER) as u128 + 1);
        let data = liquidationCallCall {
            collateralAsset: p.collateral,
            debtAsset: p.debt,
            user: h.account.account,
            debtToCover: U256::MAX,
            receiveAToken: false,
        }
        .abi_encode();
        Ok(Some(Plan {
            protocol: ID,
            account: h.account,
            debt_asset: p.debt,
            collateral_asset: p.collateral,
            flash_amount: flash,
            expected_collateral: p.collateral_out,
            bonus_usd: p.bonus_usd,
            calls: vec![ProtocolCall {
                target: self.cfg.pool,
                data: data.into(),
                approve_spender: self.cfg.pool,
                approve_amount: flash,
            }],
        }))
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feeds.clone()
    }

    fn targets(&self) -> Vec<Address> {
        vec![self.cfg.pool]
    }

    fn assets(&self) -> Vec<Address> {
        self.reserves.clone()
    }
}
