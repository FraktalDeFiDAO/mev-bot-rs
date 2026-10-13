//! `copper-jev-advisor`: Process B of the Jev liquidation advisor (JEV-LIQ-012).
//!
//! A queue consumer that asks TypeSafe for advisory rankings and stores the answers as shadow
//! decisions. It composes `jev-advisor` (transport, retry, breaker, quota, cache) and
//! `mev-liq-state` (outbox, decision storage). It has no signer, no RPC client and no executor
//! channel, and its dependency graph is checked for that (`tests/isolation.rs`).

pub mod check;
pub mod config;
pub mod features;
pub mod worker;

pub use check::{CheckError, CheckReport, check};
pub use config::{Config, ConfigError, Mode, RawArgs};
pub use features::{FeatureReject, parse_redacted};
pub use worker::{CountersSnapshot, Worker, WorkerError};

/// Install the JSON log subscriber on stdout (`RUST_LOG` filters, default `info`).
pub fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(true)
        .with_span_list(false)
        .with_ansi(false)
        .init();
}
