//! Rotating transport over several free RPC endpoints.
//!
//! Public RPCs rate-limit per IP. Spreading heavy reads (health sweeps) across several endpoints
//! keeps each under its limit. When one fails for its own reasons — a transport error *or* a
//! JSON-RPC error payload such as PublicNode's `-32005` rate limit or "eth_call is not
//! supported", which alloy's `FallbackLayer` would treat as success — the request is retried on
//! the next endpoint. EVM execution reverts are returned as-is (every node would agree).

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use alloy::{
    rpc::json_rpc::{RequestPacket, ResponsePacket},
    transports::{BoxTransport, TransportError, TransportErrorKind, TransportFut, http::Http},
};
use tower::Service;

/// JSON-RPC codes / messages meaning "slow down" rather than a real call failure.
pub fn is_rate_limit_payload(code: i64, message: &str) -> bool {
    let m = message.to_lowercase();
    matches!(code, -32005 | 429 | -32029)
        || m.contains("rate limit")
        || m.contains("too many requests")
}

/// An EVM execution failure is the same on every node: return it. Anything else (rate limits,
/// "method not supported", internal errors) is the endpoint's problem: try the next one.
pub fn is_execution_error(code: i64, message: &str) -> bool {
    let m = message.to_lowercase();
    code == 3 || m.contains("revert") || m.contains("out of gas") || m.contains("invalid opcode")
}

fn try_next(resp: &ResponsePacket) -> bool {
    resp.iter_errors()
        .any(|e| !is_execution_error(e.code, &e.message))
}

/// Round-robin start, then fall through the rest on rate limits / transport errors.
#[derive(Clone)]
pub struct RotatingTransport {
    inner: Arc<Vec<BoxTransport>>,
    next: Arc<AtomicUsize>,
}

impl RotatingTransport {
    pub fn new(transports: Vec<BoxTransport>) -> Self {
        assert!(
            !transports.is_empty(),
            "RotatingTransport needs at least one transport"
        );
        Self {
            inner: Arc::new(transports),
            next: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn http(urls: &[String]) -> anyhow::Result<Self> {
        let ts = urls
            .iter()
            .map(|u| Ok(BoxTransport::new(Http::new(u.parse()?))))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self::new(ts))
    }
}

impl Service<RequestPacket> for RotatingTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        let transports = self.inner.clone();
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            let n = transports.len();
            let mut last: Option<Result<ResponsePacket, TransportError>> = None;
            for i in 0..n {
                let mut t = transports[(start + i) % n].clone();
                match t.call(req.clone()).await {
                    Ok(resp) if !try_next(&resp) => return Ok(resp),
                    other => last = Some(other),
                }
            }
            last.unwrap_or_else(|| Err(TransportErrorKind::custom_str("no transports")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_rate_limit_payloads() {
        assert!(is_rate_limit_payload(
            -32005,
            "Rate limit exceeded. To obtain higher limits..."
        ));
        assert!(is_rate_limit_payload(429, "Too Many Requests"));
        assert!(is_rate_limit_payload(
            -32000,
            "daily request rate limit reached"
        ));
        assert!(!is_rate_limit_payload(3, "execution reverted"));
        assert!(!is_rate_limit_payload(-32602, "invalid argument"));
    }

    #[test]
    fn only_execution_errors_stop_the_rotation() {
        assert!(is_execution_error(3, "execution reverted"));
        assert!(is_execution_error(
            -32000,
            "execution reverted: NotProfitable"
        ));
        assert!(!is_execution_error(
            -32000,
            "The method eth_call is not supported"
        ));
        assert!(!is_execution_error(-32005, "Rate limit exceeded"));
    }
}
