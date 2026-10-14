//! `AccountRechecker` (JEV-LIQ-003): one account, one block, exact integers.
//!
//! Reads `getUserAccountData` through an EIP-1898 hash-pinned `eth_call` so every field of the
//! returned snapshot comes from the same block. The f64 `health()` path is unrelated and unchanged.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::{eips::BlockId, primitives::Address, providers::DynProvider};
use anyhow::{Context, Result};
use mev_bindings::IAavePool;
use mev_liq_state::{AccountKey, BorrowerSnapshot, ChainRef, ChainSource, SnapshotStatus};

pub const PROVENANCE: &str = "aave-v3:getUserAccountData@eip1898-hash";

#[derive(Clone)]
pub struct AaveAccountRechecker {
    pub chain_id: u64,
    pub pool: Address,
    pub provider: DynProvider,
    /// Version of the reserve-risk metadata the caller considers current (0 = none tracked yet).
    pub risk_metadata_version: u64,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

impl AaveAccountRechecker {
    /// Rebuild the snapshot of `borrower` at exactly `at` (its block hash pins the read). The
    /// result has `revision` 0 (the repository assigns it), no per-reserve balances
    /// (`getUserAccountData` doesn't return them) and `at.source` as given.
    pub async fn recheck(&self, borrower: Address, at: &ChainRef) -> Result<BorrowerSnapshot> {
        let block = BlockId::hash_canonical(at.block_hash);
        let d = IAavePool::new(self.pool, &self.provider)
            .getUserAccountData(borrower)
            .block(block)
            .call()
            .await
            .with_context(|| {
                format!(
                    "getUserAccountData({borrower}) at block {}",
                    at.block_number
                )
            })?;
        let observed = ChainRef {
            observed_at_ns: now_ns(),
            ..*at
        };
        Ok(BorrowerSnapshot {
            key: AccountKey {
                chain_id: self.chain_id,
                pool: self.pool,
                borrower,
            },
            revision: 0,
            chain: observed,
            health_factor_wad: d.healthFactor,
            total_debt_base: d.totalDebtBase,
            total_collateral_base: d.totalCollateralBase,
            available_borrows_base: d.availableBorrowsBase,
            liquidation_threshold: d.currentLiquidationThreshold,
            ltv: d.ltv,
            reserves: vec![],
            risk_metadata_version: self.risk_metadata_version,
            dirty_reasons: vec![],
            last_successful_check_ns: Some(observed.observed_at_ns),
            status: SnapshotStatus::Valid,
            provenance: PROVENANCE.into(),
            feature_digest: Default::default(),
        }
        .seal())
    }

    /// Pin the current head (one `snapshot_block` call, hash + number from the same header) and
    /// recheck there. The block is `Provisional`: only the repository's canonical checkpoint may
    /// promote it.
    pub async fn recheck_at_head(&self, borrower: Address) -> Result<BorrowerSnapshot> {
        let (id, number) = mev_rpc::snapshot_block(&self.provider).await?;
        let hash = id
            .as_block_hash()
            .context("snapshot_block returned a non-hash block id")?;
        self.recheck(
            borrower,
            &ChainRef {
                block_number: number,
                block_hash: hash,
                source: ChainSource::Provisional,
                observed_at_ns: now_ns(),
            },
        )
        .await
    }
}
