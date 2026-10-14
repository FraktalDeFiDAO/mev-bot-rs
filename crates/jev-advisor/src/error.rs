//! Typed error taxonomy. Task JEV-LIQ-010 (retry/breaker/quota) drives its policy from
//! [`TransportError::class`] and [`TransportError::retry_after`]. No variant ever carries the API
//! key, the request URL or raw response text beyond a short sanitized excerpt.

use std::time::Duration;

/// Which deadline fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    Connect,
    /// Total deadline or per-read stall (both bound the whole exchange).
    Deadline,
}

/// Why a transient failure is retryable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryableKind {
    /// HTTP 429.
    RateLimited,
    /// HTTP 529.
    Overloaded,
    /// HTTP 5xx other than 529.
    Server(u16),
    /// Connection reset/refused/DNS/body-read failure.
    Connection,
}

/// Contract violations in a 2xx response body.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("response is not the documented shape: {0}")]
    Schema(String),
    #[error("duplicate key `{0}`")]
    DuplicateKey(String),
    #[error("unexpected top-level key `{0}`")]
    UnknownTopLevelKey(String),
    #[error("missing answer `{0}`")]
    MissingAnswer(&'static str),
    #[error("unexpected answer id `{0}`")]
    UnexpectedAnswer(String),
    #[error("answer `{id}` has the wrong type `{got}`")]
    WrongAnswerType { id: &'static str, got: &'static str },
    #[error("unknown choice `{0}`")]
    UnknownChoice(String),
    #[error("probability keys of `{0}` are not exactly the expected set")]
    BadProbabilityKeys(&'static str),
    #[error("non-finite or out-of-range value in `{0}`")]
    OutOfRange(&'static str),
    #[error("probabilities of `{0}` do not sum to 1 within tolerance")]
    DistributionSum(&'static str),
    #[error("score is inconsistent with its probabilities")]
    ScoreInconsistent,
    #[error("legend of `monitoring_value` does not match the 5 levels")]
    BadLegend,
}

/// Everything `Transport::send` can fail with.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TransportError {
    /// Rejected before any connection (bad config / endpoint / key / request content).
    #[error("configuration or request rejected locally: {0}")]
    Config(String),
    #[error("transient failure ({kind:?})")]
    Retryable {
        kind: RetryableKind,
        retry_after: Option<Duration>,
    },
    #[error("timeout ({0:?})")]
    Timeout(TimeoutKind),
    /// 401 / 403: needs key correction, never retry.
    #[error("authentication rejected (HTTP {0})")]
    Auth(u16),
    /// 400 / 422: our request is invalid, never retry unchanged.
    #[error("request rejected as invalid (HTTP {0})")]
    BadRequest(u16),
    /// Any other status (3xx included: redirects are never followed).
    #[error("unexpected HTTP status {0}")]
    UnexpectedStatus(u16),
    #[error("response exceeded the {limit} byte cap")]
    Oversized { limit: usize },
    #[error("2xx response was not application/json")]
    NotJson,
    #[error("response body is not valid JSON")]
    MalformedJson,
    #[error("response contract violation: {0}")]
    SchemaDrift(ValidationError),
    /// `response.model` is not on the configured allowlist.
    #[error("unsupported response model `{model}`")]
    WrongModel { model: String },
    #[error("cancelled")]
    Cancelled,
}

/// What a retry layer should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Idempotent retry allowed (backoff, respect `retry_after`, bounded by TTL/budget).
    Retryable,
    /// Retrying unchanged cannot help; needs operator/config/code correction.
    NonRetryable,
    /// Caller cancelled; neither a failure of the service nor retryable.
    Cancelled,
}

impl TransportError {
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Retryable { .. } | Self::Timeout(_) => ErrorClass::Retryable,
            Self::Cancelled => ErrorClass::Cancelled,
            _ => ErrorClass::NonRetryable,
        }
    }

    /// Server-requested minimum delay, when it sent a parseable `Retry-After`.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Retryable { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// Truncate and strip control characters from server-supplied text before embedding it.
pub(crate) fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(120).collect()
}
