//! Pre-block simulation of sequencer-feed transactions.
//!
//! The sequencer feed shows a transaction ~a block before its logs exist. Running it through
//! `eth_simulateV1` (as its real sender, against the latest state) returns the logs it will emit;
//! pool `Swap`/`Sync` events carry absolute post-trade prices/reserves, so they can update the pool
//! store early. If the real block differs, its own logs overwrite the state a moment later.
//!
//! Measured from the bot's server (2026-10-04): PublicNode answers in ~16 ms on a warm
//! connection (dRPC ~70 ms, arb1 ~120 ms), with the same head block as arb1.

use alloy::{
    consensus::{Transaction, TxEnvelope, transaction::SignerRecoverable},
    primitives::{Address, B256, Bytes},
    providers::{DynProvider, Provider},
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::{IPancakeV3Pool, IUniswapV2Pair, IUniswapV3Pool};
use mev_sequencer::SequencedTx;
use serde::Deserialize;

/// Selectors of calls that can't move a pool: ERC-20 transfer/approve/transferFrom, Chainlink
/// forwarder reports, OCR transmits.
const INERT_SELECTORS: [[u8; 4]; 6] = [
    [0xa9, 0x05, 0x9c, 0xbb], // transfer
    [0x09, 0x5e, 0xa7, 0xb3], // approve
    [0x23, 0xb8, 0x72, 0xdd], // transferFrom
    [0x6f, 0xad, 0xcf, 0x72], // forward (Chainlink)
    [0xb1, 0xdc, 0x65, 0xa4], // transmit (OCR2)
    [0xc9, 0x80, 0x75, 0x39], // transmit (OCR1)
];

/// Worth simulating: a contract call that isn't obviously inert. Cheap (no I/O).
pub fn is_candidate(st: &SequencedTx) -> bool {
    st.to.is_some() && st.input.len() >= 4 && !INERT_SELECTORS.iter().any(|s| st.input[..4] == *s)
}

/// Topic0 of the pool events the engine consumes (V2 `Sync`, V3/Algebra `Swap`, PancakeV3 `Swap`).
pub fn is_pool_event(topic0: &B256) -> bool {
    *topic0 == IUniswapV2Pair::Sync::SIGNATURE_HASH
        || *topic0 == IUniswapV3Pool::Swap::SIGNATURE_HASH
        || *topic0 == IPancakeV3Pool::Swap::SIGNATURE_HASH
}

/// A log the simulated transaction would emit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SimLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
}

#[derive(Debug, Deserialize)]
struct SimBlock {
    calls: Vec<SimCall>,
}

#[derive(Debug, Deserialize)]
struct SimCall {
    status: alloy::primitives::U64,
    #[serde(default)]
    logs: Vec<SimLog>,
}

/// Run `tx` as its sender on top of the latest block; the logs it emits (empty if it reverts).
pub async fn simulate_logs(p: &DynProvider, tx: &TxEnvelope) -> Result<Vec<SimLog>> {
    let from = tx.recover_signer().context("recover sender")?;
    let call = serde_json::json!({
        "from": from,
        "to": tx.to(),
        "input": tx.input(),
        "value": tx.value(),
        "gas": format!("{:#x}", tx.gas_limit()),
    });
    // validation=false: no fee/nonce checks, so a tx priced for the next block still runs.
    let params = serde_json::json!([
        { "blockStateCalls": [{ "calls": [call] }], "validation": false },
        "latest"
    ]);
    let blocks: Vec<SimBlock> = p
        .raw_request("eth_simulateV1".into(), params)
        .await
        .context("eth_simulateV1")?;
    Ok(parse_result(blocks))
}

fn parse_result(blocks: Vec<SimBlock>) -> Vec<SimLog> {
    blocks
        .into_iter()
        .flat_map(|b| b.calls)
        .filter(|c| c.status.to::<u64>() == 1)
        .flat_map(|c| c.logs)
        .collect()
}

/// Pool events among `logs`.
pub fn pool_events(logs: &[SimLog]) -> impl Iterator<Item = &SimLog> {
    logs.iter()
        .filter(|l| l.topics.first().is_some_and(is_pool_event))
}

/// `SimLog` as the RPC log type the pool store consumes.
pub fn to_rpc_log(l: &SimLog) -> alloy::rpc::types::Log {
    alloy::rpc::types::Log {
        inner: alloy::primitives::Log::new_unchecked(l.address, l.topics.clone(), l.data.clone()),
        ..Default::default()
    }
}

/// Pool logs a feed transaction is about to emit, and when the feed delivered it.
#[derive(Debug, Clone)]
pub struct Presimulated {
    pub tx: B256,
    pub received: std::time::Instant,
    pub logs: Vec<alloy::rpc::types::Log>,
}

/// Simulate candidate feed transactions (at most `max_in_flight` at once; extras are skipped,
/// their block logs still arrive later) and forward the pool logs of those that move a pool.
/// Runs until the feed or the receiver closes.
pub async fn run(
    mut feed: tokio::sync::mpsc::UnboundedReceiver<SequencedTx>,
    p: DynProvider,
    max_in_flight: usize,
    timeout: std::time::Duration,
    out: tokio::sync::mpsc::UnboundedSender<Presimulated>,
) {
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(max_in_flight.max(1)));
    while let Some(st) = feed.recv().await {
        if out.is_closed() {
            return;
        }
        if !is_candidate(&st) {
            continue;
        }
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let (p, out) = (p.clone(), out.clone());
        tokio::spawn(async move {
            let res = tokio::time::timeout(timeout, simulate_logs(&p, &st.tx)).await;
            drop(permit);
            let Ok(Ok(logs)) = res else { return };
            let logs: Vec<_> = pool_events(&logs).map(to_rpc_log).collect();
            if !logs.is_empty() {
                let _ = out.send(Presimulated {
                    tx: st.hash,
                    received: st.received,
                    logs,
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        consensus::{SignableTransaction, TxEip1559},
        network::TxSignerSync,
        primitives::TxKind,
        signers::local::PrivateKeySigner,
    };
    use std::time::Instant;

    fn seq(to: Option<Address>, input: Vec<u8>) -> SequencedTx {
        let signer = PrivateKeySigner::random();
        let mut t = TxEip1559 {
            chain_id: 42161,
            to: to.map(TxKind::Call).unwrap_or(TxKind::Create),
            input: input.clone().into(),
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut t).unwrap();
        let env = TxEnvelope::Eip1559(t.into_signed(sig));
        SequencedTx {
            seq: 1,
            received: Instant::now(),
            hash: *env.tx_hash(),
            to,
            input: input.into(),
            tx: env,
        }
    }

    #[test]
    fn candidates_are_non_inert_contract_calls() {
        let c = Some(Address::with_last_byte(1));
        assert!(is_candidate(&seq(c, vec![0x38, 0xed, 0x17, 0x39, 0]))); // a router swap
        assert!(!is_candidate(&seq(c, vec![0xa9, 0x05, 0x9c, 0xbb, 0]))); // ERC-20 transfer
        assert!(!is_candidate(&seq(c, vec![0x6f, 0xad, 0xcf, 0x72]))); // oracle report
        assert!(!is_candidate(&seq(c, vec![]))); // ETH transfer
        assert!(!is_candidate(&seq(None, vec![1, 2, 3, 4]))); // deployment
    }

    #[test]
    fn parses_simulate_result_and_drops_reverted_calls() {
        let sync = IUniswapV2Pair::Sync::SIGNATURE_HASH;
        let json = format!(
            r#"[{{"calls":[
                {{"status":"0x1","logs":[{{"address":"0x0000000000000000000000000000000000000001","topics":["{sync:?}"],"data":"0x01"}},
                                         {{"address":"0x0000000000000000000000000000000000000002","topics":["0x{}"],"data":"0x"}}]}},
                {{"status":"0x0","logs":[{{"address":"0x0000000000000000000000000000000000000003","topics":["{sync:?}"],"data":"0x"}}]}}
            ]}}]"#,
            "11".repeat(32)
        );
        let logs = parse_result(serde_json::from_str(&json).unwrap());
        assert_eq!(logs.len(), 2, "the reverted call's logs are dropped");
        let pools: Vec<Address> = pool_events(&logs).map(|l| l.address).collect();
        assert_eq!(pools, vec![Address::with_last_byte(1)]);
    }

    #[test]
    fn rpc_log_keeps_address_topics_and_data() {
        let l = SimLog {
            address: Address::with_last_byte(7),
            topics: vec![IUniswapV2Pair::Sync::SIGNATURE_HASH],
            data: vec![1, 2, 3].into(),
        };
        let r = to_rpc_log(&l);
        assert_eq!(r.address(), l.address);
        assert_eq!(r.topics(), l.topics.as_slice());
        assert_eq!(r.data().data, l.data);
    }
}
