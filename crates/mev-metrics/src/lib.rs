//! Event log (JSONL, one file per day) and running counters for PnL and funnel stats.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Mutex,
};

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Default, Clone, Serialize)]
pub struct Counters {
    pub updates: u64,
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
}

pub struct Metrics {
    dir: PathBuf,
    file: Mutex<(String, BufWriter<File>)>,
    pub counters: Mutex<Counters>,
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
        })
    }

    /// Append one event. Never fails the caller — logging errors are reported via tracing.
    pub fn event(&self, kind: &str, data: Value) {
        let now = Utc::now();
        let line = json!({ "ts": now.to_rfc3339(), "kind": kind, "data": data });
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
