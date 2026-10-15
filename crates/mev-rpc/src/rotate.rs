//! Rotating transport over several RPC endpoints that learns which ones to use.
//!
//! Public RPCs rate-limit per IP and keyed ones have monthly quotas. Spreading reads across
//! several endpoints keeps each under its limit. When one fails for its own reasons — a transport
//! error *or* a JSON-RPC error payload such as PublicNode's `-32005` rate limit or "eth_call is
//! not supported", which alloy's `FallbackLayer` would treat as success — the request is retried
//! on the next endpoint. EVM execution reverts are returned as-is (every node would agree).
//!
//! Providers don't publish their remaining quota over JSON-RPC, so each endpoint's state is
//! learned from its answers:
//! - **quota exhausted** ("monthly quota", "capacity limit", "daily limit"): skipped for
//!   [`QUOTA_COOLDOWN`], then probed with one request; it stays exhausted until the plan resets.
//! - **rate-limited, failing or timing out**: skipped for [`BACKOFF_MIN`], doubling on each
//!   further failure up to [`BACKOFF_MAX`]; one success clears it.
//! - **healthy**: ordered by measured latency (moving average); the load rotates over the
//!   [`SPREAD`] fastest so one public endpoint is not driven into its per-IP limit.
//!
//! Fallback endpoints (free public ones) are tried only after every primary (keyed) one fails
//! or is skipped, so they carry load only while the keyed ones cannot. Skipped endpoints are
//! still tried last when nothing else answers.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy::{
    rpc::json_rpc::{RequestPacket, ResponsePacket},
    transports::{BoxTransport, TransportError, TransportErrorKind, TransportFut, http::Http},
};
use tower::Service;
use tracing::{info, warn};

/// Per-endpoint attempt timeout before falling through to the next endpoint.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an endpoint whose quota is exhausted is skipped before one probe request.
pub const QUOTA_COOLDOWN: Duration = Duration::from_secs(3_600);
/// First skip after a rate limit / failure; doubles per consecutive failure.
pub const BACKOFF_MIN: Duration = Duration::from_secs(2);
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// Healthy endpoints of a group share the load over this many of the fastest.
pub const SPREAD: usize = 3;

/// JSON-RPC codes / messages meaning "slow down" rather than a real call failure.
pub fn is_rate_limit_payload(code: i64, message: &str) -> bool {
    let m = message.to_lowercase();
    matches!(code, -32005 | 429 | -32029)
        || m.contains("rate limit")
        || m.contains("too many requests")
        || is_quota_message(message)
}

/// The endpoint's plan is used up (monthly / daily quota), not a momentary rate limit.
pub fn is_quota_message(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("quota")
        || m.contains("capacity limit")
        || m.contains("monthly")
        || m.contains("daily limit")
        || m.contains("daily request")
}

/// An EVM execution failure is the same on every node: return it. Anything else (rate limits,
/// "method not supported", internal errors) is the endpoint's problem: try the next one.
pub fn is_execution_error(code: i64, message: &str) -> bool {
    let m = message.to_lowercase();
    code == 3 || m.contains("revert") || m.contains("out of gas") || m.contains("invalid opcode")
}

/// What one attempt says about the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Answered (including EVM reverts): use the response.
    Answered,
    /// Plan used up.
    Quota,
    /// Rate limit, unsupported method, transport error or timeout: try the next endpoint.
    Failed,
}

fn classify(r: &Result<ResponsePacket, TransportError>) -> Outcome {
    match r {
        Ok(resp) => {
            let mut out = Outcome::Answered;
            for e in resp.iter_errors() {
                if is_execution_error(e.code, &e.message) {
                    continue;
                }
                if is_quota_message(&e.message) {
                    return Outcome::Quota;
                }
                out = Outcome::Failed;
            }
            out
        }
        Err(e) if is_quota_message(&e.to_string()) => Outcome::Quota,
        Err(_) => Outcome::Failed,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// What has been learned about one endpoint (lock-free; shared by concurrent requests).
#[derive(Default)]
struct Health {
    /// Unix ms until which the endpoint is skipped (0 = available).
    skip_until: AtomicU64,
    /// Consecutive failures (sets the back-off).
    strikes: AtomicU32,
    /// Moving average of answer latency in µs (0 = not measured yet: tried early to measure it).
    latency_us: AtomicU64,
    /// Whether the endpoint is currently known to be out of quota (for logging state changes).
    quota: std::sync::atomic::AtomicBool,
}

impl Health {
    fn record(&self, label: &str, outcome: Outcome, latency: Duration, now: u64) {
        match outcome {
            Outcome::Answered => {
                let us = latency.as_micros().max(1) as u64;
                let old = self.latency_us.load(Ordering::Relaxed);
                let avg = if old == 0 { us } else { (old * 7 + us) / 8 };
                self.latency_us.store(avg, Ordering::Relaxed);
                self.strikes.store(0, Ordering::Relaxed);
                self.skip_until.store(0, Ordering::Relaxed);
                if self.quota.swap(false, Ordering::Relaxed) {
                    info!(endpoint = label, "RPC endpoint answering again");
                }
            }
            Outcome::Quota => {
                self.skip_until
                    .store(now + QUOTA_COOLDOWN.as_millis() as u64, Ordering::Relaxed);
                if !self.quota.swap(true, Ordering::Relaxed) {
                    warn!(
                        endpoint = label,
                        skip_s = QUOTA_COOLDOWN.as_secs(),
                        "RPC endpoint out of quota; using the others"
                    );
                }
            }
            Outcome::Failed => {
                let k = self.strikes.fetch_add(1, Ordering::Relaxed).min(16);
                let backoff = (BACKOFF_MIN.as_millis() as u64)
                    .saturating_mul(1 << k)
                    .min(BACKOFF_MAX.as_millis() as u64);
                self.skip_until.store(now + backoff, Ordering::Relaxed);
            }
        }
    }
}

/// Attempt order: per group (primaries, then fallbacks), available endpoints by latency with the
/// first `SPREAD` rotated by `start`; then every skipped endpoint, soonest-available first.
/// `state[i]` = (skip_until ms, latency µs).
fn attempt_order(start: usize, primary: usize, state: &[(u64, u64)], now: u64) -> Vec<usize> {
    let mut order = Vec::with_capacity(state.len());
    let mut skipped = Vec::new();
    for group in [0..primary, primary..state.len()] {
        let mut ready: Vec<usize> = group
            .filter(|&i| {
                let ok = state[i].0 <= now;
                if !ok {
                    skipped.push(i);
                }
                ok
            })
            .collect();
        ready.sort_by_key(|&i| state[i].1);
        let k = ready.len().min(SPREAD);
        if k > 0 {
            ready[..k].rotate_left(start % k);
        }
        order.extend(ready);
    }
    skipped.sort_by_key(|&i| state[i].0);
    order.extend(skipped);
    order
}

/// Rotates over the primaries, then falls through to the fallbacks, learning which endpoints
/// are out of quota, failing or slow (see the module docs).
#[derive(Clone)]
pub struct RotatingTransport {
    inner: Arc<Vec<BoxTransport>>,
    /// Host of each endpoint for logs (never the path: keyed URLs carry the key there).
    labels: Arc<Vec<String>>,
    /// The first `primary` transports share the load; the rest are fallbacks.
    primary: usize,
    health: Arc<Vec<Health>>,
    next: Arc<AtomicUsize>,
}

impl RotatingTransport {
    pub fn new(transports: Vec<BoxTransport>) -> Self {
        let n = transports.len();
        Self::with_primary(transports, n)
    }

    /// The first `primary` transports rotate; the others are used only when those fail.
    pub fn with_primary(transports: Vec<BoxTransport>, primary: usize) -> Self {
        let labels = (0..transports.len()).map(|i| format!("#{i}")).collect();
        Self::labelled(transports, labels, primary)
    }

    fn labelled(transports: Vec<BoxTransport>, labels: Vec<String>, primary: usize) -> Self {
        assert!(
            !transports.is_empty(),
            "RotatingTransport needs at least one transport"
        );
        let primary = primary.clamp(1, transports.len());
        let health = transports.iter().map(|_| Health::default()).collect();
        Self {
            inner: Arc::new(transports),
            labels: Arc::new(labels),
            primary,
            health: Arc::new(health),
            next: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn http(urls: &[String]) -> anyhow::Result<Self> {
        Self::http_with_fallbacks(urls, &[])
    }

    /// `urls` rotate; `fallbacks` serve only when every one of `urls` fails or is skipped.
    pub fn http_with_fallbacks(urls: &[String], fallbacks: &[String]) -> anyhow::Result<Self> {
        let mut ts = Vec::new();
        let mut labels = Vec::new();
        for u in urls.iter().chain(fallbacks) {
            let url: alloy::transports::http::reqwest::Url = u.parse()?;
            labels.push(url.host_str().unwrap_or("?").to_string());
            ts.push(BoxTransport::new(Http::new(url)));
        }
        let primary = if urls.is_empty() {
            ts.len()
        } else {
            urls.len()
        };
        Ok(Self::labelled(ts, labels, primary))
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
        let labels = self.labels.clone();
        let health = self.health.clone();
        let primary = self.primary;
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            let state: Vec<(u64, u64)> = health
                .iter()
                .map(|h| {
                    (
                        h.skip_until.load(Ordering::Relaxed),
                        h.latency_us.load(Ordering::Relaxed),
                    )
                })
                .collect();
            let mut last: Option<Result<ResponsePacket, TransportError>> = None;
            for i in attempt_order(start, primary, &state, now_ms()) {
                let mut t = transports[i].clone();
                let t0 = Instant::now();
                // A hung endpoint must not stall the caller (reqwest has no default timeout).
                let r = match tokio::time::timeout(ATTEMPT_TIMEOUT, t.call(req.clone())).await {
                    Ok(r) => r,
                    Err(_) => Err(TransportErrorKind::custom_str("endpoint timed out")),
                };
                let outcome = classify(&r);
                health[i].record(&labels[i], outcome, t0.elapsed(), now_ms());
                if outcome == Outcome::Answered {
                    return r;
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
    fn fallbacks_come_after_every_primary_and_skipped_ones_last() {
        // 2 primaries, 3 fallbacks; all available and unmeasured.
        let fresh = [(0, 0); 5];
        assert_eq!(attempt_order(0, 2, &fresh, 10), [0, 1, 2, 3, 4]);
        assert_eq!(attempt_order(1, 2, &fresh, 10), [1, 0, 3, 4, 2]);
        // Primary 0 out of quota until 500, fallback 3 backing off until 20: both go last,
        // soonest-available first.
        let s = [(500, 0), (0, 0), (0, 0), (20, 0), (0, 0)];
        assert_eq!(attempt_order(0, 2, &s, 10), [1, 2, 4, 3, 0]);
        // Every primary skipped: fallbacks take the load.
        let s = [(500, 0), (500, 0), (0, 0), (0, 0), (0, 0)];
        assert_eq!(attempt_order(0, 2, &s, 10)[..3], [2, 3, 4]);
    }

    #[test]
    fn healthy_endpoints_are_ordered_by_latency_and_load_spreads_over_the_fastest() {
        // One group of 5, latencies in µs: 4 is fastest, then 2, 0, 3, 1.
        let s = [(0, 300), (0, 900), (0, 200), (0, 500), (0, 100)];
        assert_eq!(attempt_order(0, 5, &s, 0), [4, 2, 0, 3, 1]);
        assert_eq!(attempt_order(1, 5, &s, 0), [2, 0, 4, 3, 1]);
        assert_eq!(attempt_order(2, 5, &s, 0), [0, 4, 2, 3, 1]);
        assert_eq!(attempt_order(3, 5, &s, 0), [4, 2, 0, 3, 1]);
    }

    #[test]
    fn health_learns_quota_backoff_and_recovery() {
        let h = Health::default();
        h.record("x", Outcome::Quota, Duration::ZERO, 1_000);
        assert_eq!(
            h.skip_until.load(Ordering::Relaxed),
            1_000 + QUOTA_COOLDOWN.as_millis() as u64
        );
        h.record("x", Outcome::Answered, Duration::from_millis(80), 2_000);
        assert_eq!(h.skip_until.load(Ordering::Relaxed), 0);
        assert_eq!(h.latency_us.load(Ordering::Relaxed), 80_000);
        // Failures back off 2 s, 4 s, 8 s ... capped.
        for (k, want) in [(0, 2_000), (1, 4_000), (2, 8_000)] {
            h.record("x", Outcome::Failed, Duration::ZERO, 0);
            assert_eq!(h.skip_until.load(Ordering::Relaxed), want, "strike {k}");
        }
        for _ in 0..20 {
            h.record("x", Outcome::Failed, Duration::ZERO, 0);
        }
        assert_eq!(
            h.skip_until.load(Ordering::Relaxed),
            BACKOFF_MAX.as_millis() as u64
        );
        // Latency is a moving average.
        h.record("x", Outcome::Answered, Duration::from_millis(160), 0);
        assert_eq!(
            h.latency_us.load(Ordering::Relaxed),
            (80_000 * 7 + 160_000) / 8
        );
    }

    #[test]
    fn quota_answers_are_told_apart_from_rate_limits() {
        assert!(is_quota_message(
            "Monthly capacity limit exceeded. Visit https://dashboard.alchemy.com"
        ));
        assert!(is_quota_message(
            "You've reached your monthly quota of Request Units (RUs)."
        ));
        assert!(!is_quota_message("Rate limit exceeded"));
        assert!(!is_quota_message("Too Many Requests"));
    }

    /// Local JSON-RPC stub answering every request with `body`; counts the requests.
    async fn stub(body: &'static str) -> (String, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let h = h.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    // One request per connection read is enough for these small bodies.
                    while let Ok(n) = sock.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        h.fetch_add(1, Ordering::Relaxed);
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        if sock.write_all(resp.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (url, hits)
    }

    #[tokio::test]
    async fn an_out_of_quota_primary_is_skipped_after_its_first_refusal() {
        use alloy::providers::{Provider, ProviderBuilder};
        let (keyed, keyed_hits) = stub(
            r#"{"jsonrpc":"2.0","id":0,"error":{"code":429,"message":"Monthly capacity limit exceeded."}}"#,
        )
        .await;
        let (public, public_hits) = stub(r#"{"jsonrpc":"2.0","id":0,"result":"0xa4b1"}"#).await;
        let t = RotatingTransport::http_with_fallbacks(&[keyed], &[public]).unwrap();
        let client = alloy::rpc::client::ClientBuilder::default().transport(t, false);
        let p = ProviderBuilder::new().connect_client(client);
        for _ in 0..5 {
            assert_eq!(p.get_chain_id().await.unwrap(), 42161);
        }
        // The keyed endpoint was asked once, then skipped; the public one served everything.
        assert_eq!(keyed_hits.load(Ordering::Relaxed), 1);
        assert_eq!(public_hits.load(Ordering::Relaxed), 5);
    }
}
