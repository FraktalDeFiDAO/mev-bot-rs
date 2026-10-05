//! Arbitrum One Fast Feed client (paid, optional): the Sequencer publishes each transaction as it
//! orders and executes it inside a PGA round, before the block closes
//! (docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/fast-feed, /use-fast-feed).
//!
//! Protocol facts used here (from those pages): authenticated WebSocket upgrade with
//! `Authorization: Bearer <api key>`, completed within 5 s; client sends only ping/pong; one JSON
//! `FeedMessage` per transaction carrying a tentative `block_number` and `tx_index`; no replay —
//! after a reconnect the stream continues from the head. So gaps can only be *detected*
//! (`GapDetector`), never refilled; the standard sequencer feed remains the source of record.
//!
//! The bot works without it: this is an optional latency upgrade, off by default
//! (`[pga.fast_feed] enabled`), and it needs a paid daily ticket (DECISIONS #59).

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use alloy::{
    consensus::TxEnvelope,
    eips::eip2718::Decodable2718,
    primitives::{B256, hex},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// One Fast Feed message (`version` 1).
#[derive(Debug, Clone, Deserialize)]
pub struct FeedMessage {
    pub version: u64,
    /// Unix ms when the Sequencer created the message.
    pub timestamp_ms: u64,
    /// PGA round within the block, from 1 (optional in the spec).
    #[serde(default)]
    pub pga_round: Option<u64>,
    pub transaction: FeedTransaction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedTransaction {
    /// Tentative: not guaranteed to be final.
    #[serde(deserialize_with = "num_or_hex")]
    pub block_number: u64,
    /// Position within the block so far.
    #[serde(deserialize_with = "num_or_hex")]
    pub tx_index: u64,
    /// Hex RLP of the signed transaction.
    pub raw_tx: String,
    pub tx_hash: String,
    /// Partial receipt (no block hash); kept raw.
    #[serde(default)]
    pub receipt: Option<serde_json::Value>,
}

/// The docs give field names and meaning, not JSON types: accept a number or a hex string.
fn num_or_hex<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        Num(u64),
        Str(String),
    }
    match N::deserialize(d)? {
        N::Num(n) => Ok(n),
        N::Str(s) => {
            let t = s.trim();
            let r = match t.strip_prefix("0x") {
                Some(h) => u64::from_str_radix(h, 16),
                None => t.parse(),
            };
            r.map_err(serde::de::Error::custom)
        }
    }
}

/// A decoded, ordered transaction from the Fast Feed.
#[derive(Debug, Clone)]
pub struct FastFeedTx {
    pub block_number: u64,
    pub tx_index: u64,
    pub pga_round: Option<u64>,
    pub hash: B256,
    pub tx: TxEnvelope,
    /// Sequencer timestamp (ms) and when this process received it.
    pub sequencer_ms: u64,
    pub received: Instant,
}

/// Parse and decode one text message.
pub fn decode(text: &str, received: Instant) -> Result<FastFeedTx> {
    let m: FeedMessage = serde_json::from_str(text).context("fast feed: bad message")?;
    let raw = hex::decode(m.transaction.raw_tx.trim()).context("fast feed: raw_tx not hex")?;
    let tx = TxEnvelope::decode_2718(&mut raw.as_slice()).context("fast feed: raw_tx not a tx")?;
    let hash = m
        .transaction
        .tx_hash
        .parse::<B256>()
        .unwrap_or(*tx.tx_hash());
    Ok(FastFeedTx {
        block_number: m.transaction.block_number,
        tx_index: m.transaction.tx_index,
        pga_round: m.pga_round,
        hash,
        tx,
        sequencer_ms: m.timestamp_ms,
        received,
    })
}

/// What a message says about stream continuity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuity {
    /// The next message expected.
    InOrder,
    /// First message after a (re)connect: nothing to compare with.
    Start,
    /// `missing` messages skipped within a block, or at the start of a new block.
    Gap { missing: u64 },
    /// Already seen (or older) position: duplicate or reordering.
    Stale,
}

/// Sequence-gap detection on (`block_number`, `tx_index`). Blocks advance by one and indexes
/// count from 0 within a block. A jump of more than one block is not counted as a gap: the docs
/// don't say whether blocks without user transactions produce messages.
#[derive(Debug, Default, Clone)]
pub struct GapDetector {
    last: Option<(u64, u64)>,
}

impl GapDetector {
    pub fn observe(&mut self, block: u64, index: u64) -> Continuity {
        let out = match self.last {
            None => Continuity::Start,
            Some((b, i)) if block == b && index == i + 1 => Continuity::InOrder,
            Some((b, i)) if block == b && index > i + 1 => Continuity::Gap {
                missing: index - i - 1,
            },
            Some((b, i)) if block < b || (block == b && index <= i) => return Continuity::Stale,
            // A later block: expect it to start at index 0.
            Some(_) if index == 0 => Continuity::InOrder,
            Some(_) => Continuity::Gap { missing: index },
        };
        self.last = Some((block, index));
        out
    }

    /// Forget the position (after a reconnect there is no replay: the next message starts over).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Exponential reconnect backoff with a cap; `reset` after a healthy connection.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            attempt: 0,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self
            .base
            .saturating_mul(1u32.checked_shl(self.attempt.min(16)).unwrap_or(u32::MAX))
            .min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Counters for the engine's minute summary.
#[derive(Debug, Default)]
pub struct FastFeedStats {
    pub msgs: AtomicU64,
    pub gaps: AtomicU64,
    pub missing: AtomicU64,
    pub reconnects: AtomicU64,
    pub decode_errors: AtomicU64,
    /// Sum of (receive time − sequencer timestamp) in ms, for a mean lag.
    pub lag_ms_total: AtomicU64,
}

/// WebSocket upgrade must complete within this (docs: 5 s or the relay drops it).
const HANDSHAKE: Duration = Duration::from_secs(5);
/// No message for this long: reconnect (the relay streams continuously while blocks are made).
const IDLE: Duration = Duration::from_secs(20);

/// Run until `out` closes: connect, stream, reconnect with backoff. `api_key` is presented as a
/// bearer token and never logged.
pub async fn run(
    url: String,
    api_key: String,
    out: mpsc::UnboundedSender<FastFeedTx>,
    stats: Arc<FastFeedStats>,
) {
    let mut backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(30));
    let mut gaps = GapDetector::default();
    while !out.is_closed() {
        match stream_once(&url, &api_key, &out, &stats, &mut gaps, &mut backoff).await {
            Ok(()) => return,
            Err(e) => {
                stats.reconnects.fetch_add(1, Relaxed);
                gaps.reset();
                let wait = backoff.next_delay();
                warn!(error = %format!("{e:#}"), wait_ms = wait.as_millis() as u64, "fast feed disconnected; reconnecting");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

async fn stream_once(
    url: &str,
    api_key: &str,
    out: &mpsc::UnboundedSender<FastFeedTx>,
    stats: &FastFeedStats,
    gaps: &mut GapDetector,
    backoff: &mut Backoff,
) -> Result<()> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};
    let mut req = url.into_client_request().context("fast feed: bad url")?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {api_key}")).context("fast feed: bad key")?,
    );
    let (mut ws, _) = tokio::time::timeout(HANDSHAKE, tokio_tungstenite::connect_async(req))
        .await
        .context("fast feed: handshake timed out")?
        .context("fast feed: connect failed")?;
    info!(url, "fast feed connected");
    backoff.reset();
    loop {
        let msg = tokio::time::timeout(IDLE, ws.next())
            .await
            .context("fast feed: idle")?
            .context("fast feed: stream ended")?
            .context("fast feed: read error")?;
        let received = Instant::now();
        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(p) => {
                ws.send(Message::Pong(p)).await.ok();
                continue;
            }
            Message::Close(_) => anyhow::bail!("fast feed closed by server"),
            _ => continue,
        };
        let tx = match decode(&text, received) {
            Ok(t) => t,
            Err(_) => {
                stats.decode_errors.fetch_add(1, Relaxed);
                continue;
            }
        };
        stats.msgs.fetch_add(1, Relaxed);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        stats
            .lag_ms_total
            .fetch_add(now_ms.saturating_sub(tx.sequencer_ms), Relaxed);
        if let Continuity::Gap { missing } = gaps.observe(tx.block_number, tx.tx_index) {
            stats.gaps.fetch_add(1, Relaxed);
            stats.missing.fetch_add(missing, Relaxed);
        }
        if out.send(tx).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real signed Arbitrum tx (EIP-1559) is not needed to test framing; this is a legacy tx
    /// RLP from the Ethereum test vectors (chain id 1), valid for decode_2718.
    const RAW: &str = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";

    fn msg(block: serde_json::Value, idx: serde_json::Value) -> String {
        serde_json::json!({
            "version": 1, "timestamp_ms": 1_790_000_000_000u64, "pga_round": 2,
            "transaction": { "block_number": block, "tx_index": idx, "raw_tx": format!("0x{RAW}"),
                             "tx_hash": "0x33469b22e9f636356c4160a87eb19df52b7412e8eac32a4a55ffe88ea8350788",
                             "receipt": { "status": "0x1" } }
        })
        .to_string()
    }

    #[test]
    fn decodes_a_feed_message_with_numeric_or_hex_positions() {
        let a = decode(&msg(512_000_000.into(), 3.into()), Instant::now()).unwrap();
        assert_eq!(
            (a.block_number, a.tx_index, a.pga_round),
            (512_000_000, 3, Some(2))
        );
        let b = decode(&msg("0x1e848000".into(), "0x3".into()), Instant::now()).unwrap();
        assert_eq!((b.block_number, b.tx_index), (0x1e84_8000, 3));
        assert!(decode("{\"version\":1}", Instant::now()).is_err());
    }

    #[test]
    fn gap_detector_flags_skips_and_stale_messages() {
        let mut g = GapDetector::default();
        assert_eq!(g.observe(10, 0), Continuity::Start);
        assert_eq!(g.observe(10, 1), Continuity::InOrder);
        assert_eq!(g.observe(10, 4), Continuity::Gap { missing: 2 });
        assert_eq!(g.observe(10, 4), Continuity::Stale);
        assert_eq!(g.observe(11, 0), Continuity::InOrder);
        assert_eq!(g.observe(12, 2), Continuity::Gap { missing: 2 });
        assert_eq!(g.observe(9, 0), Continuity::Stale);
        // After a reconnect (no replay) the next message is a fresh start, not a gap.
        g.reset();
        assert_eq!(g.observe(20, 7), Continuity::Start);
    }

    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut b = Backoff::new(Duration::from_millis(250), Duration::from_secs(2));
        let d: Vec<u64> = (0..6).map(|_| b.next_delay().as_millis() as u64).collect();
        assert_eq!(d, vec![250, 500, 1000, 2000, 2000, 2000]);
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_millis(250));
    }
}
