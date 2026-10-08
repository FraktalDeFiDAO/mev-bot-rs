//! Keep the relevant feed transactions on disk for later analysis (revert rates, who bids what, decoder accuracy).
//!
//! One JSON line per transaction in `feedtx-YYYY-MM-DD.jsonl`, written by a background thread so the feed path never
//! waits on the disk: lines are handed over through a bounded channel and counted as dropped if it is full. Files
//! older than `keep_days` are deleted when a new day's file is opened.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};

use chrono::{NaiveDate, Utc};

pub struct TxRecorder {
    tx: SyncSender<String>,
    dropped: AtomicU64,
}

fn file_name(day: NaiveDate) -> String {
    format!("feedtx-{}.jsonl", day.format("%Y-%m-%d"))
}

/// Delete `feedtx-*.jsonl` files more than `keep_days` older than `today`. Returns how many were removed.
pub fn prune(dir: &Path, today: NaiveDate, keep_days: i64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(day) = name
            .strip_prefix("feedtx-")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if (today - day).num_days() > keep_days && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

impl TxRecorder {
    /// Start recording into `dir` (created if missing).
    pub fn open(dir: PathBuf, keep_days: i64) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(&dir)?;
        let (tx, rx) = sync_channel::<String>(16_384);
        std::thread::Builder::new()
            .name("feedtx-writer".into())
            .spawn(move || {
                let mut current: Option<(NaiveDate, std::io::BufWriter<std::fs::File>)> = None;
                let mut since_flush = 0u32;
                loop {
                    match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(line) => {
                            let today = Utc::now().date_naive();
                            if current.as_ref().is_none_or(|(d, _)| *d != today) {
                                if let Some((_, w)) = current.as_mut() {
                                    let _ = w.flush();
                                }
                                prune(&dir, today, keep_days);
                                current = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(dir.join(file_name(today)))
                                    .ok()
                                    .map(|f| (today, std::io::BufWriter::new(f)));
                            }
                            if let Some((_, w)) = current.as_mut() {
                                let _ = writeln!(w, "{line}");
                                since_flush += 1;
                                if since_flush >= 256 {
                                    let _ = w.flush();
                                    since_flush = 0;
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if let Some((_, w)) = current.as_mut() {
                                let _ = w.flush();
                            }
                            since_flush = 0;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            if let Some((_, w)) = current.as_mut() {
                                let _ = w.flush();
                            }
                            return;
                        }
                    }
                }
            })?;
        Ok(Arc::new(Self {
            tx,
            dropped: AtomicU64::new(0),
        }))
    }

    /// Queue one record. Never blocks: a full queue drops the line and counts it.
    pub fn record(&self, v: &serde_json::Value) {
        if self.tx.try_send(v.to_string()).is_err() {
            self.dropped.fetch_add(1, Relaxed);
        }
    }

    /// Lines dropped because the writer fell behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("feedtx-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn lines_reach_todays_file() {
        let dir = tmp("write");
        let r = TxRecorder::open(dir.clone(), 7).unwrap();
        for i in 0..3 {
            r.record(&serde_json::json!({ "i": i }));
        }
        let path = dir.join(file_name(Utc::now().date_naive()));
        let mut text = String::new();
        for _ in 0..40 {
            text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.lines().count() == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(text.contains("{\"i\":2}"));
        assert_eq!(r.dropped(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_files_are_pruned_and_others_left_alone() {
        let dir = tmp("prune");
        std::fs::create_dir_all(&dir).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        for d in ["2026-09-30", "2026-10-01", "2026-10-02", "2026-10-08"] {
            std::fs::write(dir.join(format!("feedtx-{d}.jsonl")), "x").unwrap();
        }
        std::fs::write(dir.join("events-2026-09-01.jsonl"), "x").unwrap();
        std::fs::write(dir.join("feedtx-notadate.jsonl"), "x").unwrap();
        assert_eq!(
            prune(&dir, today, 7),
            1,
            "09-30 is 8 days old; 10-01 is exactly 7 and stays"
        );
        assert!(dir.join("feedtx-2026-10-01.jsonl").exists());
        assert!(!dir.join("feedtx-2026-09-30.jsonl").exists());
        assert!(
            dir.join("events-2026-09-01.jsonl").exists(),
            "only our own files"
        );
        assert!(dir.join("feedtx-notadate.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
