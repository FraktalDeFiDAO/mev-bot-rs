//! Event log (JSONL, one file per day) and running counters for PnL and funnel stats.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Default, Clone, Serialize)]
pub struct Counters {
    pub updates: u64,
    /// Pool updates learned from simulating feed txs (before their block).
    pub presim_updates: u64,
    pub candidates: u64,
    pub simulated: u64,
    pub sim_profitable: u64,
    pub sent: u64,
    pub landed_ok: u64,
    pub landed_reverted: u64,
    pub profit_usd: f64,
    pub gas_usd: f64,
    pub liq_flagged: u64,
    pub liq_planned: u64,
    pub liq_profitable: u64,
    pub feed_triggers: u64,
    /// Arbitrage sends held back by the per-route win-rate gate.
    pub gate_skipped: u64,
    /// Arbitrage candidates through a Uniswap V4 pool, logged but not sent until the executor has V4 hops.
    pub v4_candidates: u64,
    /// PGA bids: placed, skipped (policy or tip caps), won (landed ok), lost (reverted).
    pub bids_placed: u64,
    pub bids_skipped: u64,
    pub bids_won: u64,
    pub bids_lost: u64,
    /// Priority-fee spend committed at placement (USD) and the sum of bid/net fractions.
    pub tip_usd: f64,
    pub bid_fraction_sum: f64,
    /// Sends accepted within / after one PGA round (125 ms) of their trigger.
    pub round_within: u64,
    pub round_over: u64,
    /// Fast Feed: messages, sequence gaps, reconnects.
    pub fast_feed_msgs: u64,
    pub fast_feed_gaps: u64,
    pub fast_feed_reconnects: u64,
}

pub struct Metrics {
    dir: PathBuf,
    file: Mutex<(String, BufWriter<File>)>,
    pub counters: Mutex<Counters>,
    /// Whether the bot is sending real transactions; stamped on every event so live and shadow
    /// data share one history (`events-*.jsonl`) and can be told apart.
    live: AtomicBool,
}

impl Metrics {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let file = open(&dir, &day)?;
        Ok(Self {
            dir,
            file: Mutex::new((day, file)),
            counters: Mutex::new(Counters::default()),
            live: AtomicBool::new(false),
        })
    }

    /// Set the mode stamped on later events; true if it changed.
    pub fn set_live(&self, live: bool) -> bool {
        self.live.swap(live, Ordering::Relaxed) != live
    }

    /// Append one event. Never fails the caller — logging errors are reported via tracing.
    pub fn event(&self, kind: &str, data: Value) {
        let now = Utc::now();
        let line = json!({ "ts": now.to_rfc3339(), "mode": if self.live.load(Ordering::Relaxed) { "live" } else { "shadow" }, "kind": kind, "data": data }).to_string();
        // Error strings can embed RPC URLs with API keys.
        let line = mev_types::redact_secrets(&line);
        let mut guard = self.file.lock().unwrap();
        let day = now.format("%Y-%m-%d").to_string();
        if guard.0 != day {
            match open(&self.dir, &day) {
                Ok(f) => *guard = (day, f),
                Err(e) => tracing::error!(error = %e, "rotating metrics file failed"),
            }
        }
        if let Err(e) = writeln!(guard.1, "{line}").and_then(|_| guard.1.flush()) {
            tracing::error!(error = %e, "writing metrics failed");
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut Counters)) {
        f(&mut self.counters.lock().unwrap());
    }

    /// Touch `heartbeat` (unix seconds) so a container healthcheck can tell the loop is alive.
    pub fn heartbeat(&self) {
        let now = Utc::now().timestamp().to_string();
        if let Err(e) = std::fs::write(self.dir.join("heartbeat"), now) {
            tracing::error!(error = %e, "writing heartbeat failed");
        }
    }

    pub fn snapshot(&self) -> Counters {
        self.counters.lock().unwrap().clone()
    }
}

fn open(dir: &std::path::Path, day: &str) -> Result<BufWriter<File>> {
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("events-{day}.jsonl")))?;
    Ok(BufWriter::new(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_stamped_with_the_mode() {
        let dir = tempfile::tempdir().unwrap();
        let m = Metrics::new(dir.path().to_path_buf()).unwrap();
        m.event("a", json!({}));
        assert!(m.set_live(true));
        assert!(!m.set_live(true));
        m.event("b", json!({}));
        let day = Utc::now().format("%Y-%m-%d");
        let text = std::fs::read_to_string(dir.path().join(format!("events-{day}.jsonl"))).unwrap();
        let modes: Vec<String> = text
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["mode"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(modes, ["shadow", "live"]);
    }

    #[test]
    fn event_files_never_contain_rpc_keys() {
        let dir = tempfile::tempdir().unwrap();
        let m = Metrics::new(dir.path().to_path_buf()).unwrap();
        m.event(
            "liq_sim_revert",
            json!({ "reason": format!("error sending request for url (https://arb-mainnet.g.alchemy.com/v2/{})", ["SECRETKEY", "0123456789abc"].concat()) }),
        );
        let text: String = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
            .collect();
        assert!(!text.contains("SECRETKEY"), "{text}");
        assert!(text.contains("alchemy.com/v2/***"));
    }

    #[test]
    fn writes_jsonl_events_counters_and_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let m = Metrics::new(dir.path().to_path_buf()).unwrap();
        m.event("candidate", json!({ "x": 1 }));
        m.event("sent", json!({ "hash": "0xabc" }));
        m.update(|c| {
            c.sent += 2;
            c.profit_usd += 1.5;
        });
        m.heartbeat();

        let day = Utc::now().format("%Y-%m-%d").to_string();
        let text = std::fs::read_to_string(dir.path().join(format!("events-{day}.jsonl"))).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["kind"], "candidate");
        assert_eq!(lines[1]["data"]["hash"], "0xabc");
        let c = m.snapshot();
        assert_eq!((c.sent, c.profit_usd), (2, 1.5));
        let hb: i64 = std::fs::read_to_string(dir.path().join("heartbeat"))
            .unwrap()
            .parse()
            .unwrap();
        assert!((Utc::now().timestamp() - hb).abs() < 5);
    }
}
