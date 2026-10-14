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

/// Arbitrum One's Nitro genesis block: L2 block number = feed sequence number + this.
pub const ARB_ONE_GENESIS_BLOCK: u64 = 22_207_817;

impl SequencedTx {
    /// The L2 block this tx was sequenced into (Arbitrum One).
    pub fn block(&self) -> u64 {
        self.seq + ARB_ONE_GENESIS_BLOCK
    }

    /// 4-byte function selector, if any.
    pub fn selector(&self) -> Option<[u8; 4]> {
        self.input.get(..4).map(|s| [s[0], s[1], s[2], s[3]])
    }

    /// The priority fee (wei/gas) this contract call bids, if it bids one: an EIP-1559 call with
    /// a non-zero `maxPriorityFeePerGas` (capped by `maxFeePerGas`, an upper bound on what it
    /// pays). Plain transfers and legacy txs (whose tip needs the base fee) give `None`. Under PGA
    /// this is what competing bots pay for ordering.
    pub fn bid_tip_wei(&self) -> Option<u128> {
        use alloy::consensus::Transaction;
        if self.input.len() < 4 {
            return None;
        }
        let prio = self.tx.max_priority_fee_per_gas()?;
        let tip = prio.min(self.tx.max_fee_per_gas());
        (tip > 0).then_some(tip)
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

/// Parse one broadcast JSON frame into (sequence number, decoded transactions), one entry per message in the
/// frame, in order. Messages without L2 transactions come back with an empty list. A frame with no messages (a
/// `confirmedSequenceNumberMessage`) gives an empty result.
pub fn parse_broadcast(frame: &str) -> Result<Vec<(u64, Vec<TxEnvelope>)>> {
    let b: Broadcast =
        serde_json::from_str(frame).context("feed frame is not a broadcast message")?;
    let mut out = Vec::with_capacity(b.messages.len());
    for m in b.messages {
        let msg = m.message.message;
        // Messages that carry no L2 transactions (deposits, retryables, end-of-block markers) still consume a
        // sequence number: report them with no transactions so continuity can be checked.
        if msg.header.kind != L1_KIND_L2_MESSAGE || msg.l2_msg.is_empty() {
            out.push((m.sequence_number, Vec::new()));
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

/// When each recent feed transaction arrived, by hash: the reference clock for race telemetry
/// (the feed shows every transaction in sequencer order, ours and competitors' alike).
#[derive(Debug, Default)]
pub struct FeedLog {
    seen: std::collections::HashMap<B256, (Instant, u64)>,
    order: std::collections::VecDeque<(Instant, B256)>,
}

impl FeedLog {
    /// How long arrivals are kept (attempts land and are analysed well within this).
    pub const KEEP: Duration = Duration::from_secs(120);

    pub fn record(&mut self, st: &SequencedTx) {
        self.seen.insert(st.hash, (st.received, st.seq));
        self.order.push_back((st.received, st.hash));
        while let Some((t, h)) = self.order.front().copied() {
            if t.elapsed() <= Self::KEEP {
                break;
            }
            self.order.pop_front();
            self.seen.remove(&h);
        }
    }

    /// Arrival time and sequence number of `hash`, if seen in the last `KEEP`.
    pub fn get(&self, hash: &B256) -> Option<(Instant, u64)> {
        self.seen.get(hash).copied()
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// What observing one sequence number told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeqObservation {
    /// The first message seen.
    First,
    /// Exactly the previous number plus one.
    InOrder,
    /// At or below the highest number accepted: a duplicate, a replay after reconnecting, or a late delivery. Drop it.
    Stale { last: u64 },
    /// Messages were skipped. Whatever they carried is missing; state built across the gap is suspect.
    Gap {
        missing: std::ops::RangeInclusive<u64>,
    },
}

/// Checks that the standard feed's sequence numbers arrive in order without holes. The semantics follow Nitro's
/// feed: one message per L2 block, numbers increase by one, and a client may ask to resume from a number after
/// reconnecting (`Arbitrum-Requested-Sequence-Number`).
#[derive(Debug, Default, Clone)]
pub struct SequenceTracker {
    last: Option<u64>,
}

impl SequenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Highest accepted sequence number.
    pub fn last(&self) -> Option<u64> {
        self.last
    }

    /// The number to ask for when reconnecting.
    pub fn resume_point(&self) -> Option<u64> {
        self.last.map(|l| l + 1)
    }

    /// Observe one number. `First`, `InOrder` and `Gap` advance the tracker; `Stale` does not.
    pub fn observe(&mut self, seq: u64) -> SeqObservation {
        match self.last {
            None => {
                self.last = Some(seq);
                SeqObservation::First
            }
            Some(last) if seq <= last => SeqObservation::Stale { last },
            Some(last) if seq == last + 1 => {
                self.last = Some(seq);
                SeqObservation::InOrder
            }
            Some(last) => {
                self.last = Some(seq);
                SeqObservation::Gap {
                    missing: (last + 1)..=(seq - 1),
                }
            }
        }
    }
}

/// Counters for the feed connection since the last summary.
#[derive(Debug, Default)]
pub struct FeedStats {
    /// Never reset by summaries. Any continuity loss invalidates in-flight decisions.
    pub confidence_epoch: std::sync::atomic::AtomicU64,
    pub messages: std::sync::atomic::AtomicU64,
    /// Times a hole was seen in the sequence numbers.
    pub gaps: std::sync::atomic::AtomicU64,
    /// Sequence numbers inside those holes.
    pub missed: std::sync::atomic::AtomicU64,
    /// Messages dropped as already seen.
    pub stale: std::sync::atomic::AtomicU64,
    pub reconnects: std::sync::atomic::AtomicU64,
    /// Feed transactions a consumer's bounded queue refused because it fell behind (counted once
    /// per consumer). The consumer keeps running; only the stale backlog is shed.
    pub dropped: std::sync::atomic::AtomicU64,
}

impl FeedStats {
    /// One line since the last call; resets the counters.
    pub fn take_summary(&self) -> String {
        use std::sync::atomic::Ordering::Relaxed;
        let t = |a: &std::sync::atomic::AtomicU64| a.swap(0, Relaxed);
        format!(
            "messages={} gaps={} missed={} stale={} reconnects={} dropped={}",
            t(&self.messages),
            t(&self.gaps),
            t(&self.missed),
            t(&self.stale),
            t(&self.reconnects),
            t(&self.dropped)
        )
    }
}

/// The feed sends several frames a second; this long without one means the connection is dead
/// even if the socket is still open (seen live: a reconnect that never delivered a frame).
pub const FEED_IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Connect to the feed and forward every sequenced transaction. Reconnects with backoff forever, resuming from the
/// last sequence number seen; returns only when the receiver is dropped.
pub async fn run(url: String, tx: mpsc::UnboundedSender<SequencedTx>) {
    run_tracked(url, tx, std::sync::Arc::new(FeedStats::default())).await
}

/// `run`, counting continuity into `stats` (gaps, missed numbers, replays, reconnects).
pub async fn run_tracked(
    url: String,
    tx: mpsc::UnboundedSender<SequencedTx>,
    stats: std::sync::Arc<FeedStats>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut backoff = mev_rpc::Backoff::default();
    let mut tracker = SequenceTracker::new();
    loop {
        let started = Instant::now();
        match stream_once(&url, &tx, FEED_IDLE_TIMEOUT, &mut tracker, &stats).await {
            Ok(()) => return, // receiver dropped
            Err(e) => {
                stats.reconnects.fetch_add(1, Relaxed);
                stats
                    .confidence_epoch
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
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

/// Header a client sends to ask the feed to start from a given sequence number.
const RESUME_HEADER: &str = "Arbitrum-Requested-Sequence-Number";

async fn stream_once(
    url: &str,
    tx: &mpsc::UnboundedSender<SequencedTx>,
    idle: Duration,
    tracker: &mut SequenceTracker,
    stats: &FeedStats,
) -> Result<()> {
    use std::sync::atomic::Ordering::Relaxed;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};
    let mut request = url.into_client_request().context("feed url")?;
    // After a reconnect, ask for what was sequenced while we were away. If the feed no longer has it, the tracker
    // reports the hole below instead of the loss going unnoticed.
    if let Some(from) = tracker.resume_point() {
        request.headers_mut().insert(
            RESUME_HEADER,
            HeaderValue::from_str(&from.to_string()).context("resume header")?,
        );
    }
    let (mut ws, _) = tokio::time::timeout(idle, tokio_tungstenite::connect_async(request))
        .await
        .context("feed connect timed out")?
        .context("feed connect")?;
    info!(url, resume_from = ?tracker.resume_point(), "sequencer feed connected");
    // Only data frames count as liveness: pings alone don't mean transactions are flowing.
    let mut last_data = tokio::time::Instant::now();
    loop {
        let Some(frame) = tokio::time::timeout_at(last_data + idle, ws.next())
            .await
            .with_context(|| format!("feed stalled: no frame for {}s", idle.as_secs()))?
        else {
            break;
        };
        let text = match frame.context("feed read")? {
            Message::Text(t) => t,
            Message::Close(_) => anyhow::bail!("feed closed by server"),
            _ => continue,
        };
        last_data = tokio::time::Instant::now();
        let received = Instant::now();
        let parsed = match parse_broadcast(&text) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "unparseable feed frame");
                continue;
            }
        };
        for (seq, txs) in parsed {
            match tracker.observe(seq) {
                SeqObservation::Stale { .. } => {
                    stats.stale.fetch_add(1, Relaxed);
                    continue;
                }
                SeqObservation::Gap { missing } => {
                    stats.gaps.fetch_add(1, Relaxed);
                    stats
                        .confidence_epoch
                        .fetch_add(1, std::sync::atomic::Ordering::Release);
                    stats
                        .missed
                        .fetch_add(missing.end() - missing.start() + 1, Relaxed);
                    warn!(
                        ?missing,
                        "sequencer feed skipped messages: transactions in them are missing"
                    );
                }
                SeqObservation::First | SeqObservation::InOrder => {}
            }
            stats.messages.fetch_add(1, Relaxed);
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

    fn sample_tx(seq: u64) -> SequencedTx {
        let t = signed_tx(seq);
        SequencedTx {
            seq,
            received: Instant::now(),
            hash: *t.tx_hash(),
            to: t.to(),
            input: t.input().clone(),
            tx: t,
        }
    }

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

    #[test]
    fn bid_tip_is_the_priority_fee_of_contract_calls_only() {
        let signer = PrivateKeySigner::random();
        let mk = |prio: u128, max: u128, input: Vec<u8>| {
            let mut tx = TxEip1559 {
                chain_id: 42161,
                nonce: 0,
                gas_limit: 300_000,
                max_fee_per_gas: max,
                max_priority_fee_per_gas: prio,
                to: TxKind::Call(address!("00000000000000000000000000000000000000aa")),
                input: input.into(),
                ..Default::default()
            };
            let sig = signer.sign_transaction_sync(&mut tx).unwrap();
            let env = TxEnvelope::Eip1559(tx.into_signed(sig));
            SequencedTx {
                seq: 1,
                received: Instant::now(),
                hash: *env.tx_hash(),
                to: env.to(),
                input: env.input().clone(),
                tx: env,
            }
        };
        let call = vec![1, 2, 3, 4, 5];
        assert_eq!(
            mk(3_000_000_000, 5_000_000_000, call.clone()).bid_tip_wei(),
            Some(3_000_000_000)
        );
        // Capped by maxFeePerGas.
        assert_eq!(mk(9, 4, call.clone()).bid_tip_wei(), Some(4));
        // No tip, or a plain transfer: no bid.
        assert_eq!(mk(0, 5, call).bid_tip_wei(), None);
        assert_eq!(mk(7, 9, vec![]).bid_tip_wei(), None);
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
    fn feed_log_remembers_arrivals_and_forgets_old_ones() {
        let mut log = FeedLog::default();
        let mut st = sample_tx(1);
        log.record(&st);
        assert_eq!(log.get(&st.hash).map(|(_, seq)| seq), Some(1));
        // An arrival older than KEEP is pruned when the next one is recorded.
        st.received = Instant::now() - FeedLog::KEEP - Duration::from_secs(1);
        let old = st.hash;
        let mut log = FeedLog::default();
        log.record(&st);
        log.record(&sample_tx(2));
        assert!(log.get(&old).is_none());
        assert_eq!(log.len(), 1);
    }

    /// A server that accepts the WebSocket, then only pings: the reader must give up, not hang.
    #[tokio::test]
    async fn silent_feed_times_out_instead_of_hanging() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            use futures::SinkExt;
            loop {
                if ws.send(Message::Ping(vec![].into())).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let idle = Duration::from_millis(300);
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            stream_once(
                &url,
                &tx,
                idle,
                &mut SequenceTracker::new(),
                &FeedStats::default(),
            ),
        )
        .await
        .expect("stream_once hung on a silent feed");
        let err = format!("{:#}", res.unwrap_err());
        assert!(err.contains("feed stalled"), "{err}");
    }

    /// Adapted from Nitro's `ExampleBroadcastMessage_broadcastfeedmessageWithBlockHashAndBlockMetadata` (the 32-byte
    /// request id and block hash are shortened; this parser ignores them). Kind 0 is not an L2 message, and its confirmed-sequence-number frame (no messages at all).
    const NITRO_GOLDEN: &str = r#"{"version":1,"messages":[{"sequenceNumber":12345,"message":{"message":{"header":{"kind":0,"sender":"0x0000000000000000000000000000000000000000","blockNumber":0,"timestamp":0,"requestId":null,"baseFeeL1":0},"l2Msg":"3q2+7w=="},"delayedMessagesRead":3333},"blockHash":"0xff","signatureV2":null,"blockMetadata":"AAI="}]}"#;
    const NITRO_CONFIRMED: &str =
        r#"{"version":1,"confirmedSequenceNumberMessage":{"sequenceNumber":1234}}"#;

    #[test]
    fn nitros_own_example_frames_parse_and_keep_their_sequence_numbers() {
        // A message that is not an L2 message still reports its sequence number, with no transactions.
        let parsed = parse_broadcast(NITRO_GOLDEN).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!((parsed[0].0, parsed[0].1.len()), (12345, 0));
        // A confirmed-sequence-number frame has no messages.
        assert!(parse_broadcast(NITRO_CONFIRMED).unwrap().is_empty());
    }

    #[test]
    fn the_tracker_reports_gaps_replays_and_the_resume_point() {
        let mut t = SequenceTracker::new();
        assert_eq!(t.resume_point(), None);
        assert_eq!(t.observe(100), SeqObservation::First);
        assert_eq!(t.observe(101), SeqObservation::InOrder);
        assert_eq!(t.observe(101), SeqObservation::Stale { last: 101 });
        assert_eq!(t.observe(99), SeqObservation::Stale { last: 101 });
        assert_eq!(t.last(), Some(101), "stale numbers do not move the tracker");
        assert_eq!(t.observe(105), SeqObservation::Gap { missing: 102..=104 });
        assert_eq!(t.resume_point(), Some(106));
    }

    fn l1_frame(seq: u64, kind: u8) -> String {
        format!(
            r#"{{"version":1,"messages":[{{"sequenceNumber":{seq},"message":{{"message":{{"header":{{"kind":{kind},"sender":"0x0000000000000000000000000000000000000000","blockNumber":1,"timestamp":1,"requestId":null,"baseFeeL1":null}},"l2Msg":""}},"delayedMessagesRead":1}},"signature":null}}]}}"#
        )
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)] // tungstenite's handshake callback signature
    async fn a_mock_feed_with_a_hole_and_a_replay_is_counted_and_the_reconnect_asks_to_resume() {
        use std::sync::{Arc, Mutex};
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let asked: Arc<Mutex<Vec<Option<String>>>> = Default::default();
        let asked_by_server = asked.clone();
        tokio::spawn(async move {
            use futures::SinkExt;
            // First connection: 10, 11, a hole (12 missing), 13, then 13 again (a replay), then it closes.
            // Second connection: 14.
            for frames in [vec![10u64, 11, 13, 13], vec![14]] {
                let (sock, _) = listener.accept().await.unwrap();
                let seen = asked_by_server.clone();
                let mut ws = tokio_tungstenite::accept_hdr_async(
                    sock,
                    move |req: &Request, resp: Response| {
                        let h = req
                            .headers()
                            .get(RESUME_HEADER)
                            .map(|v| v.to_str().unwrap().to_string());
                        seen.lock().unwrap().push(h);
                        Ok(resp)
                    },
                )
                .await
                .unwrap();
                for seq in frames {
                    ws.send(Message::Text(l1_frame(seq, 13).into()))
                        .await
                        .unwrap();
                }
                ws.close(None).await.ok();
            }
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let (mut tracker, stats) = (SequenceTracker::new(), FeedStats::default());
        let idle = Duration::from_secs(5);
        assert!(
            stream_once(&url, &tx, idle, &mut tracker, &stats)
                .await
                .is_err(),
            "the server closed"
        );
        assert!(
            stream_once(&url, &tx, idle, &mut tracker, &stats)
                .await
                .is_err()
        );
        use std::sync::atomic::Ordering::Relaxed;
        assert_eq!(stats.gaps.load(Relaxed), 1);
        assert_eq!(stats.confidence_epoch.load(Relaxed), 1);
        assert_eq!(stats.missed.load(Relaxed), 1, "only number 12 was skipped");
        assert_eq!(stats.stale.load(Relaxed), 1, "the replayed 13");
        assert_eq!(
            stats.messages.load(Relaxed),
            4,
            "10, 11, 13 and 14 counted; the replay is not"
        );
        stats.take_summary();
        assert_eq!(
            stats.confidence_epoch.load(Relaxed),
            1,
            "summaries cannot restore confidence"
        );
        assert_eq!(tracker.last(), Some(14));
        // The first connection asked for nothing; the second asked to start right after the last number seen (13).
        assert_eq!(*asked.lock().unwrap(), vec![None, Some("14".to_string())]);
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

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Malformed feed data must be skipped, never panic the reader.
        #[test]
        fn decoder_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut out = vec![];
            decode_l2_message(&bytes, 0, &mut out);
        }

        #[test]
        fn broadcast_parser_never_panics(s in ".{0,300}") {
            let _ = parse_broadcast(&s);
        }
    }
}
