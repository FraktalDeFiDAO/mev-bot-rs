//! Net PnL per strategy after priority fees, from the bot's event logs.
//!   cargo run --release -p mev-analytics --bin mev-pnl -- [data/arbitrum/events-*.jsonl ...]

use std::io::BufRead;

fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let files = if files.is_empty() {
        let mut v: Vec<String> = std::fs::read_dir("data/arbitrum")?
            .filter_map(|e| e.ok().map(|e| e.path().display().to_string()))
            .filter(|p| p.contains("events-") && p.ends_with(".jsonl"))
            .collect();
        v.sort();
        v
    } else {
        files
    };
    let mut events = Vec::new();
    for f in &files {
        for line in std::io::BufReader::new(std::fs::File::open(f)?).lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line?) {
                events.push(v);
            }
        }
    }
    println!("{} events from {} file(s)", events.len(), files.len());
    println!(
        "{:<12} {:>6} {:>6} {:>6} {:>5} {:>5} {:>10} {:>10} {:>10} {:>10}",
        "strategy",
        "bids",
        "skips",
        "landed",
        "won",
        "lost",
        "gross $",
        "gas+tip $",
        "tip bid $",
        "net $"
    );
    for (k, s) in mev_analytics::pnl::aggregate(events.iter()) {
        println!(
            "{:<12} {:>6} {:>6} {:>6} {:>5} {:>5} {:>10.4} {:>10.4} {:>10.4} {:>10.4}",
            k,
            s.bids,
            s.bids_skipped,
            s.landed,
            s.won,
            s.lost,
            s.gross_usd,
            s.gas_usd,
            s.tip_committed_usd,
            s.net_usd()
        );
    }
    Ok(())
}
