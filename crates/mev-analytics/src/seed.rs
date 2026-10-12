//! Operator tool core: build the first durable risk ledger from chain evidence (H01/M11).
//!
//! Live mode refuses to start without a ledger that records the wallet identity and the block it was
//! reconciled at (`Ledger::initialize` + `persistent_existing`). This module turns canonical RPC facts about
//! the wallet's own transactions into a [`Seed`]: every receipt of the current UTC day (contiguous nonces,
//! no gaps) and every transaction still pending. The RPC reading lives in the `mev-ledger-seed` binary;
//! everything here is pure so the reconciliation rules are unit-tested.

use std::collections::{BTreeMap, HashMap};

use alloy::primitives::{Address, B256};
use anyhow::{Result, bail, ensure};
use mev_risk::ledger::{Pending, Seed, SeedReceipt, native_cost};

/// A transaction the wallet signed, as the node reports it.
#[derive(Debug, Clone)]
pub struct TxInfo {
    pub hash: B256,
    pub nonce: u64,
    pub from: Address,
    pub gas_limit: u64,
    pub max_fee: u128,
    pub max_priority: u128,
}

/// What the canonical receipt and its block say about a mined transaction.
#[derive(Debug, Clone, Copy)]
pub struct Mined {
    pub block_ts: u64,
    pub base_fee: u128,
    pub gas_used: u64,
    pub effective_price: u128,
    pub success: bool,
}

pub struct Params {
    pub chain_id: u64,
    pub owner: Address,
    pub block: u64,
    pub block_hash: B256,
    /// Timestamp of `block`: the ledger's "now".
    pub now: u64,
    /// Transaction count of `owner` at `block`.
    pub confirmed_nonce: u64,
    /// Transaction count including the mempool.
    pub pending_nonce: u64,
    /// USD per whole native token, micro-USD. Receipts are priced at this one conservative quote.
    pub native_price_microusd: u64,
}

/// `txs` maps nonce -> the transaction that holds it (the mined one for confirmed nonces, the live one for
/// pending nonces); `mined` maps hash -> receipt facts. Any hole in the evidence is an error: an incomplete
/// history would silently admit spend the wallet already made today.
pub fn assemble(
    p: &Params,
    txs: &BTreeMap<u64, TxInfo>,
    mined: &HashMap<B256, Mined>,
) -> Result<Seed> {
    ensure!(p.native_price_microusd > 0, "native price must be positive");
    ensure!(
        p.pending_nonce >= p.confirmed_nonce,
        "pending nonce below confirmed nonce"
    );
    let day_start = p.now / 86_400 * 86_400;
    let mut receipts = Vec::new();
    let mut starting_nonce = 0;
    let mut n = p.confirmed_nonce;
    while n > 0 {
        let nonce = n - 1;
        let Some(tx) = txs.get(&nonce) else {
            bail!(
                "no transaction supplied for nonce {nonce}: cannot prove it predates today ({day_start}); add its hash to the list"
            );
        };
        ensure!(
            tx.from == p.owner && tx.nonce == nonce,
            "transaction {} is not the owner's nonce {nonce}",
            tx.hash
        );
        let Some(m) = mined.get(&tx.hash) else {
            bail!(
                "transaction {} (nonce {nonce}) has no canonical receipt at or before block {}",
                tx.hash,
                p.block
            );
        };
        if m.block_ts < day_start {
            starting_nonce = n;
            break;
        }
        ensure!(
            m.block_ts <= p.now,
            "receipt {} is newer than the reconciliation block",
            tx.hash
        );
        let gas_microusd = native_cost(m.gas_used, m.effective_price, p.native_price_microusd)
            .map_err(|e| anyhow::anyhow!("fee overflow for {}: {e:?}", tx.hash))?;
        let paid_tip = m.effective_price.saturating_sub(m.base_fee);
        let tip_microusd = native_cost(m.gas_used, paid_tip, p.native_price_microusd)
            .map_err(|e| anyhow::anyhow!("tip overflow for {}: {e:?}", tx.hash))?
            .min(gas_microusd);
        receipts.push(SeedReceipt {
            nonce,
            gas_microusd,
            tip_microusd,
            timestamp: m.block_ts,
            // The wallet's transactions are not classified here; counting all as arbitrage spend only
            // tightens the arbitrage budget.
            arb: true,
            success: m.success,
        });
        n = nonce;
    }
    receipts.reverse();
    let mut pending = Vec::new();
    for nonce in p.confirmed_nonce..p.pending_nonce {
        let Some(tx) = txs.get(&nonce) else {
            bail!(
                "pending nonce {nonce} has no supplied transaction: the mempool holds something the ledger cannot account for"
            );
        };
        ensure!(
            tx.from == p.owner && tx.nonce == nonce,
            "pending {} is not the owner's nonce {nonce}",
            tx.hash
        );
        let bound = native_cost(tx.gas_limit, tx.max_fee, p.native_price_microusd)
            .map_err(|e| anyhow::anyhow!("pending fee overflow: {e:?}"))?;
        let tip_bound = native_cost(
            tx.gas_limit,
            tx.max_priority.min(tx.max_fee),
            p.native_price_microusd,
        )
        .map_err(|e| anyhow::anyhow!("pending tip overflow: {e:?}"))?;
        pending.push(Pending {
            id: 0,
            bound_microusd: bound.max(1),
            arb: true,
            hash: Some(format!("{:#x}", tx.hash)),
            nonce: Some(nonce),
            native_price_microusd: p.native_price_microusd,
            created_secs: p.now,
            from: Some(format!("{:#x}", p.owner)),
            tip_bound_microusd: tip_bound.min(bound.max(1)),
        });
    }
    Ok(Seed {
        chain_id: p.chain_id,
        owner: p.owner,
        block: p.block,
        block_hash: p.block_hash,
        now: p.now,
        starting_nonce,
        confirmed_nonce: p.confirmed_nonce,
        receipts,
        pending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: Address = Address::repeat_byte(0xaa);
    const NOW: u64 = 86_400 * 100 + 5_000;

    fn tx(nonce: u64) -> TxInfo {
        TxInfo {
            hash: B256::repeat_byte(nonce as u8 + 1),
            nonce,
            from: OWNER,
            gas_limit: 1_000_000,
            max_fee: 100_000_000,
            max_priority: 10_000_000,
        }
    }

    fn mined(ts: u64, ok: bool) -> Mined {
        Mined {
            block_ts: ts,
            base_fee: 20_000_000,
            gas_used: 400_000,
            effective_price: 30_000_000,
            success: ok,
        }
    }

    fn params(confirmed: u64, pending: u64) -> Params {
        Params {
            chain_id: 42161,
            owner: OWNER,
            block: 7,
            block_hash: B256::repeat_byte(9),
            now: NOW,
            confirmed_nonce: confirmed,
            pending_nonce: pending,
            native_price_microusd: 3_000_000_000,
        }
    }

    #[test]
    fn todays_receipts_are_priced_and_yesterdays_stop_the_walk() {
        let txs: BTreeMap<u64, TxInfo> = (0..4).map(|n| (n, tx(n))).collect();
        let m: HashMap<B256, Mined> = [
            (tx(0).hash, mined(NOW - 90_000, true)), // yesterday
            (tx(1).hash, mined(NOW - 900, true)),
            (tx(2).hash, mined(NOW - 500, false)),
            (tx(3).hash, mined(NOW - 10, true)),
        ]
        .into();
        let s = assemble(&params(4, 4), &txs, &m).unwrap();
        assert_eq!(s.starting_nonce, 1);
        assert_eq!(
            s.receipts.iter().map(|r| r.nonce).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // 400_000 gas * 0.03 gwei * $3000 = $0.036 -> 36_000 micro-USD; tip part is 10M wei of the 30M.
        assert_eq!(s.receipts[0].gas_microusd, 36_000);
        assert_eq!(s.receipts[0].tip_microusd, 12_000);
        assert!(!s.receipts[1].success);
        // The seed is accepted by the ledger's own validation.
        let dir = std::env::temp_dir().join(format!("seed-ok-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        mev_risk::ledger::Ledger::initialize(dir.clone(), s).unwrap();
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn a_missing_hash_or_receipt_is_refused_not_skipped() {
        let mut txs: BTreeMap<u64, TxInfo> = (0..3).map(|n| (n, tx(n))).collect();
        let m: HashMap<B256, Mined> = (0..3)
            .map(|n| (tx(n).hash, mined(NOW - 100, true)))
            .collect();
        txs.remove(&1);
        assert!(
            assemble(&params(3, 3), &txs, &m)
                .unwrap_err()
                .to_string()
                .contains("nonce 1")
        );
        let txs: BTreeMap<u64, TxInfo> = (0..3).map(|n| (n, tx(n))).collect();
        let mut m2 = m.clone();
        m2.remove(&tx(2).hash);
        assert!(
            assemble(&params(3, 3), &txs, &m2)
                .unwrap_err()
                .to_string()
                .contains("no canonical receipt")
        );
    }

    #[test]
    fn pending_transactions_become_liabilities_and_unknown_ones_are_refused() {
        let txs: BTreeMap<u64, TxInfo> = (0..3).map(|n| (n, tx(n))).collect();
        let m: HashMap<B256, Mined> = (0..2)
            .map(|n| (tx(n).hash, mined(NOW - 100, true)))
            .collect();
        let s = assemble(&params(2, 3), &txs, &m).unwrap();
        assert_eq!(s.pending.len(), 1);
        // 1_000_000 gas * 0.1 gwei * $3000 = $0.30.
        assert_eq!(s.pending[0].bound_microusd, 300_000);
        assert_eq!(s.pending[0].tip_bound_microusd, 30_000);
        assert!(
            assemble(&params(2, 4), &txs, &m).is_err(),
            "nonce 3 is in the mempool but unaccounted"
        );
    }

    #[test]
    fn a_foreign_transaction_is_refused() {
        let mut txs: BTreeMap<u64, TxInfo> = (0..1).map(|n| (n, tx(n))).collect();
        txs.get_mut(&0).unwrap().from = Address::repeat_byte(1);
        let m: HashMap<B256, Mined> = [(tx(0).hash, mined(NOW - 100, true))].into();
        assert!(assemble(&params(1, 1), &txs, &m).is_err());
    }
}
