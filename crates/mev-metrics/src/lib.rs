//! Event log (JSONL, one file per day) and running counters for PnL and funnel stats.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
    },
    time::Duration,
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

/// Events queued for the writer thread before new ones are dropped. A burst of a few thousand events
/// fits; a stalled disk costs diagnostics, never the hot path (invariant 4).
const QUEUE: usize = 16_384;

enum Msg {
    /// `(day, json line)`.
    Line(String, String),
    Heartbeat(String),
    /// Written and flushed up to here; reply when done.
    Flush(SyncSender<()>),
}

pub struct Metrics {
    tx: SyncSender<Msg>,
    /// Events dropped because the queue was full (or the writer is gone).
    dropped: Arc<AtomicU64>,
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
        let (tx, rx) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let seen = dropped.clone();
        std::thread::Builder::new()
            .name("metrics-writer".into())
            .spawn(move || writer(dir, day, file, rx, seen))?;
        Ok(Self {
            tx,
            dropped,
            counters: Mutex::new(Counters::default()),
            live: AtomicBool::new(false),
        })
    }

    /// Events dropped so far because the writer could not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait until everything queued so far is on disk (shutdown, tests). Never call on the hot path.
    pub fn flush(&self) {
        let (done_tx, done_rx) = sync_channel(1);
        if self.tx.send(Msg::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv_timeout(Duration::from_secs(5));
        }
    }

    /// Set the mode stamped on later events; true if it changed.
    pub fn set_live(&self, live: bool) -> bool {
        self.live.swap(live, Ordering::Relaxed) != live
    }

    /// Queue one event for the writer thread. Never blocks and never fails the caller: if the queue is
    /// full the event is dropped and counted (`dropped()`).
    pub fn event(&self, kind: &str, data: Value) {
        let now = Utc::now();
        let line = json!({ "ts": now.to_rfc3339(), "mode": if self.live.load(Ordering::Relaxed) { "live" } else { "shadow" }, "kind": kind, "data": data }).to_string();
        // Error strings can embed RPC URLs with API keys.
        let line = mev_types::redact_secrets(&line).into_owned();
        let day = now.format("%Y-%m-%d").to_string();
        if self.tx.try_send(Msg::Line(day, line)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut Counters)) {
        f(&mut self.counters.lock().unwrap());
    }

    /// Touch `heartbeat` (unix seconds) so a container healthcheck can tell the loop is alive.
    /// The write happens on the writer thread.
    pub fn heartbeat(&self) {
        let now = Utc::now().timestamp().to_string();
        if self.tx.try_send(Msg::Heartbeat(now)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> Counters {
        self.counters.lock().unwrap().clone()
    }
}

/// Owns the event file: appends queued lines (rotating by day), flushes when the queue goes idle or a
/// flush is requested, writes the heartbeat, and reports drops. Exits when every sender is gone.
fn writer(
    dir: PathBuf,
    mut day: String,
    mut file: BufWriter<File>,
    rx: Receiver<Msg>,
    dropped: Arc<AtomicU64>,
) {
    let mut reported = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        // Block for the next message; once the queue is empty, flush what was buffered.
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(TryRecvError::Empty) => {
                if let Err(e) = file.flush() {
                    tracing::error!(error = %e, "flushing metrics failed");
                }
                match rx.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                }
            }
            Err(TryRecvError::Disconnected) => {
                let _ = file.flush();
                return;
            }
        };
        match msg {
            Msg::Line(d, line) => {
                if d != day {
                    match open(&dir, &d) {
                        Ok(f) => {
                            let _ = file.flush();
                            (day, file) = (d, f);
                        }
                        Err(e) => tracing::error!(error = %e, "rotating metrics file failed"),
                    }
                }
                if let Err(e) = writeln!(file, "{line}") {
                    tracing::error!(error = %e, "writing metrics failed");
                }
            }
            Msg::Heartbeat(ts) => {
                if let Err(e) = std::fs::write(dir.join("heartbeat"), ts) {
                    tracing::error!(error = %e, "writing heartbeat failed");
                }
            }
            Msg::Flush(done) => {
                let _ = file.flush();
                let _ = done.send(());
            }
        }
        let d = dropped.load(Ordering::Relaxed);
        if d > reported && last_report.elapsed() >= Duration::from_secs(10) {
            tracing::warn!(
                dropped = d,
                "metrics events dropped: the writer cannot keep up"
            );
            (reported, last_report) = (d, std::time::Instant::now());
        }
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
        m.flush();
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
        m.flush();
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
        m.flush();

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

    #[test]
    fn a_full_queue_drops_events_without_blocking_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let m = Metrics::new(dir.path().to_path_buf()).unwrap();
        // Far more events than the queue holds, as fast as possible: the calls must all return.
        let t = std::time::Instant::now();
        for i in 0..(QUEUE * 4) {
            m.event("burst", json!({ "i": i }));
        }
        assert!(
            // A blocking queue would hang for as long as the disk is slow; this only needs to tell "returns" from
            // "hangs", and a debug build under a loaded test run is slow (11 s seen with the 10 s bound).
            t.elapsed() < Duration::from_secs(60),
            "event() must not wait on the disk"
        );
        m.flush();
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let written = std::fs::read_to_string(dir.path().join(format!("events-{day}.jsonl")))
            .unwrap()
            .lines()
            .count() as u64;
        assert_eq!(
            written + m.dropped(),
            (QUEUE * 4) as u64,
            "every event is written or counted as dropped"
        );
    }

    #[test]
    fn events_of_a_new_day_go_to_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let m = Metrics::new(dir.path().to_path_buf()).unwrap();
        m.tx.send(Msg::Line("2001-01-01".into(), "{\"x\":1}".into()))
            .unwrap();
        m.flush();
        assert!(dir.path().join("events-2001-01-01.jsonl").exists());
    }
}
