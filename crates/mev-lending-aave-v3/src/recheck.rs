//! `AccountRechecker` (JEV-LIQ-003): one account, one block, exact integers.
//!
//! Reads `getUserAccountData` through an EIP-1898 hash-pinned `eth_call` so every field of the
//! returned snapshot comes from the same block. The f64 `health()` path is unrelated and unchanged.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    primitives::Address,
    providers::{DynProvider, Provider},
};
use anyhow::{Context, Result};
use mev_bindings::IAavePool;
use mev_liq_state::{AccountKey, BorrowerSnapshot, ChainRef, ChainSource, SnapshotStatus};
use mev_rpc::{call, decode, multicall_at};

pub const PROVENANCE: &str = "aave-v3:getUserAccountData@eip1898-hash";
/// Provenance of snapshots read in a batch (Multicall3 `aggregate3`, same pinned block).
pub const PROVENANCE_BATCH: &str = "aave-v3:getUserAccountData@multicall3+eip1898-hash";
/// Accounts per `aggregate3` request: `getUserAccountData` loops over every reserve, so the
/// per-request gas stays well under node `eth_call` caps (same size as the legacy sweep).
pub const RECHECK_CHUNK: usize = 50;
/// A batch is pinned this many blocks behind the head (~2 s on Arbitrum) so every endpoint
/// behind a rotating provider already has the block.
pub const PIN_LAG_BLOCKS: u64 = 8;

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
        Ok(self.snapshot(borrower, &d, at, PROVENANCE))
    }

    fn snapshot(
        &self,
        borrower: Address,
        d: &IAavePool::getUserAccountDataReturn,
        at: &ChainRef,
        provenance: &str,
    ) -> BorrowerSnapshot {
        let observed = ChainRef {
            observed_at_ns: now_ns(),
            ..*at
        };
        BorrowerSnapshot {
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
            provenance: provenance.into(),
            feature_digest: Default::default(),
        }
        .seal()
    }

    /// Recheck many accounts at one block: `PIN_LAG_BLOCKS` behind the head, pinned by hash,
    /// read through Multicall3 in `RECHECK_CHUNK`-sized requests. Returns the block and one entry
    /// per input in order; `None` where that account's call failed. Any request failing fails
    /// the whole batch (nothing partial from a different block).
    pub async fn recheck_many(
        &self,
        borrowers: &[Address],
    ) -> Result<(ChainRef, Vec<Option<BorrowerSnapshot>>)> {
        let head = self
            .provider
            .get_block_number()
            .await
            .context("block number")?;
        let n = head.saturating_sub(PIN_LAG_BLOCKS);
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(n))
            .await
            .with_context(|| format!("block {n}"))?
            .with_context(|| format!("block {n} unavailable"))?;
        let at = ChainRef {
            block_number: block.header.number,
            block_hash: block.header.hash,
            source: ChainSource::Provisional,
            observed_at_ns: now_ns(),
        };
        let calls: Vec<_> = borrowers
            .iter()
            .map(|u| call(self.pool, IAavePool::getUserAccountDataCall { user: *u }))
            .collect();
        let res = multicall_at(
            &self.provider,
            &calls,
            RECHECK_CHUNK,
            BlockId::hash_canonical(at.block_hash),
        )
        .await
        .with_context(|| format!("getUserAccountData x{} at block {n}", borrowers.len()))?;
        anyhow::ensure!(
            res.len() == borrowers.len(),
            "multicall returned {} results for {} calls",
            res.len(),
            borrowers.len()
        );
        let snaps = borrowers
            .iter()
            .zip(&res)
            .map(|(u, r)| {
                decode::<IAavePool::getUserAccountDataCall>(r)
                    .map(|d| self.snapshot(*u, &d, &at, PROVENANCE_BATCH))
            })
            .collect();
        Ok((at, snaps))
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
