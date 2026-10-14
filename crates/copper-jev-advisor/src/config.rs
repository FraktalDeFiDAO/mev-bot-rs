//! Startup configuration (spec section 12). Everything arrives through clap flags or environment
//! variables, is validated once, and is then immutable. The API key is read from a `0600` file
//! (preferred) or an environment variable and is wrapped in [`Secret`] whose `Debug` redacts.
//!
//! There is deliberately no RPC URL, signer, wallet, executor address or database URL setting:
//! Process B has no blockchain privileges (spec invariant 7). The two storage locations are SQLite
//! file paths (the repo uses SQLite, not Postgres; see docs/design/jev-liq-008-outbox.md).

use std::path::{Path, PathBuf};
use std::time::Duration;

use jev_advisor::{
    BreakerConfig, CacheConfig, ClientConfig, InflightPolicy, QuotaConfig, ResilienceConfig,
    RetryConfig, Secret, TokenPrices,
};

/// Operating mode. Only `Shadow` exists in this build: `Active` needs the policy gate of
/// JEV-LIQ-019 and is rejected at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Shadow,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error(
        "JEV_MODE=active is not supported by this build: active mode is gated behind \
         JEV-LIQ-019 (shadow-to-active promotion). Use JEV_MODE=shadow"
    )]
    ActiveModeNotSupported,
    #[error("JEV_MODE must be `shadow` (got an unknown value)")]
    UnknownMode,
    #[error("no API key: set TYPESAFE_API_KEY_FILE (preferred) or TYPESAFE_API_KEY")]
    MissingApiKey,
    #[error("set only one of TYPESAFE_API_KEY_FILE and TYPESAFE_API_KEY")]
    AmbiguousApiKey,
    #[error("API key file {0} cannot be read")]
    KeyFileUnreadable(PathBuf),
    #[error("API key file {0} is not a regular file")]
    KeyFileNotRegular(PathBuf),
    #[error(
        "API key file {path} has permissions {mode:04o}; it must not be accessible by group or \
         others (chmod 0600)"
    )]
    KeyFilePermissions { path: PathBuf, mode: u32 },
    #[error("API key is empty or contains whitespace/control/non-ASCII characters")]
    KeyMalformed,
    #[error("invalid endpoint configuration: {0}")]
    Endpoint(String),
    #[error("JEV_MODEL `{0}` is not in ALLOWED_MODELS")]
    ModelNotAllowed(String),
    #[error("ALLOWED_MODELS must list at least one model")]
    NoAllowedModels,
    #[error("JEV_POLICY_VERSION must be 1..=64 chars of [A-Za-z0-9._-]")]
    BadPolicyVersion,
    #[error("{0}")]
    Invalid(String),
}

fn invalid(m: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(m.into())
}

/// A raw secret as delivered by clap. Never parsed or echoed by clap (parse is infallible), and
/// `Debug` redacts, so it cannot appear in a clap error or a config dump.
#[derive(Clone)]
pub struct RawSecret(String);

impl RawSecret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Debug for RawSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RawSecret(<redacted>)")
    }
}

fn raw_secret(s: &str) -> Result<RawSecret, std::convert::Infallible> {
    Ok(RawSecret(s.to_string()))
}

/// Flags / environment variables. See `docs/design/jev-liq-012-worker.md` for the table.
#[derive(Debug, Clone, clap::Args)]
pub struct RawArgs {
    /// TypeSafe System One endpoint (https only).
    #[arg(
        long,
        env = "TYPESAFE_ENDPOINT",
        default_value = "https://api.typesafe.ai/v1/systemone"
    )]
    pub typesafe_endpoint: String,
    /// Comma-separated host allowlist the endpoint host must match.
    #[arg(
        long,
        env = "TYPESAFE_ALLOWED_HOSTS",
        value_delimiter = ',',
        default_value = "api.typesafe.ai"
    )]
    pub typesafe_allowed_hosts: Vec<String>,
    /// File holding the API key (mode 0600). Preferred over the environment variable.
    #[arg(long, env = "TYPESAFE_API_KEY_FILE", hide_env_values = true)]
    pub typesafe_api_key_file: Option<PathBuf>,
    /// API key itself (discouraged: visible in the process environment).
    #[arg(long, env = "TYPESAFE_API_KEY", hide_env_values = true, value_parser = raw_secret)]
    pub typesafe_api_key: Option<RawSecret>,
    #[arg(long, env = "JEV_MODEL", default_value = "jev-1.13.0")]
    pub jev_model: String,
    /// Comma-separated models a response may name (defaults to JEV_MODEL alone).
    #[arg(long, env = "ALLOWED_MODELS", value_delimiter = ',')]
    pub allowed_models: Vec<String>,
    /// `shadow` only.
    #[arg(long, env = "JEV_MODE", default_value = "shadow")]
    pub jev_mode: String,
    /// Must equal the policy version the producer exports with.
    #[arg(long, env = "JEV_POLICY_VERSION")]
    pub jev_policy_version: String,
    /// Chain the decisions are for.
    #[arg(long, env = "ADVISOR_CHAIN_ID", default_value_t = 42161)]
    pub chain_id: u64,
    /// Concurrent Jev requests (also the number of leased queue rows).
    #[arg(long, env = "JEV_MAX_INFLIGHT", default_value_t = 4)]
    pub jev_max_inflight: usize,
    /// Per-attempt timeout (clamped to the item's remaining TTL).
    #[arg(long, env = "JEV_TIMEOUT_MS", default_value_t = 1500)]
    pub jev_timeout_ms: u64,
    /// HTTP request budget per hour (retries included).
    #[arg(long, env = "JEV_BUDGET_PER_HOUR", default_value_t = 200)]
    pub jev_budget_per_hour: u32,
    /// Optional input+output token budget per hour.
    #[arg(long, env = "JEV_BUDGET_TOKENS_PER_HOUR")]
    pub jev_budget_tokens_per_hour: Option<u64>,
    /// Optional spend budget per hour in micro-USD; requires both price variables.
    #[arg(long, env = "JEV_BUDGET_USD_MICRO_PER_HOUR")]
    pub jev_budget_usd_micro_per_hour: Option<u64>,
    #[arg(long, env = "JEV_PRICE_INPUT_MICRO_USD_PER_MTOK")]
    pub jev_price_input_micro_usd_per_mtok: Option<u64>,
    #[arg(long, env = "JEV_PRICE_OUTPUT_MICRO_USD_PER_MTOK")]
    pub jev_price_output_micro_usd_per_mtok: Option<u64>,
    /// Advisory result storage (Process B owns and writes this SQLite file).
    #[arg(long, env = "ADVISOR_DB")]
    pub advisor_db: PathBuf,
    /// Process A's snapshot/export SQLite file; opened read-only.
    #[arg(long, env = "SNAPSHOT_DB")]
    pub snapshot_db: PathBuf,
    /// Decision and cache lifetime, and the claim lease (the item's TTL).
    #[arg(long, env = "ADVISOR_TTL_SECONDS", default_value_t = 60)]
    pub advisor_ttl_seconds: u64,
    /// Queue poll interval.
    #[arg(long, env = "ADVISOR_POLL_MS", default_value_t = 250)]
    pub advisor_poll_ms: u64,
    /// Maximum pending rows copied from the export log.
    #[arg(long, env = "ADVISOR_MAX_PENDING", default_value_t = 1000)]
    pub advisor_max_pending: u64,
    /// Attempts per queue row before it is dead-lettered.
    #[arg(long, env = "ADVISOR_MAX_ATTEMPTS", default_value_t = 5)]
    pub advisor_max_attempts: u32,
    /// Queue retry backoff base / cap.
    #[arg(long, env = "ADVISOR_RETRY_BASE_MS", default_value_t = 1000)]
    pub advisor_retry_base_ms: u64,
    #[arg(long, env = "ADVISOR_RETRY_MAX_MS", default_value_t = 60_000)]
    pub advisor_retry_max_ms: u64,
    /// Claiming pauses this long after a systemic fallback (breaker open, budget, saturation).
    #[arg(long, env = "ADVISOR_FALLBACK_PAUSE_MS", default_value_t = 2000)]
    pub advisor_fallback_pause_ms: u64,
    /// Grace period for in-flight work after SIGTERM/SIGINT before leases are released.
    #[arg(long, env = "ADVISOR_SHUTDOWN_GRACE_SECONDS", default_value_t = 10)]
    pub advisor_shutdown_grace_seconds: u64,
    /// Interval of the structured metrics log line.
    #[arg(long, env = "ADVISOR_METRICS_INTERVAL_SECONDS", default_value_t = 30)]
    pub advisor_metrics_interval_seconds: u64,
}

/// Validated, immutable configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub endpoint: String,
    pub allowed_hosts: Vec<String>,
    pub api_key: Secret,
    pub model: String,
    pub allowed_models: Vec<String>,
    pub mode: Mode,
    pub policy_version: String,
    pub chain_id: u64,
    pub resilience: ResilienceConfig,
    pub advisor_db: PathBuf,
    pub snapshot_db: PathBuf,
    pub ttl: Duration,
    pub poll: Duration,
    pub max_pending: u64,
    pub max_attempts: u32,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub fallback_pause: Duration,
    pub shutdown_grace: Duration,
    pub metrics_interval: Duration,
}

fn model_ok(m: &str) -> bool {
    (1..=64).contains(&m.len())
        && m.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn read_key_file(path: &Path) -> Result<Secret, ConfigError> {
    let meta =
        std::fs::metadata(path).map_err(|_| ConfigError::KeyFileUnreadable(path.to_path_buf()))?;
    if !meta.is_file() {
        return Err(ConfigError::KeyFileNotRegular(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o7777;
        if mode & 0o077 != 0 {
            return Err(ConfigError::KeyFilePermissions {
                path: path.to_path_buf(),
                mode,
            });
        }
    }
    let secret = Secret::from_file(path).map_err(|e| match e {
        jev_advisor::SecretError::FileUnreadable => {
            ConfigError::KeyFileUnreadable(path.to_path_buf())
        }
        _ => ConfigError::KeyMalformed,
    })?;
    Ok(secret)
}

impl Config {
    /// Validate everything. No network access.
    pub fn from_raw(raw: RawArgs) -> Result<Self, ConfigError> {
        match raw.jev_mode.as_str() {
            "shadow" => {}
            "active" => return Err(ConfigError::ActiveModeNotSupported),
            _ => return Err(ConfigError::UnknownMode),
        }

        let api_key = match (&raw.typesafe_api_key_file, &raw.typesafe_api_key) {
            (Some(_), Some(_)) => return Err(ConfigError::AmbiguousApiKey),
            (Some(p), None) => read_key_file(p)?,
            (None, Some(k)) => Secret::new(k.0.clone()).map_err(|_| ConfigError::KeyMalformed)?,
            (None, None) => return Err(ConfigError::MissingApiKey),
        };

        let allowed_hosts: Vec<String> = raw
            .typesafe_allowed_hosts
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        if allowed_hosts.is_empty() {
            return Err(ConfigError::Endpoint(
                "TYPESAFE_ALLOWED_HOSTS is empty".into(),
            ));
        }
        // https only (loopback http is a test-only cargo feature of the client crate).
        jev_advisor::validate_endpoint(&raw.typesafe_endpoint, &allowed_hosts, false)
            .map_err(|e| ConfigError::Endpoint(e.to_string()))?;

        if !model_ok(&raw.jev_model) {
            return Err(invalid("JEV_MODEL must be 1..=64 chars of [A-Za-z0-9._-]"));
        }
        let allowed_models: Vec<String> = if raw.allowed_models.is_empty() {
            vec![raw.jev_model.clone()]
        } else {
            raw.allowed_models
                .iter()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect()
        };
        if allowed_models.is_empty() {
            return Err(ConfigError::NoAllowedModels);
        }
        if let Some(bad) = allowed_models.iter().find(|m| !model_ok(m)) {
            return Err(invalid(format!(
                "ALLOWED_MODELS entry `{bad}` is malformed"
            )));
        }
        if !allowed_models.contains(&raw.jev_model) {
            return Err(ConfigError::ModelNotAllowed(raw.jev_model));
        }

        if !model_ok(&raw.jev_policy_version) {
            return Err(ConfigError::BadPolicyVersion);
        }

        let positive = |name: &str, v: u64, max: u64| {
            if v == 0 || v > max {
                Err(invalid(format!("{name} must be in 1..={max}")))
            } else {
                Ok(())
            }
        };
        positive("ADVISOR_TTL_SECONDS", raw.advisor_ttl_seconds, 86_400)?;
        positive("JEV_TIMEOUT_MS", raw.jev_timeout_ms, 600_000)?;
        positive("JEV_MAX_INFLIGHT", raw.jev_max_inflight as u64, 1024)?;
        positive("ADVISOR_POLL_MS", raw.advisor_poll_ms, 60_000)?;
        positive("ADVISOR_MAX_PENDING", raw.advisor_max_pending, 1_000_000)?;
        positive(
            "ADVISOR_MAX_ATTEMPTS",
            u64::from(raw.advisor_max_attempts),
            100,
        )?;
        positive(
            "ADVISOR_RETRY_BASE_MS",
            raw.advisor_retry_base_ms,
            3_600_000,
        )?;
        positive("ADVISOR_RETRY_MAX_MS", raw.advisor_retry_max_ms, 86_400_000)?;
        if raw.advisor_retry_base_ms > raw.advisor_retry_max_ms {
            return Err(invalid(
                "ADVISOR_RETRY_BASE_MS must be <= ADVISOR_RETRY_MAX_MS",
            ));
        }
        positive(
            "ADVISOR_SHUTDOWN_GRACE_SECONDS",
            raw.advisor_shutdown_grace_seconds,
            3600,
        )?;
        positive(
            "ADVISOR_METRICS_INTERVAL_SECONDS",
            raw.advisor_metrics_interval_seconds,
            86_400,
        )?;
        if raw.advisor_fallback_pause_ms > 3_600_000 {
            return Err(invalid("ADVISOR_FALLBACK_PAUSE_MS must be <= 1 hour"));
        }

        if raw.advisor_db == raw.snapshot_db {
            return Err(invalid(
                "ADVISOR_DB and SNAPSHOT_DB must be different files (the snapshot DB is read-only here)",
            ));
        }

        let prices = match (
            raw.jev_price_input_micro_usd_per_mtok,
            raw.jev_price_output_micro_usd_per_mtok,
        ) {
            (Some(i), Some(o)) => Some(TokenPrices {
                input_micro_usd_per_mtok: i,
                output_micro_usd_per_mtok: o,
            }),
            (None, None) => None,
            _ => {
                return Err(invalid(
                    "set both JEV_PRICE_INPUT_MICRO_USD_PER_MTOK and JEV_PRICE_OUTPUT_MICRO_USD_PER_MTOK, or neither",
                ));
            }
        };
        let resilience = ResilienceConfig {
            attempt_timeout: Duration::from_millis(raw.jev_timeout_ms),
            max_inflight: raw.jev_max_inflight,
            inflight_policy: InflightPolicy::Wait,
            retry: RetryConfig::default(),
            breaker: BreakerConfig::default(),
            quota: QuotaConfig {
                requests_per_window: raw.jev_budget_per_hour,
                tokens_per_window: raw.jev_budget_tokens_per_hour,
                usd_micro_per_window: raw.jev_budget_usd_micro_per_hour,
                prices,
                ..QuotaConfig::default()
            },
            cache: CacheConfig {
                ttl: Duration::from_secs(raw.advisor_ttl_seconds),
                ..CacheConfig::default()
            },
        };
        resilience.validate().map_err(|e| invalid(e.to_string()))?;

        Ok(Self {
            endpoint: raw.typesafe_endpoint,
            allowed_hosts,
            api_key,
            model: raw.jev_model,
            allowed_models,
            mode: Mode::Shadow,
            policy_version: raw.jev_policy_version,
            chain_id: raw.chain_id,
            resilience,
            advisor_db: raw.advisor_db,
            snapshot_db: raw.snapshot_db,
            ttl: Duration::from_secs(raw.advisor_ttl_seconds),
            poll: Duration::from_millis(raw.advisor_poll_ms),
            max_pending: raw.advisor_max_pending,
            max_attempts: raw.advisor_max_attempts,
            retry_base: Duration::from_millis(raw.advisor_retry_base_ms),
            retry_max: Duration::from_millis(raw.advisor_retry_max_ms),
            fallback_pause: Duration::from_millis(raw.advisor_fallback_pause_ms),
            shutdown_grace: Duration::from_secs(raw.advisor_shutdown_grace_seconds),
            metrics_interval: Duration::from_secs(raw.advisor_metrics_interval_seconds),
        })
    }

    /// The production transport configuration (https only, host allowlisted).
    pub fn client_config(&self) -> ClientConfig {
        let mut c = ClientConfig::new(
            self.api_key.clone(),
            &self.model,
            self.allowed_models.clone(),
        );
        c.endpoint = self.endpoint.clone();
        c.allowed_hosts = self.allowed_hosts.clone();
        c
    }
}
