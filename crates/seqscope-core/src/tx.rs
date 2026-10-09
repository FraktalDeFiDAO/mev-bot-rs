//! A transaction as the rule engine sees it: plain data, serialisable, independent of the feed crate, so the same
//! engine runs on the live feed, on a recorded file and in tests.

use alloy::primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

/// One transaction from the sequencer feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tx {
    pub hash: B256,
    /// Sequencer message number; the L2 block is `seq + 22,207,817`.
    pub seq: u64,
    /// When the feed delivered it (unix milliseconds).
    pub t_ms: u64,
    pub from: Option<Address>,
    /// `None` for a contract creation.
    pub to: Option<Address>,
    /// EIP-2718 type byte: 0 legacy, 1 access list, 2 EIP-1559, 3 blob, 4 set-code; Arbitrum's own types (0x64
    /// deposit, 0x68 unsigned, 0x69 contract, 0x6a retry, 0x6c submit-retryable, 0x78 internal) when seen.
    pub tx_type: u8,
    pub value: U256,
    pub gas_limit: u64,
    pub max_fee: u128,
    pub tip: Option<u128>,
    pub nonce: u64,
    pub input: Bytes,
}

#[cfg(feature = "feed")]
impl Tx {
    /// From a feed transaction. `t_ms` is the delivery time reconstructed from the feed's monotonic timestamp.
    pub fn from_sequenced(st: &mev_sequencer::SequencedTx) -> Self {
        use alloy::consensus::{Transaction, transaction::SignerRecoverable};
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        Self {
            hash: st.hash,
            seq: st.seq,
            t_ms: now.saturating_sub(st.received.elapsed().as_millis() as u64),
            from: st.tx.recover_signer().ok(),
            to: st.to,
            tx_type: u8::from(st.tx.tx_type()),
            value: st.tx.value(),
            gas_limit: st.tx.gas_limit(),
            max_fee: st.tx.max_fee_per_gas(),
            tip: st.tx.max_priority_fee_per_gas(),
            nonce: st.tx.nonce(),
            input: st.input.clone(),
        }
    }
}
