//! Rotating transport over several free RPC endpoints.
//!
//! Public RPCs rate-limit per IP. Spreading heavy reads (health sweeps) across several endpoints
//! keeps each under its limit. When one fails for its own reasons — a transport error *or* a
//! JSON-RPC error payload such as PublicNode's `-32005` rate limit or "eth_call is not
//! supported", which alloy's `FallbackLayer` would treat as success — the request is retried on
//! the next endpoint. EVM execution reverts are returned as-is (every node would agree).
//!
//! An endpoint that answers with a rate limit or an exhausted quota is skipped for
//! [`COOLDOWN`] (a monthly quota stays exhausted for weeks: without the skip every request would
//! pay one wasted round trip to it first). Fallback endpoints are tried only after the primary
//! ones fail, so free public endpoints carry load only while the keyed ones cannot.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy::{
    rpc::json_rpc::{RequestPacket, ResponsePacket},
    transports::{BoxTransport, TransportError, TransportErrorKind, TransportFut, http::Http},
};
use tower::Service;

/// Per-endpoint attempt timeout before falling through to the next endpoint.
const ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// How long an endpoint that answered with a rate limit or exhausted quota is skipped.
const COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// JSON-RPC codes / messages meaning "slow down" rather than a real call failure.
pub fn is_rate_limit_payload(code: i64, message: &str) -> bool {
    let m = message.to_lowercase();
    matches!(code, -32005 | 429 | -32029)
        || m.contains("rate limit")
        || m.contains("too many requests")
        || m.contains("quota")
        || m.contains("capacity limit")
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

/// True when the endpoint said "slow down" (payload or HTTP 429): skip it for a while.
fn rate_limited(r: &Result<ResponsePacket, TransportError>) -> bool {
    match r {
        Ok(resp) => resp
            .iter_errors()
            .any(|e| is_rate_limit_payload(e.code, &e.message)),
        Err(e) => {
            let m = e.to_string();
            m.contains("429") || is_rate_limit_payload(0, &m)
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The order to try endpoints in: primaries round-robin from `start`, then the fallbacks, also
/// round-robin (one public endpoint alone would hit its per-IP limit at once).
fn attempt_order(start: usize, primary: usize, n: usize) -> impl Iterator<Item = usize> {
    let fallbacks = n - primary;
    (0..primary)
        .map(move |i| (start + i) % primary)
        .chain((0..fallbacks).map(move |i| primary + (start + i) % fallbacks))
}

/// Round-robin start over the primaries, then fall through the rest on rate limits / transport
/// errors; endpoints in cooldown are tried last.
#[derive(Clone)]
pub struct RotatingTransport {
    inner: Arc<Vec<BoxTransport>>,
    /// The first `primary` transports share the load; the rest are fallbacks.
    primary: usize,
    /// Per transport: unix ms until which it is skipped (0 = available).
    cooled: Arc<Vec<AtomicU64>>,
    next: Arc<AtomicUsize>,
}

impl RotatingTransport {
    pub fn new(transports: Vec<BoxTransport>) -> Self {
        let n = transports.len();
        Self::with_primary(transports, n)
    }

    /// The first `primary` transports rotate; the others are used only when those fail.
    pub fn with_primary(transports: Vec<BoxTransport>, primary: usize) -> Self {
        assert!(
            !transports.is_empty(),
            "RotatingTransport needs at least one transport"
        );
        let primary = primary.clamp(1, transports.len());
        let cooled = transports.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            inner: Arc::new(transports),
            primary,
            cooled: Arc::new(cooled),
            next: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn http(urls: &[String]) -> anyhow::Result<Self> {
        Self::http_with_fallbacks(urls, &[])
    }

    /// `urls` rotate; `fallbacks` serve only when every one of `urls` fails or is cooling down.
    pub fn http_with_fallbacks(urls: &[String], fallbacks: &[String]) -> anyhow::Result<Self> {
        let ts = urls
            .iter()
            .chain(fallbacks)
            .map(|u| Ok(BoxTransport::new(Http::new(u.parse()?))))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let primary = if urls.is_empty() {
            ts.len()
        } else {
            urls.len()
        };
        Ok(Self::with_primary(ts, primary))
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
        let cooled = self.cooled.clone();
        let primary = self.primary;
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            let n = transports.len();
            let now = now_ms();
            // Available endpoints first, then the cooling ones (better a likely refusal than none).
            let (ready, cooling): (Vec<usize>, Vec<usize>) = attempt_order(start, primary, n)
                .partition(|&i| cooled[i].load(Ordering::Relaxed) <= now);
            let mut last: Option<Result<ResponsePacket, TransportError>> = None;
            for i in ready.into_iter().chain(cooling) {
                let mut t = transports[i].clone();
                // A hung endpoint must not stall the caller (reqwest has no default timeout).
                let r = match tokio::time::timeout(ATTEMPT_TIMEOUT, t.call(req.clone())).await {
                    Ok(r) => r,
                    Err(_) => Err(TransportErrorKind::custom_str("endpoint timed out")),
                };
                if matches!(&r, Ok(resp) if !try_next(resp)) {
                    cooled[i].store(0, Ordering::Relaxed);
                    return r;
                }
                if rate_limited(&r) {
                    cooled[i].store(now_ms() + COOLDOWN.as_millis() as u64, Ordering::Relaxed);
                }
                last = Some(r);
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
        assert!(is_rate_limit_payload(
            -32000,
            "You've reached your monthly quota of Request Units (RUs)."
        ));
        assert!(is_rate_limit_payload(
            -32000,
            "Monthly capacity limit exceeded."
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

    #[test]
    fn fallbacks_come_after_every_primary() {
        let order = |start| attempt_order(start, 2, 5).collect::<Vec<_>>();
        assert_eq!(order(0), [0, 1, 2, 3, 4]);
        assert_eq!(order(1), [1, 0, 3, 4, 2]);
        assert_eq!(order(2), [0, 1, 4, 2, 3]);
        assert_eq!(attempt_order(3, 3, 3).collect::<Vec<_>>(), [0, 1, 2]);
    }
}
