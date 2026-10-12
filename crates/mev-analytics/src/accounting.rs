//! Read-only, receipt-backed accounting migration. Does not sign or broadcast.
use std::collections::BTreeSet;

use alloy::{
    consensus::Transaction,
    eips::{BlockId, BlockNumberOrTag},
    network::TransactionResponse,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
};
use anyhow::{Context, Result, ensure};
use mev_risk::ledger::{Pending, Seed, SeedReceipt, native_cost};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub chain_id: u64,
    pub owner: Address,
    /// Last block before UTC midnight; the next block must be in the accounting day.
    pub before_day_block: u64,
    /// One conservative, integer USD micro-dollar value per ETH, reviewed by the operator.
    pub native_price_microusd: u64,
    pub confirmed: Vec<Entry>,
    pub pending: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub hash: B256,
    pub arb: bool,
}

/// Verifies every nonce in the day, receipts, canonical hashes and unresolved pending terms.
/// Failure produces no ledger. A second head/nonce check rejects changes during preparation.
pub async fn prepare(provider: &DynProvider, manifest: Manifest) -> Result<Seed> {
    ensure!(
        manifest.native_price_microusd > 0 && !manifest.owner.is_zero(),
        "invalid valuation or owner"
    );
    ensure!(
        manifest.confirmed.len() <= 10_000 && manifest.pending.len() <= 1024,
        "manifest too large"
    );
    let mut hashes = BTreeSet::new();
    ensure!(
        manifest
            .confirmed
            .iter()
            .chain(&manifest.pending)
            .all(|e| hashes.insert(e.hash)),
        "duplicate transaction hash"
    );
    ensure!(
        provider.get_chain_id().await? == manifest.chain_id,
        "chain ID mismatch"
    );
    let head = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .context("head unavailable")?;
    let now = head.header.timestamp;
    let day = now / 86_400;
    let before = provider
        .get_block_by_number(manifest.before_day_block.into())
        .await?
        .context("midnight boundary unavailable")?;
    let next = provider
        .get_block_by_number(
            manifest
                .before_day_block
                .checked_add(1)
                .context("block overflow")?
                .into(),
        )
        .await?
        .context("midnight successor unavailable")?;
    ensure!(
        before.header.number == manifest.before_day_block
            && before.header.timestamp < day * 86_400
            && next.header.timestamp >= day * 86_400
            && next.header.parent_hash == before.header.hash
            && next.header.number <= head.header.number,
        "invalid UTC accounting boundary"
    );
    let starting_nonce = provider
        .get_transaction_count(manifest.owner)
        .block_id(BlockId::hash_canonical(before.header.hash))
        .await?;
    let confirmed_nonce = provider
        .get_transaction_count(manifest.owner)
        .block_id(BlockId::hash_canonical(head.header.hash))
        .await?;
    let pending_nonce = provider
        .get_transaction_count(manifest.owner)
        .pending()
        .await?;
    ensure!(
        confirmed_nonce.checked_sub(starting_nonce) == Some(manifest.confirmed.len() as u64),
        "confirmed nonce coverage incomplete"
    );
    ensure!(
        pending_nonce.checked_sub(confirmed_nonce) == Some(manifest.pending.len() as u64),
        "pending nonce coverage incomplete"
    );
    let mut receipts = Vec::with_capacity(manifest.confirmed.len());
    for entry in manifest.confirmed {
        let tx = provider
            .get_transaction_by_hash(entry.hash)
            .await?
            .context("transaction unavailable")?;
        let receipt = provider
            .get_transaction_receipt(entry.hash)
            .await?
            .context("receipt unavailable")?;
        ensure!(
            receipt.transaction_hash == entry.hash
                && tx.tx_hash() == entry.hash
                && receipt.from == manifest.owner
                && tx.inner.signer() == manifest.owner,
            "receipt identity mismatch"
        );
        ensure!(
            tx.inner.chain_id().is_none_or(|id| id == manifest.chain_id),
            "transaction chain mismatch"
        );
        let number = receipt.block_number.context("receipt block missing")?;
        let block = provider
            .get_block_by_number(number.into())
            .await?
            .context("receipt block unavailable")?;
        ensure!(
            number <= head.header.number
                && receipt.block_hash == Some(block.header.hash)
                && tx.block_hash == receipt.block_hash
                && block.header.timestamp / 86_400 == day,
            "receipt is not canonical in the accounting day"
        );
        let gas = native_cost(
            receipt.gas_used,
            receipt.effective_gas_price,
            manifest.native_price_microusd,
        )?;
        // Priority charged on Arbitrum is bounded by the signed priority cap. Total receipt gas
        // includes data charging, so this is deliberately a conservative tip debit.
        let tip = native_cost(
            receipt.gas_used,
            tx.inner.max_priority_fee_per_gas().unwrap_or(0),
            manifest.native_price_microusd,
        )?
        .min(gas);
        receipts.push(SeedReceipt {
            nonce: tx.inner.nonce(),
            gas_microusd: gas,
            tip_microusd: tip,
            timestamp: block.header.timestamp,
            arb: entry.arb,
            success: receipt.status(),
        });
    }
    let mut pending = Vec::with_capacity(manifest.pending.len());
    for entry in manifest.pending {
        let tx = provider
            .get_transaction_by_hash(entry.hash)
            .await?
            .context("pending transaction unavailable")?;
        ensure!(
            tx.tx_hash() == entry.hash
                && tx.inner.signer() == manifest.owner
                && tx.block_hash.is_none()
                && tx.inner.chain_id().is_none_or(|id| id == manifest.chain_id),
            "pending identity mismatch"
        );
        ensure!(
            provider
                .get_transaction_receipt(entry.hash)
                .await?
                .is_none(),
            "pending transaction already has a receipt; refresh manifest"
        );
        let bound = native_cost(
            tx.inner.gas_limit(),
            tx.inner.max_fee_per_gas(),
            manifest.native_price_microusd,
        )?;
        let tip = native_cost(
            tx.inner.gas_limit(),
            tx.inner.max_priority_fee_per_gas().unwrap_or(0),
            manifest.native_price_microusd,
        )?;
        pending.push(Pending {
            id: 0,
            bound_microusd: bound,
            tip_bound_microusd: tip,
            arb: entry.arb,
            hash: Some(entry.hash.to_string()),
            nonce: Some(tx.inner.nonce()),
            native_price_microusd: manifest.native_price_microusd,
            created_secs: now,
            from: Some(manifest.owner.to_string()),
        });
    }
    let anchor = provider
        .get_block_by_number(head.header.number.into())
        .await?
        .context("anchor unavailable")?;
    ensure!(
        anchor.header.hash == head.header.hash,
        "anchor reorged during reconciliation"
    );
    let latest_nonce = provider.get_transaction_count(manifest.owner).await?;
    let latest_pending = provider
        .get_transaction_count(manifest.owner)
        .pending()
        .await?;
    ensure!(
        latest_nonce == confirmed_nonce && latest_pending == pending_nonce,
        "wallet changed during reconciliation; refresh manifest"
    );
    Ok(Seed {
        chain_id: manifest.chain_id,
        owner: manifest.owner,
        block: head.header.number,
        block_hash: head.header.hash,
        now,
        starting_nonce,
        confirmed_nonce,
        receipts,
        pending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{providers::ProviderBuilder, rpc::types::Block, transports::mock::Asserter};

    fn block(number: u64, timestamp: u64, hash: u8, parent: u8) -> Block {
        let mut b: Block = Block::default();
        b.header.inner.number = number;
        b.header.inner.timestamp = timestamp;
        b.header.hash = B256::repeat_byte(hash);
        b.header.inner.parent_hash = B256::repeat_byte(parent);
        b
    }
    fn manifest() -> Manifest {
        Manifest {
            chain_id: 42161,
            owner: Address::with_last_byte(1),
            before_day_block: 10,
            native_price_microusd: 2_000_000_000,
            confirmed: vec![],
            pending: vec![],
        }
    }

    #[tokio::test]
    async fn empty_wallet_day_is_verified_and_reorg_or_missing_nonce_coverage_rejects() {
        for case in 0..3 {
            let mock = Asserter::new();
            let provider = ProviderBuilder::new()
                .connect_mocked_client(mock.clone())
                .erased();
            mock.push_success(&"0xa4b1");
            mock.push_success(&block(12, 86_500, 12, 11));
            mock.push_success(&block(10, 86_399, 10, 9));
            mock.push_success(&block(11, 86_400, 11, 10));
            mock.push_success(&"0x7");
            mock.push_success(if case == 1 { &"0x8" } else { &"0x7" });
            mock.push_success(&"0x7");
            mock.push_success(&block(12, 86_500, if case == 2 { 99 } else { 12 }, 11));
            mock.push_success(&"0x7");
            mock.push_success(&"0x7");
            let result = prepare(&provider, manifest()).await;
            if case == 0 {
                let seed = result.unwrap();
                assert_eq!(seed.starting_nonce, 7);
                assert_eq!(seed.confirmed_nonce, 7);
                assert_eq!(seed.block_hash, B256::repeat_byte(12));
                assert!(seed.receipts.is_empty() && seed.pending.is_empty());
                assert!(mock.read_q().is_empty());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn duplicate_hashes_reject_before_rpc() {
        let mock = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(mock).erased();
        let mut m = manifest();
        m.confirmed.push(Entry {
            hash: B256::repeat_byte(1),
            arb: true,
        });
        m.pending.push(Entry {
            hash: B256::repeat_byte(1),
            arb: true,
        });
        assert!(
            prepare(&provider, m)
                .await
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }
}
