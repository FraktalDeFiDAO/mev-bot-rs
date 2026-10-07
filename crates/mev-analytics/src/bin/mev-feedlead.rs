//! Experiment 0 (docs/runbooks/fast-feed-experiment.md): how much earlier does the paid Fast Feed
//! deliver each transaction than the free standard sequencer feed?
//!
//!   cargo run --release -p mev-analytics --bin mev-feedlead -- [config/arbitrum.toml] [secs=600] [out_dir=data/feedlead]
//!
//! Needs `ARBITRUM_FAST_FEED_KEY` (the raw API key of an active ticket). Connects to both feeds, pairs
//! every transaction by hash, and writes `lead.jsonl` (one line per paired transaction),
//! `fast_feed_samples.jsonl` (the first raw Fast Feed messages, fixtures for the feed decoder) and
//! `summary.json`. Read-only: it never touches the bot, a wallet or a transaction.

use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, atomic::Ordering::Relaxed},
    time::{Duration, Instant},
};

use alloy::primitives::B256;
use anyhow::{Context, Result};
use mev_config::ChainConfig;
use mev_ingest::{
    fast_feed::{self, FastFeedStats, FastFeedTx},
    lead::{Feed, LeadStats, LeadTracker},
};
use serde_json::json;
use tokio::sync::mpsc;

const SAMPLE_LIMIT: usize = 300;
/// A transaction not seen on the other feed within this long counts as missed by it.
const PAIR_WINDOW: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().collect();
    let cfg = ChainConfig::load(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("config/arbitrum.toml")
            .as_ref(),
    )?;
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(600);
    let out_dir = PathBuf::from(args.get(3).map(String::as_str).unwrap_or("data/feedlead"));
    std::fs::create_dir_all(&out_dir)?;

    let Ok(key) = std::env::var("ARBITRUM_FAST_FEED_KEY") else {
        eprintln!(
            "ARBITRUM_FAST_FEED_KEY is not set. It is the raw API key whose Keccak-256 hash was used to buy a\nticket; see docs/runbooks/fast-feed-experiment.md. Nothing was connected."
        );
        std::process::exit(2);
    };
    let standard_url = cfg
        .sequencer_feed
        .clone()
        .context("config has no sequencer_feed")?;
    let fast_url = cfg.pga.fast_feed.url.clone();
    println!(
        "standard feed {standard_url}\nfast feed     {fast_url}\nrunning {secs}s; output in {}",
        out_dir.display()
    );

    let (std_tx, mut std_rx) = mpsc::unbounded_channel::<mev_sequencer::SequencedTx>();
    tokio::spawn(mev_sequencer::run(standard_url, std_tx));
    let (fast_tx, mut fast_rx) = mpsc::unbounded_channel::<FastFeedTx>();
    let (sample_tx, mut sample_rx) = mpsc::unbounded_channel::<String>();
    let stats = Arc::new(FastFeedStats::default());
    tokio::spawn(fast_feed::run_with_samples(
        fast_url,
        key,
        fast_tx,
        stats.clone(),
        Some(sample_tx),
    ));

    let mut samples = std::fs::File::create(out_dir.join("fast_feed_samples.jsonl"))?;
    let mut lead_file = std::fs::File::create(out_dir.join("lead.jsonl"))?;
    let (mut tracker, mut agg) = (LeadTracker::new(PAIR_WINDOW), LeadStats::default());
    // Block / index / round of Fast Feed messages, kept until their pair arrives.
    let mut meta: HashMap<B256, (u64, u64, Option<u64>)> = HashMap::new();
    let (mut n_std, mut n_fast, mut written) = (0u64, 0u64, 0usize);
    let started = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            Some(st) = std_rx.recv() => {
                n_std += 1;
                if let Some(s) = tracker.observe(st.hash, Feed::Standard, st.received) {
                    record(&mut lead_file, &mut agg, &mut meta, &s)?;
                }
            }
            Some(f) = fast_rx.recv() => {
                n_fast += 1;
                if meta.len() > 100_000 { meta.clear(); }
                meta.insert(f.hash, (f.block_number, f.tx_index, f.pga_round));
                if let Some(s) = tracker.observe(f.hash, Feed::Fast, f.received) {
                    record(&mut lead_file, &mut agg, &mut meta, &s)?;
                }
            }
            Some(raw) = sample_rx.recv() => {
                if written < SAMPLE_LIMIT {
                    writeln!(samples, "{raw}")?;
                    written += 1;
                }
            }
            _ = tick.tick() => {
                tracker.expire(Instant::now());
                match agg.summary() {
                    Some(s) => println!(
                        "[{:>4}s] standard {n_std} fast {n_fast} paired {} | fast first {:.0}% median lead {:+.0} ms (p10 {:+.0}, p90 {:+.0}) | fast-only {} standard-only {}",
                        started.elapsed().as_secs(), s.n, s.fast_first * 100.0, s.median, s.p10, s.p90, tracker.only_fast, tracker.only_standard),
                    None => println!("[{:>4}s] standard {n_std} fast {n_fast} paired 0 (waiting for both feeds)", started.elapsed().as_secs()),
                }
            }
            _ = &mut deadline => break,
        }
    }
    tracker.expire(Instant::now() + PAIR_WINDOW);
    let summary = agg.summary();
    let doc = json!({
        "seconds": started.elapsed().as_secs(),
        "standard_msgs": n_std, "fast_msgs": n_fast,
        "fast_only": tracker.only_fast, "standard_only": tracker.only_standard,
        "fast_gaps": stats.gaps.load(Relaxed), "fast_missing": stats.missing.load(Relaxed),
        "fast_reconnects": stats.reconnects.load(Relaxed), "fast_decode_errors": stats.decode_errors.load(Relaxed),
        "paired": summary.as_ref().map(|s| json!({
            "n": s.n, "fast_first": s.fast_first, "p10_ms": s.p10, "median_ms": s.median,
            "p90_ms": s.p90, "mean_ms": s.mean, "share_ge_125ms": s.ge_one_round,
            "premise_holds": s.premise_holds() })),
    });
    std::fs::write(
        out_dir.join("summary.json"),
        serde_json::to_string_pretty(&doc)?,
    )?;
    println!("\n{}", serde_json::to_string_pretty(&doc)?);
    match summary {
        Some(s) if s.premise_holds() => println!(
            "\nVERDICT: the Fast Feed leads by a median {:+.0} ms and is first for {:.0}% of transactions. The premise holds.",
            s.median,
            s.fast_first * 100.0
        ),
        Some(s) => println!(
            "\nVERDICT: median lead {:+.0} ms, first for {:.0}%. Below the decision rule (median >= 100 ms and first for > 50%): the Fast Feed alone does not explain the timing losses.",
            s.median,
            s.fast_first * 100.0
        ),
        None => println!(
            "\nVERDICT: no transaction was seen on both feeds. Check the ticket is active for this round and the key is the raw key."
        ),
    }
    Ok(())
}

fn record(
    file: &mut std::fs::File,
    agg: &mut LeadStats,
    meta: &mut HashMap<B256, (u64, u64, Option<u64>)>,
    s: &mev_ingest::lead::LeadSample,
) -> Result<()> {
    agg.add(s);
    let m = meta.remove(&s.hash);
    writeln!(
        file,
        "{}",
        json!({ "hash": s.hash, "first": format!("{:?}", s.first), "lead_ms": s.lead_ms,
                "fast_advantage_ms": s.fast_advantage_ms(),
                "block": m.map(|m| m.0), "tx_index": m.map(|m| m.1), "pga_round": m.and_then(|m| m.2) })
    )?;
    Ok(())
}
