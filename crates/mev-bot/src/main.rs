//! Bot binary: CLI, logging, config and secrets. Everything else lives in the crates.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use mev_config::{ChainConfig, Secrets};
use mev_engine::{Engine, Mode};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliMode {
    /// Find and simulate opportunities; never send.
    Shadow,
    /// Send transactions for opportunities that pass simulation and risk checks.
    Live,
}

#[derive(Parser, Debug)]
#[command(about = "L2 MEV bot")]
struct Args {
    #[arg(long, default_value = "config/arbitrum.toml")]
    config: PathBuf,
    #[arg(long, value_enum, default_value = "shadow")]
    mode: CliMode,
    /// Directory for event logs and the STOP kill-switch file.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(|| Redacting(std::io::stdout()))
        .init();
    let args = Args::parse();
    let cfg = ChainConfig::load(&args.config)?;
    let secrets = Secrets::from_env(&cfg.name.to_uppercase())?;
    let mode = match args.mode {
        CliMode::Shadow => Mode::Shadow,
        CliMode::Live => Mode::Live,
    };
    // Retry bootstrap with backoff instead of exiting: a crash makes podman restart us
    // immediately, which turns a network outage into a tight restart loop.
    let mut engine = mev_rpc::retry_forever("bootstrap", || {
        Engine::bootstrap(cfg.clone(), secrets.clone(), mode, args.data_dir.clone())
    })
    .await;
    engine.run().await
}

/// Log writer that masks API keys in URLs (RPC errors print the full endpoint URL). The fmt layer
/// writes each event in one call, so redacting per write sees whole lines.
struct Redacting<W>(W);

impl<W: std::io::Write> std::io::Write for Redacting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0
            .write_all(mev_types::redact_secrets(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
