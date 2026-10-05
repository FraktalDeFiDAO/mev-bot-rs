//! Per-block state diffs (`debug_traceBlockByNumber` + `prestateTracer` in `diffMode`), applied to
//! the cache so its base state is exact as of the last applied block — no time-based staleness.
//!
//! diffMode semantics (geth): per transaction, `pre` holds the touched accounts' prior values and
//! `post` only what changed. A storage slot in `pre` but absent from `post` was cleared to zero; an
//! account in `pre` but absent from `post` was destroyed.

use std::collections::BTreeMap;

use alloy::{
    primitives::{Address, B256, Bytes, U256},
    providers::{DynProvider, Provider},
};
use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Default, Clone, Deserialize)]
pub struct AccountState {
    pub balance: Option<U256>,
    pub nonce: Option<u64>,
    pub code: Option<Bytes>,
    #[serde(default)]
    pub storage: BTreeMap<B256, B256>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct StateDiff {
    #[serde(default)]
    pub pre: BTreeMap<Address, AccountState>,
    #[serde(default)]
    pub post: BTreeMap<Address, AccountState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TxDiff {
    #[serde(rename = "txHash")]
    pub tx_hash: Option<B256>,
    pub result: Option<StateDiff>,
}

/// One change to apply, in execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Balance(Address, U256),
    Nonce(Address, u64),
    Code(Address, Bytes),
    Slot(Address, U256, U256),
    Destroyed(Address),
}

/// The changes a block's diffs describe, in transaction order.
pub fn changes(block: &[TxDiff]) -> Vec<Change> {
    let mut out = Vec::new();
    for d in block.iter().filter_map(|t| t.result.as_ref()) {
        for (addr, pre) in &d.pre {
            match d.post.get(addr) {
                None => out.push(Change::Destroyed(*addr)),
                Some(post) => {
                    // Slots cleared to zero appear only in pre.
                    for slot in pre.storage.keys() {
                        if !post.storage.contains_key(slot) {
                            out.push(Change::Slot(*addr, U256::from_be_bytes(slot.0), U256::ZERO));
                        }
                    }
                }
            }
        }
        for (addr, post) in &d.post {
            if let Some(b) = post.balance {
                out.push(Change::Balance(*addr, b));
            }
            if let Some(n) = post.nonce {
                out.push(Change::Nonce(*addr, n));
            }
            if let Some(c) = &post.code {
                out.push(Change::Code(*addr, c.clone()));
            }
            for (slot, val) in &post.storage {
                out.push(Change::Slot(
                    *addr,
                    U256::from_be_bytes(slot.0),
                    U256::from_be_bytes(val.0),
                ));
            }
        }
    }
    out
}

/// Fetch block `n`'s per-transaction state diffs.
pub async fn fetch_block_diff(p: &DynProvider, n: u64) -> anyhow::Result<Vec<TxDiff>> {
    let params = serde_json::json!([
        format!("{n:#x}"),
        { "tracer": "prestateTracer", "tracerConfig": { "diffMode": true } }
    ]);
    p.raw_request("debug_traceBlockByNumber".into(), params)
        .await
        .context("debug_traceBlockByNumber")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_mode_semantics() {
        let a = Address::repeat_byte(0xaa);
        let b = Address::repeat_byte(0xbb);
        let s1 = B256::with_last_byte(1);
        let s2 = B256::with_last_byte(2);
        let json = format!(
            r#"[{{"txHash":"0x{h}","result":{{
                "pre":{{"{a}":{{"balance":"0x10","storage":{{"{s1}":"{v5}","{s2}":"{v7}"}}}},
                        "{b}":{{"balance":"0x1"}}}},
                "post":{{"{a}":{{"balance":"0x20","nonce":3,"storage":{{"{s1}":"{v9}"}}}}}}
            }}}}]"#,
            h = "11".repeat(32),
            // Storage values are full 32-byte words in geth/dRPC output.
            v5 = B256::with_last_byte(5),
            v7 = B256::with_last_byte(7),
            v9 = B256::with_last_byte(9),
        );
        let block: Vec<TxDiff> = serde_json::from_str(&json).unwrap();
        let c = changes(&block);
        assert!(c.contains(&Change::Destroyed(b)));
        assert!(
            c.contains(&Change::Slot(a, U256::from(2), U256::ZERO)),
            "s2 cleared"
        );
        assert!(c.contains(&Change::Slot(a, U256::from(1), U256::from(9))));
        assert!(c.contains(&Change::Balance(a, U256::from(0x20))));
        assert!(c.contains(&Change::Nonce(a, 3)));
    }
}
