//! Hardened TypeSafe HTTP transport. One persistent `reqwest::Client` (rustls, no redirects, no
//! proxy). No retry, circuit breaker or quota lives here (JEV-LIQ-010): `Transport::send` makes
//! exactly one attempt and returns a typed [`TransportError`].

use std::future::Future;
use std::time::{Duration, SystemTime};

use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER,
};
use reqwest::{Client, Url, redirect};
use tokio_util::sync::CancellationToken;

use crate::error::{RetryableKind, TimeoutKind, TransportError};
use crate::request::AdvisoryRequest;
use crate::response::{AdvisoryAnswer, ResponsePolicy, parse_response};
use crate::secret::Secret;

/// Upper bound applied to a parsed `Retry-After` (one day) so a hostile value cannot park a worker.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(86_400);

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Full endpoint URL, from configuration only.
    pub endpoint: String,
    /// Bare host names; the endpoint host must equal one of them.
    pub allowed_hosts: Vec<String>,
    /// Never logged, never in Debug output.
    pub api_key: Secret,
    /// Request model. Must be on `response.allowed_models`.
    pub model: String,
    pub response: ResponsePolicy,
    pub connect_timeout: Duration,
    /// Max stall between reads.
    pub read_timeout: Duration,
    /// Whole exchange including body streaming.
    pub total_timeout: Duration,
    pub max_response_bytes: usize,
    /// Allow `http://` to a literal loopback IP (test servers only).
    #[cfg(feature = "loopback-http")]
    pub allow_loopback_http: bool,
}

impl ClientConfig {
    /// Production defaults for the real API.
    pub fn new(api_key: Secret, model: &str, allowed_models: Vec<String>) -> Self {
        Self {
            endpoint: "https://api.typesafe.ai/v1/systemone".to_string(),
            allowed_hosts: vec!["api.typesafe.ai".to_string()],
            api_key,
            model: model.to_string(),
            response: ResponsePolicy::new(allowed_models),
            connect_timeout: Duration::from_secs(3),
            read_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(20),
            max_response_bytes: 64 * 1024,
            #[cfg(feature = "loopback-http")]
            allow_loopback_http: false,
        }
    }

    fn loopback(&self) -> bool {
        #[cfg(feature = "loopback-http")]
        {
            self.allow_loopback_http
        }
        #[cfg(not(feature = "loopback-http"))]
        {
            false
        }
    }
}

fn cfg_err(msg: &str) -> TransportError {
    TransportError::Config(msg.to_string())
}

/// Validate the endpoint against the allowlist before any connection. Messages never echo the URL
/// (it could carry credentials).
pub fn validate_endpoint(
    endpoint: &str,
    allowed_hosts: &[String],
    loopback_http: bool,
) -> Result<Url, TransportError> {
    let url = Url::parse(endpoint).map_err(|_| cfg_err("endpoint is not a valid URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(cfg_err("endpoint must not contain credentials"));
    }
    if url.fragment().is_some() {
        return Err(cfg_err("endpoint must not contain a fragment"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| cfg_err("endpoint has no host"))?;
    let is_loopback_ip = host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" => {}
        "http" if loopback_http && is_loopback_ip => {}
        _ => return Err(cfg_err("endpoint must use https")),
    }
    if url.scheme() == "https" && url.port().is_some() {
        return Err(cfg_err("endpoint must use the default https port"));
    }
    if !allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return Err(cfg_err("endpoint host is not on the allowlist"));
    }
    Ok(url)
}

/// Parse `Retry-After`: delta-seconds or an HTTP-date (relative to `now`). Capped at
/// [`MAX_RETRY_AFTER`]; a date in the past yields zero; garbage yields `None`.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if v.bytes().all(|b| b.is_ascii_digit()) {
        let secs: u64 = v.parse().ok().or(Some(u64::MAX))?;
        return Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER));
    }
    let when = httpdate::parse_http_date(v).ok()?;
    Some(
        when.duration_since(now)
            .unwrap_or(Duration::ZERO)
            .min(MAX_RETRY_AFTER),
    )
}

/// What task 010 wraps. One attempt, no hidden retries.
pub trait Transport: Send + Sync {
    fn send(
        &self,
        request: &AdvisoryRequest,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<AdvisoryAnswer, TransportError>> + Send;
}

pub struct TypeSafeClient {
    http: Client,
    url: Url,
    auth: HeaderValue,
    model: String,
    policy: ResponsePolicy,
    max_bytes: usize,
}

impl std::fmt::Debug for TypeSafeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeSafeClient")
            .field("host", &self.url.host_str())
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl TypeSafeClient {
    pub fn new(cfg: ClientConfig) -> Result<Self, TransportError> {
        let loopback = cfg.loopback();
        let url = validate_endpoint(&cfg.endpoint, &cfg.allowed_hosts, loopback)?;
        if !cfg.response.allowed_models.contains(&cfg.model) {
            return Err(cfg_err("request model is not on the model allowlist"));
        }
        if cfg.max_response_bytes == 0 {
            return Err(cfg_err("max_response_bytes must be positive"));
        }
        if cfg.connect_timeout.is_zero()
            || cfg.read_timeout.is_zero()
            || cfg.total_timeout.is_zero()
        {
            return Err(cfg_err("timeouts must be positive"));
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", cfg.api_key.expose()))
            .map_err(|_| cfg_err("api key is not a valid header value"))?;
        auth.set_sensitive(true);
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .no_proxy()
            .https_only(!loopback)
            .connect_timeout(cfg.connect_timeout)
            .read_timeout(cfg.read_timeout)
            .timeout(cfg.total_timeout)
            .user_agent("jev-advisor/0.1")
            .build()
            .map_err(|_| cfg_err("http client could not be built"))?;
        Ok(Self {
            http,
            url,
            auth,
            model: cfg.model,
            policy: cfg.response,
            max_bytes: cfg.max_response_bytes,
        })
    }

    async fn exchange(&self, request: &AdvisoryRequest) -> Result<AdvisoryAnswer, TransportError> {
        if request.key().model != self.model {
            return Err(cfg_err("request model differs from the configured model"));
        }
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.auth.clone());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let mut resp = self
            .http
            .post(self.url.clone())
            .headers(headers)
            .body(request.body().to_vec())
            .send()
            .await
            .map_err(map_reqwest)?;

        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_retry_after(v, SystemTime::now()));
        match status {
            200..=299 => {}
            401 | 403 => return Err(TransportError::Auth(status)),
            400 | 422 => return Err(TransportError::BadRequest(status)),
            429 => {
                return Err(TransportError::Retryable {
                    kind: RetryableKind::RateLimited,
                    retry_after,
                });
            }
            529 => {
                return Err(TransportError::Retryable {
                    kind: RetryableKind::Overloaded,
                    retry_after,
                });
            }
            500..=599 => {
                return Err(TransportError::Retryable {
                    kind: RetryableKind::Server(status),
                    retry_after,
                });
            }
            other => return Err(TransportError::UnexpectedStatus(other)),
        }

        let is_json = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("application/json")
            })
            .unwrap_or(false);
        if !is_json {
            return Err(TransportError::NotJson);
        }
        let oversized = TransportError::Oversized {
            limit: self.max_bytes,
        };
        if let Some(len) = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            && len > self.max_bytes as u64
        {
            return Err(oversized);
        }
        // Cap enforced while streaming: stop reading the moment the cap is crossed.
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(map_reqwest)? {
            if body.len() + chunk.len() > self.max_bytes {
                return Err(oversized);
            }
            body.extend_from_slice(&chunk);
        }
        parse_response(&body, &self.policy)
    }
}

fn map_reqwest(e: reqwest::Error) -> TransportError {
    // The reqwest message can contain the URL; it is deliberately never forwarded.
    if e.is_timeout() {
        if e.is_connect() {
            TransportError::Timeout(TimeoutKind::Connect)
        } else {
            TransportError::Timeout(TimeoutKind::Deadline)
        }
    } else if e.is_redirect() {
        TransportError::UnexpectedStatus(0)
    } else {
        TransportError::Retryable {
            kind: RetryableKind::Connection,
            retry_after: None,
        }
    }
}

impl Transport for TypeSafeClient {
    async fn send(
        &self,
        request: &AdvisoryRequest,
        cancel: &CancellationToken,
    ) -> Result<AdvisoryAnswer, TransportError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(TransportError::Cancelled),
            r = self.exchange(request) => r,
        }
    }
}
