//! Base Flashblocks client: the OP Stack counterpart of the Arbitrum sequencer feed.
//!
//! Base's sequencer publishes each block in ~200 ms parts ("flashblocks") over a free WebSocket
//! (`wss://mainnet.flashblocks.base.org/ws`) before the 2 s block is sealed. Each message is
//! Brotli-compressed JSON `{payload_id, index, base?, diff: {transactions: [raw EIP-2718 hex],
//! ...}, metadata: {block_number, ...}}`; `index` restarts at 0 every block. Transactions are
//! forwarded as [`SequencedTx`] so the liquidation runner reacts to price-feed updates exactly as
//! it does on Arbitrum. Deposit transactions (type 0x7e) are not EIP-2718 envelopes alloy knows
//! and are skipped; they are system and bridge traffic, never oracle updates.

use std::{
    io::Read,
    sync::{Arc, atomic::Ordering::Relaxed},
    time::{Duration, Instant},
};

use alloy::{
    consensus::{Transaction, TxEnvelope},
    eips::eip2718::Decodable2718,
    primitives::Bytes,
};
use anyhow::{Context, Result};
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::{FEED_IDLE_TIMEOUT, FeedStats, SequencedTx};

/// Largest decompressed message accepted (a full flashblock is well under 1 MB).
const MAX_MESSAGE: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct Flashblock {
    index: u64,
    /// Block header fields; only the block's first flashblock (index 0) carries them.
    #[serde(default)]
    base: Option<Base>,
    diff: Diff,
    metadata: Metadata,
}

#[derive(Deserialize)]
struct Base {
    timestamp: alloy::primitives::U64,
}

#[derive(Deserialize)]
struct Diff {
    #[serde(default)]
    transactions: Vec<Bytes>,
}

#[derive(Deserialize)]
struct Metadata {
    block_number: u64,
}

/// One parsed flashblock: block number, index within the block, the block's timestamp (index 0
/// only), decodable transactions.
#[derive(Debug)]
pub struct Parsed {
    pub block: u64,
    pub index: u64,
    pub timestamp: Option<u64>,
    pub txs: Vec<TxEnvelope>,
}

/// Decode one WebSocket message (Brotli-compressed or plain JSON).
pub fn parse_flashblock(raw: &[u8]) -> Result<Parsed> {
    let json = if raw.first() == Some(&b'{') {
        raw.to_vec()
    } else {
        let mut out = Vec::new();
        brotli_decompressor::Decompressor::new(raw, 4096)
            .take(MAX_MESSAGE)
            .read_to_end(&mut out)
            .context("brotli")?;
        out
    };
    let fb: Flashblock = serde_json::from_slice(&json).context("flashblock json")?;
    let txs = fb
        .diff
        .transactions
        .iter()
        .filter_map(|b| TxEnvelope::decode_2718(&mut b.as_ref()).ok())
        .collect();
    Ok(Parsed {
        block: fb.metadata.block_number,
        index: fb.index,
        timestamp: fb.base.map(|b| b.timestamp.to::<u64>()),
        txs,
    })
}

/// Sequence number for a flashblock: monotonic across blocks and parts.
pub fn seq_of(block: u64, index: u64) -> u64 {
    (block << 8) | index.min(255)
}

/// Connect and forward every flashblock transaction; reconnects with backoff forever and returns
/// only when the receiver is dropped.
pub async fn run_tracked(
    url: String,
    tx: mpsc::UnboundedSender<SequencedTx>,
    stats: Arc<FeedStats>,
) {
    let mut backoff = mev_rpc::Backoff::default();
    loop {
        let started = Instant::now();
        match stream_once(&url, &tx, &stats).await {
            Ok(()) => return,
            Err(e) => {
                stats.reconnects.fetch_add(1, Relaxed);
                stats
                    .confidence_epoch
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
                if started.elapsed() > Duration::from_secs(60) {
                    backoff.reset();
                }
                let d = backoff.next_delay();
                warn!(error = %format!("{e:#}"), retry_in_s = d.as_secs(), "flashblocks feed disconnected");
                tokio::time::sleep(d).await;
            }
        }
    }
}

async fn stream_once(
    url: &str,
    tx: &mpsc::UnboundedSender<SequencedTx>,
    stats: &FeedStats,
) -> Result<()> {
    let (mut ws, _) =
        tokio::time::timeout(FEED_IDLE_TIMEOUT, tokio_tungstenite::connect_async(url))
            .await
            .context("flashblocks connect timed out")?
            .context("flashblocks connect")?;
    info!(url, "flashblocks feed connected");
    let mut last_seq: Option<u64> = None;
    // (block, timestamp) from the current block's first flashblock.
    let mut block_ts: Option<(u64, u64)> = None;
    loop {
        let frame = tokio::time::timeout(FEED_IDLE_TIMEOUT, ws.next())
            .await
            .context("flashblocks stalled")?;
        let Some(frame) = frame else {
            anyhow::bail!("flashblocks stream ended");
        };
        let raw: Vec<u8> = match frame.context("flashblocks read")? {
            Message::Binary(b) => b.to_vec(),
            Message::Text(t) => t.as_bytes().to_vec(),
            Message::Close(_) => anyhow::bail!("flashblocks closed by server"),
            _ => continue,
        };
        let received = Instant::now();
        let p = match parse_flashblock(&raw) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %format!("{e:#}"), "unparseable flashblock");
                continue;
            }
        };
        let seq = seq_of(p.block, p.index);
        match last_seq {
            Some(l) if seq <= l => {
                stats.stale.fetch_add(1, Relaxed);
                continue;
            }
            // A new block must start at index 0, and parts of one block come in order.
            Some(l)
                if (p.index > 0 && seq != l + 1) || (p.index == 0 && p.block != (l >> 8) + 1) =>
            {
                stats.gaps.fetch_add(1, Relaxed);
                stats
                    .confidence_epoch
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
            }
            _ => {}
        }
        last_seq = Some(seq);
        stats.messages.fetch_add(1, Relaxed);
        if let Some(ts) = p.timestamp {
            block_ts = Some((p.block, ts));
        }
        let block_timestamp = block_ts.filter(|(b, _)| *b == p.block).map(|(_, ts)| ts);
        for t in p.txs {
            let st = SequencedTx {
                seq,
                received,
                block_timestamp,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Base's live feed: the first flashblock of a block carries its timestamp, and Base's
    /// timestamps are genesis (1686789347) + 2 s x block number.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_first_flashblock_carries_the_block_timestamp() {
        let (mut ws, _) = tokio_tungstenite::connect_async("wss://mainnet.flashblocks.base.org/ws")
            .await
            .unwrap();
        loop {
            let raw = match ws.next().await.unwrap().unwrap() {
                Message::Binary(b) => b.to_vec(),
                Message::Text(t) => t.as_bytes().to_vec(),
                _ => continue,
            };
            let p = parse_flashblock(&raw).unwrap();
            if p.index == 0 {
                assert_eq!(p.timestamp, Some(1_686_789_347 + 2 * p.block));
                return;
            }
            assert_eq!(p.timestamp, None, "only index 0 carries the header");
        }
    }

    #[test]
    fn a_recorded_base_flashblock_decodes() {
        // Base mainnet block 52668260, part 4, trimmed to its first 4 transactions (2026-10-16).
        let raw = include_bytes!("../tests/fixtures/flashblock.json.br");
        let p = parse_flashblock(raw).unwrap();
        assert_eq!((p.block, p.index), (52_668_260, 4));
        assert_eq!(p.txs.len(), 4);
        assert!(p.txs.iter().all(|t| t.to().is_some()));
    }

    #[test]
    fn plain_json_is_accepted_and_deposits_are_skipped() {
        // A deposit (0x7e) is not an envelope alloy decodes: dropped, not an error.
        let json = br#"{"payload_id":"0x1","index":0,"diff":{"transactions":["0x7ef8"]},"metadata":{"block_number":7}}"#;
        let p = parse_flashblock(json).unwrap();
        assert_eq!((p.block, p.index, p.txs.len()), (7, 0, 0));
    }

    #[test]
    fn sequence_numbers_order_blocks_then_parts() {
        assert!(seq_of(10, 9) < seq_of(11, 0));
        assert_eq!(seq_of(10, 1), seq_of(10, 0) + 1);
    }
}
