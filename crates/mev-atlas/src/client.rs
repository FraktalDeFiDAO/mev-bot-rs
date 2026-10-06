//! The auction websocket: subscribe, stream auctions out, take signed solutions in. Reconnects
//! with backoff; never panics on malformed frames.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tracing::{info, warn};

use crate::{Auction, parse_notification, subscribe_frame};

/// Spec: read and write buffers must exceed 10 KB (auction frames carry the raw report).
const BUFFER: usize = 1 << 20;
const SUBSCRIBE_ID: u64 = 1;

/// What the connection reports besides auctions.
#[derive(Debug, Clone)]
pub enum Event {
    Connected,
    Disconnected(String),
    /// Reply to a submission: `error` is the JSON-RPC error text, if any.
    Submitted {
        id: u64,
        error: Option<String>,
    },
}

/// Run until the receiving side of `auctions` is dropped. `submissions` carries ready-made
/// request frames (see [`crate::submit_frame`]).
pub async fn run(
    url: String,
    auctions: mpsc::UnboundedSender<Auction>,
    events: mpsc::UnboundedSender<Event>,
    mut submissions: mpsc::UnboundedReceiver<String>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let cfg = WebSocketConfig::default()
            .read_buffer_size(BUFFER)
            .write_buffer_size(BUFFER);
        match connect_async_with_config(&url, Some(cfg), false).await {
            Ok((ws, _)) => {
                let (mut sink, mut stream) = ws.split();
                if sink
                    .send(Message::text(subscribe_frame(SUBSCRIBE_ID)))
                    .await
                    .is_err()
                {
                    warn!("atlas: subscribe failed");
                } else {
                    info!(%url, "atlas: connected, subscribed to auctions");
                    let _ = events.send(Event::Connected);
                    backoff = Duration::from_secs(1);
                    let why = loop {
                        tokio::select! {
                            frame = stream.next() => match frame {
                                Some(Ok(Message::Text(t))) => {
                                    handle(&t, &auctions, &events);
                                    if auctions.is_closed() { return; }
                                }
                                Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                                Some(Ok(Message::Close(_))) | None => break "closed".to_string(),
                                Some(Ok(_)) => {}
                                Some(Err(e)) => break e.to_string(),
                            },
                            out = submissions.recv() => match out {
                                Some(frame) => {
                                    if let Err(e) = sink.send(Message::text(frame)).await { break e.to_string(); }
                                }
                                None => return,
                            },
                        }
                    };
                    warn!(%why, "atlas: disconnected");
                    let _ = events.send(Event::Disconnected(why));
                }
            }
            Err(e) => {
                warn!(error = %e, "atlas: connect failed");
                let _ = events.send(Event::Disconnected(e.to_string()));
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn handle(
    text: &str,
    auctions: &mpsc::UnboundedSender<Auction>,
    events: &mpsc::UnboundedSender<Event>,
) {
    match parse_notification(text) {
        Ok(Some(a)) => {
            let _ = auctions.send(a);
        }
        Ok(None) => {
            // A reply to one of our requests (subscribe ack or a submission result).
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text)
                && let Some(id) = v["id"].as_u64()
                && id != SUBSCRIBE_ID
            {
                let error = v
                    .get("error")
                    .filter(|e| !e.is_null())
                    .map(|e| e.to_string());
                let _ = events.send(Event::Submitted { id, error });
            }
        }
        Err(e) => warn!(error = %format!("{e:#}"), "atlas: unparseable frame"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_submissions_become_events() {
        let (a_tx, _a_rx) = mpsc::unbounded_channel();
        let (e_tx, mut e_rx) = mpsc::unbounded_channel();
        handle(r#"{"jsonrpc":"2.0","id":7,"result":null}"#, &a_tx, &e_tx);
        handle(
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-1,"message":"late"}}"#,
            &a_tx,
            &e_tx,
        );
        handle(r#"{"jsonrpc":"2.0","id":1,"result":"0xsub"}"#, &a_tx, &e_tx); // subscribe ack: ignored
        let Event::Submitted { id: 7, error: None } = e_rx.try_recv().unwrap() else {
            panic!()
        };
        let Event::Submitted {
            id: 8,
            error: Some(e),
        } = e_rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(e.contains("late"));
        assert!(e_rx.try_recv().is_err());
    }

    #[test]
    fn auctions_are_forwarded() {
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (e_tx, _e_rx) = mpsc::unbounded_channel();
        handle(&crate::tests::notification(), &a_tx, &e_tx);
        assert_eq!(a_rx.try_recv().unwrap().chain_id, 42161);
    }
}
