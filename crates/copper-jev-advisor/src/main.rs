use std::process::ExitCode;

use clap::{Parser, Subcommand};
use copper_jev_advisor::{Config, RawArgs, Worker, check, init_tracing};
use jev_advisor::CancellationToken;

#[derive(Parser)]
#[command(
    name = "copper-jev-advisor",
    version,
    about = "Jev liquidation advisor worker (shadow mode)"
)]
struct Cli {
    #[command(flatten)]
    args: RawArgs,
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
