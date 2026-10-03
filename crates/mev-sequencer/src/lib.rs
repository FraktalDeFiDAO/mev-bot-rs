//! Arbitrum sequencer feed client.
//!
//! The sequencer broadcasts every transaction as soon as it is ordered — before it is in a block
//! that RPC nodes serve — over a free, unmetered WebSocket (`wss://arb1.arbitrum.io/feed`). Reading
//! it gives a head start over block/log subscriptions and costs no RPC quota.
//!
//! Wire format (Nitro `broadcaster`/`arbos/parse_l2.go`): JSON `{"version":1,"messages":[...]}`;
//! each message holds an L1-incoming message whose `header.kind == 3` (L2 message) carries base64
//! `l2Msg` bytes. Their first byte is the L2 kind: `3` = batch (sequence of u64-BE length-prefixed
//! nested messages), `4` = one signed EIP-2718 transaction.

use std::time::{Duration, Instant};

use alloy::{
    consensus::{Transaction, TxEnvelope},
    eips::eip2718::Decodable2718,
    primitives::{Address, B256, Bytes},
};
use anyhow::{Context, Result};
use base64::Engine as _;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

/// L1 incoming message kind carrying sequencer-ordered L2 transactions.
const L1_KIND_L2_MESSAGE: u8 = 3;
const L2_KIND_BATCH: u8 = 3;
const L2_KIND_SIGNED_TX: u8 = 4;
/// Guard against malformed nesting / lengths.
const MAX_BATCH_DEPTH: usize = 16;
const MAX_L2_MESSAGE_SIZE: usize = 256 * 1024;

/// One transaction as ordered by the sequencer.
#[derive(Debug, Clone)]
pub struct SequencedTx {
    /// Sequencer message number (monotonic).
    pub seq: u64,
    /// When this process received it.
    pub received: Instant,
    pub hash: B256,
    pub to: Option<Address>,
    pub input: Bytes,
    pub tx: TxEnvelope,
}

impl SequencedTx {
    /// 4-byte function selector, if any.
    pub fn selector(&self) -> Option<[u8; 4]> {
        self.input.get(..4).map(|s| [s[0], s[1], s[2], s[3]])
    }
}

#[derive(Deserialize)]
struct Broadcast {
    #[serde(default)]
    messages: Vec<FeedMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedMessage {
    sequence_number: u64,
    message: MessageWithMeta,
}

#[derive(Deserialize)]
struct MessageWithMeta {
    message: L1IncomingMessage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct L1IncomingMessage {
    header: Header,
    #[serde(default)]
    l2_msg: String,
}

#[derive(Deserialize)]
struct Header {
    kind: u8,
}

/// Parse one broadcast JSON frame into (sequence number, decoded transactions).
pub fn parse_broadcast(frame: &str) -> Result<Vec<(u64, Vec<TxEnvelope>)>> {
    let b: Broadcast =
        serde_json::from_str(frame).context("feed frame is not a broadcast message")?;
    let mut out = Vec::with_capacity(b.messages.len());
    for m in b.messages {
        let msg = m.message.message;
        if msg.header.kind != L1_KIND_L2_MESSAGE || msg.l2_msg.is_empty() {
            continue;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&msg.l2_msg)
            .context("l2Msg base64")?;
        let mut txs = Vec::new();
        decode_l2_message(&bytes, 0, &mut txs);
        out.push((m.sequence_number, txs));
    }
    Ok(out)
}

/// Decode an L2 message (batch or signed tx) into transactions. Unknown kinds and malformed
/// entries are skipped rather than failing the whole message.
pub fn decode_l2_message(bytes: &[u8], depth: usize, out: &mut Vec<TxEnvelope>) {
    let Some((&kind, rest)) = bytes.split_first() else {
        return;
    };
    match kind {
        L2_KIND_SIGNED_TX => {
            if let Ok(tx) = TxEnvelope::decode_2718(&mut &rest[..]) {
                out.push(tx);
            }
        }
        L2_KIND_BATCH if depth < MAX_BATCH_DEPTH => {
            let mut rd = rest;
            while rd.len() >= 8 {
                let len = u64::from_be_bytes(rd[..8].try_into().unwrap()) as usize;
                rd = &rd[8..];
                if len > MAX_L2_MESSAGE_SIZE || len > rd.len() {
                    return;
                }
                decode_l2_message(&rd[..len], depth + 1, out);
                rd = &rd[len..];
            }
        }
        _ => {}
    }
}

/// Connect to the feed and forward every sequenced transaction. Reconnects with backoff forever;
/// returns only when the receiver is dropped.
pub async fn run(url: String, tx: mpsc::UnboundedSender<SequencedTx>) {
    let mut backoff = mev_rpc::Backoff::default();
    loop {
        let started = Instant::now();
        match stream_once(&url, &tx).await {
            Ok(()) => return, // receiver dropped
            Err(e) => {
                // A connection that stayed up a while was healthy: start backoff over.
                if started.elapsed() > Duration::from_secs(60) {
                    backoff.reset();
                }
                let d = backoff.next_delay();
                warn!(error = %format!("{e:#}"), retry_in_s = d.as_secs(), "sequencer feed disconnected");
                tokio::time::sleep(d).await;
            }
        }
    }
}

async fn stream_once(url: &str, tx: &mpsc::UnboundedSender<SequencedTx>) -> Result<()> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .context("feed connect")?;
    info!(url, "sequencer feed connected");
    while let Some(frame) = ws.next().await {
        let text = match frame.context("feed read")? {
            Message::Text(t) => t,
            Message::Close(_) => anyhow::bail!("feed closed by server"),
            _ => continue,
        };
        let received = Instant::now();
        let parsed = match parse_broadcast(&text) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "unparseable feed frame");
                continue;
            }
        };
        for (seq, txs) in parsed {
            for t in txs {
                let st = SequencedTx {
                    seq,
                    received,
                    hash: *t.tx_hash(),
                    to: t.to(),
                    input: t.input().clone(),
                    tx: t,
                };
                if tx.send(st).is_err() {
                    return Ok(());
                }
            }
        }
    }
    anyhow::bail!("feed stream ended")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        consensus::{SignableTransaction, TxEip1559},
        eips::eip2718::Encodable2718,
        network::TxSignerSync,
        primitives::{TxKind, U256, address},
        signers::local::PrivateKeySigner,
    };

    fn signed_tx(nonce: u64) -> TxEnvelope {
        let signer = PrivateKeySigner::random();
        let mut tx = TxEip1559 {
            chain_id: 42161,
            nonce,
            gas_limit: 21_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 0,
            to: TxKind::Call(address!("00000000000000000000000000000000000000aa")),
            value: U256::from(1),
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut tx).unwrap();
        TxEnvelope::Eip1559(tx.into_signed(sig))
    }

    fn signed_msg(t: &TxEnvelope) -> Vec<u8> {
        let mut b = vec![L2_KIND_SIGNED_TX];
        b.extend(t.encoded_2718());
        b
    }

    fn batch(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![L2_KIND_BATCH];
        for p in parts {
            b.extend((p.len() as u64).to_be_bytes());
            b.extend(p);
        }
        b
    }

    #[test]
    fn decodes_signed_tx() {
        let t = signed_tx(1);
        let mut out = vec![];
        decode_l2_message(&signed_msg(&t), 0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tx_hash(), t.tx_hash());
    }

    #[test]
    fn decodes_nested_batches() {
        let (a, b, c) = (signed_tx(1), signed_tx(2), signed_tx(3));
        let inner = batch(&[signed_msg(&b), signed_msg(&c)]);
        let outer = batch(&[signed_msg(&a), inner]);
        let mut out = vec![];
        decode_l2_message(&outer, 0, &mut out);
        let hashes: Vec<_> = out.iter().map(|t| *t.tx_hash()).collect();
        assert_eq!(hashes, vec![*a.tx_hash(), *b.tx_hash(), *c.tx_hash()]);
    }

    #[test]
    fn truncated_batch_is_skipped_not_panicking() {
        let mut bad = batch(&[signed_msg(&signed_tx(1))]);
        bad.truncate(bad.len() - 5);
        let mut out = vec![];
        decode_l2_message(&bad, 0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn parses_broadcast_json() {
        let t = signed_tx(7);
        let l2 = base64::engine::general_purpose::STANDARD.encode(signed_msg(&t));
        let frame = format!(
            r#"{{"version":1,"messages":[{{"sequenceNumber":42,"message":{{"message":{{"header":{{"kind":3,"sender":"0xa4b000000000000000000073657175656e636572","blockNumber":1,"timestamp":1,"requestId":null,"baseFeeL1":null}},"l2Msg":"{l2}"}},"delayedMessagesRead":1}},"signature":null}}]}}"#
        );
        let parsed = parse_broadcast(&frame).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, 42);
        assert_eq!(parsed[0].1[0].tx_hash(), t.tx_hash());
    }
}
