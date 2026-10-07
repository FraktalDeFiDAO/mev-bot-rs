//! The Fast Feed client against a local server that behaves as docs.arbitrum.io describes it:
//! `Authorization: Bearer <raw key>` on the WebSocket upgrade, one JSON `FeedMessage` per transaction
//! inside a **binary** frame. No network, no ticket.

use std::{sync::Arc, time::Duration};

use futures::SinkExt;
use mev_ingest::fast_feed::{self, FastFeedStats};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{ErrorResponse, Request, Response},
};

/// A legacy tx from the Ethereum test vectors; decodes with `decode_2718`.
const RAW: &str = concat!(
    "f86c098504a817c8008252089435353535353535",
    "35353535353535353535353535880de0b6b3a764",
    "00008025a028ef61340bd939bc2195fe53756786",
    "6003e1a15d3c71ff63e1590620aa636276a067cb",
    "e9d8997f761aecb703304b3800ccf555c9f3dc64",
    "214b297fb1966a3b6d83"
);
const HASH: &str = concat!(
    "0x33469b22e9f636356c",
    "4160a87eb19df52b7412",
    "e8eac32a4a55ffe88ea8",
    "350788"
);

fn message(block: u64, index: u64) -> String {
    serde_json::json!({
        "version": 1, "timestamp_ms": 1_790_000_000_000u64, "pga_round": 1,
        "transaction": { "block_number": block, "tx_index": index, "raw_tx": format!("0x{RAW}"),
                         "tx_hash": HASH, "receipt": { "status": "0x1", "logs": [] } }
    })
    .to_string()
}

// The handshake callback's error type is fixed by tungstenite's signature.
#[allow(clippy::result_large_err)]
/// Accepts one client, requires the bearer key, then sends `frames` (binary) and holds the socket open.
async fn serve(listener: TcpListener, key: &'static str, frames: Vec<String>) {
    let (sock, _) = listener.accept().await.unwrap();
    let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let got = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if got == Some(&format!("Bearer {key}")) {
            Ok(resp)
        } else {
            Err(tokio_tungstenite::tungstenite::http::Response::builder()
                .status(401)
                .body(Some("unauthorized".into()))
                .unwrap())
        }
    };
    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(sock, check).await else {
        return;
    };
    for f in frames {
        ws.send(Message::Binary(f.into_bytes().into()))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn binary_json_frames_with_a_bearer_key_are_decoded_and_sampled() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    // Positions 0, 1, then a jump to 4: two messages are missing.
    let frames = vec![message(100, 0), message(100, 1), message(100, 4)];
    tokio::spawn(serve(listener, "secret-key", frames.clone()));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let (sample_tx, mut sample_rx) = mpsc::unbounded_channel();
    let stats = Arc::new(FastFeedStats::default());
    tokio::spawn(fast_feed::run_with_samples(
        url,
        "secret-key".into(),
        tx,
        stats.clone(),
        Some(sample_tx),
    ));
    for want in [(100u64, 0u64), (100, 1), (100, 4)] {
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a decoded message arrives")
            .unwrap();
        assert_eq!((got.block_number, got.tx_index), want);
        assert_eq!(got.pga_round, Some(1));
    }
    // The raw text of every message is offered as a fixture.
    let first = sample_rx.recv().await.unwrap();
    assert_eq!(first, frames[0]);
    use std::sync::atomic::Ordering::Relaxed;
    assert_eq!(stats.msgs.load(Relaxed), 3);
    assert_eq!(
        (stats.gaps.load(Relaxed), stats.missing.load(Relaxed)),
        (1, 2),
        "the jump is a gap of two"
    );
    assert_eq!(stats.decode_errors.load(Relaxed), 0);
}

#[tokio::test]
async fn a_wrong_key_never_yields_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    tokio::spawn(serve(listener, "right-key", vec![message(1, 0)]));
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(fast_feed::run(
        url,
        "wrong-key".into(),
        tx,
        Arc::new(FastFeedStats::default()),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(800), rx.recv())
            .await
            .is_err(),
        "an unauthorized upgrade delivers nothing"
    );
}
