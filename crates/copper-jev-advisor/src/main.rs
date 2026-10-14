use std::process::ExitCode;

use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use copper_jev_advisor::observability::{self, SnapshotCache};
use copper_jev_advisor::{Config, RawArgs, Worker, check, init_tracing};
use jev_advisor::CancellationToken;
use mev_metrics::{Registry, ServerHandle, parse_listen_addr};

#[derive(Parser)]
#[command(
    name = "copper-jev-advisor",
    version,
    about = "Jev liquidation advisor worker (shadow mode)"
)]
struct Cli {
    #[command(flatten)]
    args: RawArgs,
    /// Serve Prometheus `/metrics` on this `ip:port` (default off). Loopback only unless
    /// `--metrics-allow-non-loopback` is also given; the advisor must not be exposed publicly.
    #[arg(long, env = "ADVISOR_METRICS_ADDR")]
    metrics_addr: Option<String>,
    /// Explicitly allow `--metrics-addr` to be a non-loopback address (e.g. 0.0.0.0 inside a container
    /// whose published port is bound to the host's loopback).
    #[arg(long, env = "ADVISOR_METRICS_ALLOW_NON_LOOPBACK")]
    metrics_allow_non_loopback: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the worker (default).
    Run,
    /// Readiness probe: validate configuration, open both databases, compare schema versions.
    /// Never contacts the Jev API.
    Check,
}

/// Install the handlers immediately (before storage and TLS setup, which take a while) so a
/// SIGTERM during startup is a clean stop, not a kill. `None` if they cannot be installed.
fn install_signal_handlers() -> Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)> {
    use tokio::signal::unix::{SignalKind, signal};
    Some((
        signal(SignalKind::terminate()).ok()?,
        signal(SignalKind::interrupt()).ok()?,
    ))
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = match Config::from_raw(cli.args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    let metrics_addr = match cli
        .metrics_addr
        .as_deref()
        .filter(|a| !a.trim().is_empty())
        .map(|a| parse_listen_addr(a, cli.metrics_allow_non_loopback))
        .transpose()
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    match cli.command.unwrap_or(Cmd::Run) {
        Cmd::Check => match check(&cfg) {
            Ok(r) => {
                println!(
                    "{{\"ready\":true,\"snapshot_schema\":{},\"advisor_schema\":{}}}",
                    r.snapshot_schema, r.advisor_schema
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("not ready: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Run => {
            init_tracing();
            let shutdown = CancellationToken::new();
            match install_signal_handlers() {
                Some((mut term, mut int)) => {
                    let trigger = shutdown.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = term.recv() => {}
                            _ = int.recv() => {}
                        }
                        trigger.cancel();
                    });
                }
                None => eprintln!("warning: could not install signal handlers"),
            }
            let client_cfg = cfg.client_config();
            let worker = match Worker::new(cfg, client_cfg) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("startup failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // Optional exporter: renders only the cached snapshot, never touches the worker or the DB.
            let registry = Arc::new(Registry::new());
            let _exporter = match metrics_addr {
                None => None,
                Some(addr) => {
                    let cache = SnapshotCache::default();
                    observability::install(&registry, &cache);
                    let handle = match ServerHandle::start(addr, Arc::clone(&registry)) {
                        Ok(h) => h,
                        Err(e) => {
                            eprintln!("metrics listener failed on {addr}: {e}");
                            return ExitCode::FAILURE;
                        }
                    };
                    tokio::spawn(observability::refresh_loop(
                        Arc::clone(&worker),
                        cache,
                        Duration::from_secs(5),
                        shutdown.clone(),
                    ));
                    Some(handle)
                }
            };
            match worker.run(shutdown).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("worker failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
