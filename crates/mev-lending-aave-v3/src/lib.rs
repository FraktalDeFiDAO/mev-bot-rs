//! Aave v3 liquidation adapter.
//!
//! - Accounts: borrowers from `Borrow` events (newest-first back-fill + forward), cached on disk.
//! - Health: `getUserAccountData` (HF, debt, collateral in USD).
//! - Plan: best collateral/debt pair under Aave v3 close-factor rules ([`math`]), executed as one
//!   `liquidationCall(collateral, debt, user, type(uint256).max, false)` with an exact approval.
//! - Price feeds: Aave oracle sources resolved to Chainlink aggregators (sequencer triggers).

pub mod math;

use std::{collections::HashMap, path::PathBuf, sync::Mutex, time::Duration};

use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use math::{Cached, LiquidationPlan, Position, ReserveInfo};
use mev_bindings::{IAaveDataProvider, IAaveOracle, IAavePool, IERC20};
use mev_config::AaveConfig;
use mev_lending::{
    AccountRef, AdapterRpc, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolCall,
    ProtocolId,
};
use mev_lending_index::{EventIndex, IndexConfig, market_key, topic_address};
use mev_rpc::{call, decode, multicall};
use tracing::{info, warn};

alloy::sol! {
    #[sol(rpc)]
    interface IAggregator {
        function latestAnswer() external view returns (int256);
    }
    function liquidationCall(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken);
}

pub const ID: ProtocolId = "aave-v3";
/// getUserAccountData walks every reserve + oracle: keep batches small.
const ACCOUNT_DATA_CHUNK: usize = 50;
/// Flash this much more than the user's debt: Aave rounds variable debt up; excess is returned.
const FLASH_BUFFER: f64 = 1.002;
/// Sweeps fetch reserve bitmaps for accounts below this health factor (the runner's at-risk set).
const EXPOSURE_HF: f64 = 1.10;
/// Reserve ids can exceed the live reserve count after reserves are dropped.
const MAX_RESERVE_ID_SLACK: usize = 16;

/// `UserConfigurationMap` bits (borrowing + collateral) for reserve `id`.
pub fn reserve_bits(id: u16) -> U256 {
    U256::from(3) << (2 * id as usize)
}

pub struct AaveV3Adapter {
    cfg: AaveConfig,
    rpc: AdapterRpc,
    index: EventIndex,
    reserves: Vec<Address>,
    decimals: HashMap<Address, u8>,
    /// Aggregator -> `UserConfigurationMap` bits of the reserves it prices.
    feed_masks: HashMap<Address, U256>,
    /// User -> `UserConfigurationMap`, refreshed by sweeps for at-risk users.
    user_masks: Mutex<HashMap<Address, U256>>,
    /// Aggregator -> reserves it prices (for repricing on a predicted update).
    feed_assets: HashMap<Address, Vec<Address>>,
    /// State for backrun prediction, refreshed by sweeps and moved by each predicted update.
    snapshot: Mutex<Snapshot>,
}

#[derive(Default)]
struct Snapshot {
    reserves: Vec<ReserveInfo>,
    cached: Vec<Cached>,
    /// Aggregator -> last known answer (on-chain at refresh, then from the feed).
    answers: HashMap<Address, f64>,
}

impl AaveV3Adapter {
    pub async fn new(cfg: AaveConfig, rpc: AdapterRpc, cache_path: PathBuf) -> Result<Self> {
        migrate_v1_cache(&cache_path, cfg.pool);
        let index = EventIndex::open(IndexConfig {
            addresses: vec![cfg.pool],
            topics: vec![IAavePool::Borrow::SIGNATURE_HASH],
            from_block: cfg.from_block,
            step: cfg.scan_step,
            max_requests: 20,
            throttle: Duration::from_millis(500),
            cache_path,
            name: ID,
        });
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
        let asset_sources: Vec<(Address, Address)> = multicall(&rpc.fast, &src_calls, 200)
            .await?
            .iter()
            .zip(&reserves)
            .filter_map(|(r, a)| decode::<IAaveOracle::getSourceOfAssetCall>(r).map(|s| (*a, s)))
            .filter(|(_, s)| !s.is_zero())
            .collect();
        let id_calls: Vec<_> = (0..reserves.len() + MAX_RESERVE_ID_SLACK)
            .map(|id| {
                call(
                    cfg.pool,
                    IAavePool::getReserveAddressByIdCall { id: id as u16 },
                )
            })
            .collect();
        let ids: HashMap<Address, u16> = multicall(&rpc.fast, &id_calls, 200)
            .await?
            .iter()
            .enumerate()
            .filter_map(|(id, r)| {
                decode::<IAavePool::getReserveAddressByIdCall>(r).map(|a| (a, id as u16))
            })
            .filter(|(a, _)| !a.is_zero())
            .collect();
        let sources: Vec<Address> = asset_sources.iter().map(|(_, s)| *s).collect();
        let aggregators = mev_oracle::resolve_aggregators(&rpc.fast, &sources).await?;
        let feed_masks: HashMap<Address, U256> = mev_lending::feed_index(&aggregators, |src| {
            asset_sources
                .iter()
                .filter(|(_, s)| *s == src)
                .filter_map(|(a, _)| ids.get(a).copied())
                .collect()
        })
        .into_iter()
        .map(|(agg, ids)| {
            (
                agg,
                ids.into_iter()
                    .fold(U256::ZERO, |m, id| m | reserve_bits(id)),
            )
        })
        .collect();
        let feed_assets: HashMap<Address, Vec<Address>> =
            mev_lending::feed_index(&aggregators, |src| {
                asset_sources
                    .iter()
                    .filter(|(_, s)| *s == src)
                    .map(|(a, _)| *a)
                    .collect()
            })
            .into_iter()
            .map(|(agg, assets)| (agg, assets.into_iter().collect()))
            .collect();
        info!(
            protocol = ID,
            reserves = reserves.len(),
            price_feeds = feed_masks.len(),
            borrowers_cached = index.len(),
            "adapter ready"
        );
        Ok(Self {
            cfg,
            rpc,
            index,
            reserves,
            decimals,
            feed_masks,
            user_masks: Mutex::new(HashMap::new()),
            feed_assets,
            snapshot: Mutex::new(Snapshot::default()),
        })
    }

    /// Which reserves at-risk users touch, so feed updates re-poll only exposed users. Runs on
    /// the sweep RPC; on failure the old (or no) bitmap stays and `affected` stays conservative.
    async fn refresh_user_masks(&self, hs: &[Health]) {
        let users: Vec<Address> = hs
            .iter()
            .filter(|h| h.debt_usd > 0.0 && h.health_factor < EXPOSURE_HF)
            .map(|h| h.account.account)
            .collect();
        if users.is_empty() {
            return;
        }
        let calls: Vec<_> = users
            .iter()
            .map(|u| {
                call(
                    self.cfg.pool,
                    IAavePool::getUserConfigurationCall { user: *u },
                )
            })
            .collect();
        match multicall(&self.rpc.sweep, &calls, 200).await {
            Ok(res) => {
                let mut masks = self.user_masks.lock().unwrap();
                for (u, r) in users.iter().zip(res.iter()) {
                    if let Some(m) = decode::<IAavePool::getUserConfigurationCall>(r) {
                        masks.insert(*u, m);
                    }
                }
            }
            Err(e) => warn!(protocol = ID, error = %format!("{e:#}"), "user reserve bitmaps"),
        }
    }

    fn account(&self, user: Address) -> AccountRef {
        AccountRef {
            market: self.cfg.pool,
            account: user,
            sub_id: 0,
        }
    }

    async fn reserves_info(&self) -> Result<Vec<ReserveInfo>> {
        self.reserves_info_on(&self.rpc.fast).await
    }

    async fn reserves_info_on(&self, p: &DynProvider) -> Result<Vec<ReserveInfo>> {
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
        let res = multicall(p, &calls, 200).await?;
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
                    liquidation_threshold_bps: u32::try_from(c.liquidationThreshold).unwrap_or(0),
                    price_usd: f64::from(px) / 1e8,
                })
            })
            .collect())
    }

    async fn position(&self, user: Address, health_factor: f64) -> Result<Position> {
        let mut v = self
            .positions_on(&self.rpc.fast, &[(user, health_factor)])
            .await?;
        v.pop().context("no position")
    }

    /// Balances per reserve for each `(user, health factor)`, in one batched multicall.
    async fn positions_on(
        &self,
        p: &DynProvider,
        users: &[(Address, f64)],
    ) -> Result<Vec<Position>> {
        let n = self.reserves.len().max(1);
        let calls: Vec<_> = users
            .iter()
            .flat_map(|(user, _)| {
                self.reserves.iter().map(|a| {
                    call(
                        self.cfg.data_provider,
                        IAaveDataProvider::getUserReserveDataCall {
                            asset: *a,
                            user: *user,
                        },
                    )
                })
            })
            .collect();
        let res = multicall(p, &calls, 200).await?;
        Ok(users
            .iter()
            .zip(res.chunks(n))
            .map(|((user, hf), rows)| self.position_from(*user, *hf, rows))
            .collect())
    }

    fn position_from(
        &self,
        user: Address,
        health_factor: f64,
        rows: &[Option<alloy::primitives::Bytes>],
    ) -> Position {
        let mut pos = Position {
            user,
            health_factor,
            ..Default::default()
        };
        for (a, r) in self.reserves.iter().zip(rows.iter()) {
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
        pos
    }

    /// Refresh the prediction snapshot for at-risk users (sweep RPC: free, not latency-critical).
    async fn refresh_snapshot(&self, hs: &[Health]) {
        let users: Vec<(Address, f64)> = hs
            .iter()
            .filter(|h| h.debt_usd > 0.0 && h.health_factor < EXPOSURE_HF)
            .map(|h| (h.account.account, h.health_factor))
            .collect();
        let p = &self.rpc.sweep;
        let aggs: Vec<Address> = self.feed_assets.keys().copied().collect();
        let answer_calls: Vec<_> = aggs
            .iter()
            .map(|a| call(*a, IAggregator::latestAnswerCall {}))
            .collect();
        let fetched = async {
            let reserves = self.reserves_info_on(p).await?;
            let positions = self.positions_on(p, &users).await?;
            let answers = multicall(p, &answer_calls, 200).await?;
            anyhow::Ok((reserves, positions, answers))
        };
        match fetched.await {
            Ok((reserves, positions, answers)) => {
                let cached = positions
                    .into_iter()
                    .filter_map(|pos| Cached::new(pos, &reserves))
                    .collect();
                let answers = aggs
                    .iter()
                    .zip(answers.iter())
                    .filter_map(|(a, r)| {
                        let v = decode::<IAggregator::latestAnswerCall>(r)?;
                        Some((*a, v.to_string().parse::<f64>().ok()?))
                    })
                    .collect();
                *self.snapshot.lock().unwrap() = Snapshot {
                    reserves,
                    cached,
                    answers,
                };
            }
            Err(e) => warn!(protocol = ID, error = %format!("{e:#}"), "prediction snapshot"),
        }
    }

    /// Executor plan for liquidating `pos` as sized by `p`.
    fn build_plan(&self, account: AccountRef, pos: &Position, p: &LiquidationPlan) -> Plan {
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
            user: account.account,
            debtToCover: U256::MAX,
            receiveAToken: false,
        }
        .abi_encode();
        Plan {
            protocol: ID,
            account,
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
        }
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
        let pool = self.cfg.pool;
        // Borrow(reserve, user, onBehalfOf indexed, ...): the debt belongs to onBehalfOf (topic 2).
        self.index
            .step(&self.rpc.scan, |l| {
                topic_address(l, 2)
                    .map(|u| vec![(market_key(pool), u)])
                    .unwrap_or_default()
            })
            .await
    }

    fn accounts(&self) -> Vec<AccountRef> {
        self.index
            .accounts()
            .map(|(_, u)| self.account(*u))
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
        let hs: Vec<Health> = accounts
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
            .collect();
        if !fast {
            self.refresh_user_masks(&hs).await;
            self.refresh_snapshot(&hs).await;
        }
        Ok(hs)
    }

    async fn plan(&self, h: &Health, _executor: Address) -> Result<Option<Plan>> {
        let reserves = self.reserves_info().await?;
        let pos = self.position(h.account.account, h.health_factor).await?;
        Ok(math::plan(&pos, &reserves).map(|p| self.build_plan(h.account, &pos, &p)))
    }

    fn predict(&self, feed: Address, answer: f64) -> Vec<Plan> {
        let Some(assets) = self.feed_assets.get(&feed) else {
            return vec![];
        };
        let mut snap = self.snapshot.lock().unwrap();
        let old = snap.answers.insert(feed, answer);
        let Some(old) = old.filter(|o| *o != 0.0) else {
            return vec![]; // first sighting: nothing to compare against
        };
        let Snapshot {
            reserves, cached, ..
        } = &mut *snap;
        math::predict(reserves, cached, assets, answer / old)
            .into_iter()
            .map(|(pos, p)| self.build_plan(self.account(pos.user), &pos, &p))
            .collect()
    }

    fn price_feeds(&self) -> Vec<Address> {
        self.feed_masks.keys().copied().collect()
    }

    fn affected(&self, feed: Address, account: &AccountRef) -> bool {
        let Some(feed_mask) = self.feed_masks.get(&feed) else {
            return true;
        };
        match self.user_masks.lock().unwrap().get(&account.account) {
            Some(user) => !(*user & *feed_mask).is_zero(),
            None => true,
        }
    }

    fn targets(&self) -> Vec<Address> {
        vec![self.cfg.pool]
    }

    fn assets(&self) -> Vec<Address> {
        self.reserves.clone()
    }
}

/// One-time migration from the pre-`mev-lending-index` cache format (`borrowers: [address]`).
fn migrate_v1_cache(path: &std::path::Path, pool: Address) {
    #[derive(serde::Deserialize)]
    struct V1 {
        next_block: u64,
        back_block: u64,
        borrowers: Vec<Address>,
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let Ok(v1) = serde_json::from_slice::<V1>(&bytes) else {
        return;
    };
    let v2 = serde_json::json!({
        "next_block": v1.next_block,
        "back_block": v1.back_block,
        "accounts": v1.borrowers.iter().map(|b| (pool, *b)).collect::<Vec<_>>(),
    });
    if std::fs::write(path, v2.to_string()).is_ok() {
        info!(
            protocol = ID,
            borrowers = v1.borrowers.len(),
            "migrated borrower cache to v2 format"
        );
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn v1_cache_is_migrated_to_market_account_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aave.json");
        let pool = Address::with_last_byte(0xaa);
        let user = Address::with_last_byte(1);
        std::fs::write(
            &path,
            serde_json::json!({ "next_block": 10, "back_block": 5, "borrowers": [user] })
                .to_string(),
        )
        .unwrap();
        migrate_v1_cache(&path, pool);
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["next_block"], 10);
        assert_eq!(
            v["accounts"][0][0].as_str().unwrap().to_lowercase(),
            pool.to_string().to_lowercase()
        );
        assert_eq!(
            v["accounts"][0][1].as_str().unwrap().to_lowercase(),
            user.to_string().to_lowercase()
        );
        // Already-migrated (or unrelated) files are left alone.
        let before = std::fs::read(&path).unwrap();
        migrate_v1_cache(&path, pool);
        assert_eq!(before, std::fs::read(&path).unwrap());
    }
}

#[cfg(test)]
mod exposure_tests {
    use super::*;

    #[test]
    fn reserve_bits_cover_borrow_and_collateral_flags() {
        assert_eq!(reserve_bits(0), U256::from(0b11));
        assert_eq!(reserve_bits(2), U256::from(0b11_0000));
        // A user borrowing reserve 3 and posting reserve 0 as collateral.
        let user = (U256::from(1u8) << 6usize) | (U256::from(1u8) << 1usize);
        assert!(!(user & reserve_bits(3)).is_zero());
        assert!(!(user & reserve_bits(0)).is_zero());
        assert!((user & reserve_bits(1)).is_zero());
    }
}
