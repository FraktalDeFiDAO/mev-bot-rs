//! Where transactions come from: the live feed, a recorded file replayed at a chosen pace, or nothing (the API alone).

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use seqscope_core::Tx;
use tokio::task::JoinHandle;

use crate::AppState;

#[derive(Debug, Clone)]
pub enum Source {
    /// The sequencer feed at this URL.
    Feed(String),
    /// A recording made by `seqscope record`, replayed forever (so demos and tests need no network). `speed` 1 is the
    /// recorded pace, 10 ten times faster, 0 as fast as possible.
    Replay { file: PathBuf, speed: f64 },
    /// No transactions: only the API over an existing database.
    Idle,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn spawn(st: Arc<AppState>, source: Source) -> Vec<JoinHandle<()>> {
    let mut tasks = Vec::new();
    // Every second: tell WebSocket clients how the collector is doing, and pick up edits made to the rules file.
    let ticker = st.clone();
    tasks.push(tokio::spawn(async move {
        let mut last = 0u64;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            if ticker.file_changed() {
                ticker.reload_rules_file();
            }
            let txs = ticker.txs.load(std::sync::atomic::Ordering::Relaxed);
            let msg = ticker.status(Duration::from_secs(1), last);
            last = txs;
            let _ = ticker.rows_tx.send(msg.to_string().into());
        }
    }));
    match source {
        Source::Idle => {}
        Source::Feed(url) => {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tasks.push(tokio::spawn(mev_sequencer::run(url, tx)));
            tasks.push(tokio::spawn(async move {
                while let Some(st_tx) = rx.recv().await {
                    st.feed(Tx::from_sequenced(&st_tx));
                }
            }));
        }
        Source::Replay { file, speed } => {
            tasks.push(tokio::spawn(async move {
                let text = match tokio::fs::read_to_string(&file).await {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("replay: cannot read {}: {e}", file.display());
                        return;
                    }
                };
                let txs: Vec<Tx> = text
                    .lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect();
                if txs.is_empty() {
                    eprintln!("replay: {} holds no transactions", file.display());
                    return;
                }
                loop {
                    let (start, t0) = (Instant::now(), txs[0].t_ms);
                    for (i, t) in txs.iter().enumerate() {
                        if speed > 0.0 {
                            let due = Duration::from_millis(
                                (t.t_ms.saturating_sub(t0) as f64 / speed) as u64,
                            );
                            if let Some(wait) = due.checked_sub(start.elapsed()) {
                                tokio::time::sleep(wait).await;
                            }
                        } else if i % 64 == 0 {
                            tokio::task::yield_now().await;
                        }
                        let mut t = t.clone();
                        t.t_ms = now_ms();
                        st.feed(t);
                    }
                }
            }));
        }
    }
    tasks
}
