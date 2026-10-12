//! Prepare a receipt-backed candidate ledger for operator review. Never reads a signing key.
use anyhow::{Context, Result, ensure};
use mev_analytics::accounting::{Manifest, prepare};
use mev_config::{ChainConfig, Secrets};
use mev_risk::ledger::Ledger;
use std::{io::Read, path::PathBuf};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{}", mev_types::redact_secrets(&format!("{e:#}")));
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: mev-accounting CONFIG MANIFEST OUTPUT.candidate.json"
    );
    let output = PathBuf::from(&args[2]);
    ensure!(
        output
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".candidate.json")),
        "output must be a reviewable .candidate.json file"
    );
    ensure!(!output.exists(), "output already exists");
    let _ = dotenvy::from_filename(".env.shadow");
    let cfg = ChainConfig::load(std::path::Path::new(&args[0]))?;
    let secrets = Secrets::read_only(&cfg.name.to_uppercase())?;
    let provider = mev_rpc::http_provider(
        secrets
            .http_urls()
            .first()
            .context("HTTP endpoint unavailable")?,
    )?;
    let mut bytes = Vec::new();
    std::fs::File::open(&args[1])?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1_048_576, "manifest exceeds one MiB");
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest.chain_id == cfg.chain_id,
        "manifest/config chain mismatch"
    );
    let seed = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        prepare(&provider, manifest),
    )
    .await??;
    let receipts = seed.receipts.len();
    let pending = seed.pending.len();
    Ledger::initialize(output.clone(), seed)?;
    eprintln!(
        "Prepared {}: {receipts} verified receipts, {pending} retained liabilities. Review before deployment.",
        output.display()
    );
    Ok(())
}
