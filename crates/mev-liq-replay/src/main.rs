use std::{io::Write, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use mev_liq_replay::{ReplayConfig, event::load_file, replay};
use mev_liq_sched::PolicyConfig;

#[derive(Parser)]
#[command(about = "Offline mandatory liquidation baseline replay; no RPC, advisor or signer")]
struct Args {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, default_value = "unknown")]
    code_revision: String,
    #[arg(long, default_value_t = 100)]
    tick_ms: u64,
    #[arg(long, default_value_t = 20)]
    recheck_ms: u64,
    #[arg(long, default_value_t = 16)]
    concurrency: u32,
    #[arg(long, default_value_t = 300)]
    max_age_seconds: u64,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let ns = |v: u64, scale: u64| v.checked_mul(scale).context("clock parameter overflow");
    let max_account_age_ns = ns(a.max_age_seconds, 1_000_000_000)?;
    let cfg = ReplayConfig {
        tick_ns: ns(a.tick_ms, 1_000_000)?,
        policy: PolicyConfig {
            recheck_cost_ns: ns(a.recheck_ms, 1_000_000)?,
            concurrency: a.concurrency,
            max_account_age_ns,
            expiring_window_ns: PolicyConfig::default()
                .expiring_window_ns
                .min(max_account_age_ns),
            ..PolicyConfig::default()
        },
        ..ReplayConfig::default()
    };
    let report = replay(&load_file(&a.input)?, &cfg, &a.code_revision)?;
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    if let Some(path) = a.output {
        std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    } else {
        std::io::stdout().lock().write_all(&bytes)?;
    }
    Ok(())
}
