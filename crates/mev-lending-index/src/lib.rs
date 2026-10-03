//! Event-scan account index, shared by lending adapters.
//!
//! Finds accounts from protocol events (e.g. Aave `Borrow`, Comet `Withdraw`, Morpho `Borrow`):
//! scans new blocks forward from the head and history backward (newest first, so recently active
//! accounts are found first), shrinks ranges when a response is too large, throttles requests,
//! and persists progress to a JSON cache so restarts resume instead of rescanning.

use std::{collections::BTreeSet, path::PathBuf, time::Duration};

use alloy::{
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

/// (market, account) pairs; market = the contract the account lives in (pool, comet, morpho...).
pub type Key = (Address, Address);

#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Contracts emitting the events.
    pub addresses: Vec<Address>,
    /// Event signature hashes (topic0) to scan.
    pub topics: Vec<B256>,
    /// Never scan below this block (protocol deployment).
    pub from_block: u64,
    /// Initial blocks per request (shrunk automatically when responses are too big).
    pub step: u64,
    /// Max requests per `step()` call so one call never blocks for long.
    pub max_requests: usize,
    /// Pause between requests (public RPC rate limits).
    pub throttle: Duration,
    pub cache_path: PathBuf,
    /// Label for logs.
    pub name: &'static str,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Next block to scan going forward (0 = not initialized).
    next_block: u64,
    /// History below this block is not scanned yet.
    back_block: u64,
    accounts: BTreeSet<Key>,
}

pub struct EventIndex {
    cfg: IndexConfig,
    state: State,
}

impl EventIndex {
    /// Load progress from the cache (or start fresh).
    pub fn open(cfg: IndexConfig) -> Self {
        let state = std::fs::read(&cfg.cache_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .unwrap_or_default();
        info!(
            index = cfg.name,
            accounts = state.accounts.len(),
            "account index loaded"
        );
        Self { cfg, state }
    }

    pub fn accounts(&self) -> impl Iterator<Item = &Key> {
        self.state.accounts.iter()
    }

    pub fn len(&self) -> usize {
        self.state.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.accounts.is_empty()
    }

    pub fn insert(&mut self, key: Key) {
        self.state.accounts.insert(key);
    }

    pub fn backfilled(&self) -> bool {
        self.state.next_block != 0 && self.state.back_block <= self.cfg.from_block
    }

    /// Scan new blocks, then a bounded slice of history. `extract` maps a log to accounts.
    /// Returns true once history is complete.
    pub async fn step(
        &mut self,
        p: &DynProvider,
        extract: impl Fn(&Log) -> Vec<Key>,
    ) -> Result<bool> {
        let head = p.get_block_number().await?;
        if self.state.next_block == 0 {
            self.state.next_block = head + 1;
            self.state.back_block = head + 1;
        }
        let step = self.cfg.step.max(1);
        let mut requests = 0;
        while self.state.next_block <= head && requests < self.cfg.max_requests {
            let from = self.state.next_block;
            let (_, to) = self
                .scan(
                    p,
                    from,
                    (from + step - 1).min(head),
                    false,
                    &extract,
                    &mut requests,
                )
                .await?;
            self.state.next_block = to + 1;
        }
        while self.state.back_block > self.cfg.from_block && requests < self.cfg.max_requests {
            let to = self.state.back_block - 1;
            let from = to.saturating_sub(step - 1).max(self.cfg.from_block);
            let (from, _) = self
                .scan(p, from, to, true, &extract, &mut requests)
                .await?;
            self.state.back_block = from;
        }
        self.save()?;
        let done = self.backfilled();
        info!(
            index = self.cfg.name,
            accounts = self.state.accounts.len(),
            history_left_blocks = self.state.back_block.saturating_sub(self.cfg.from_block),
            done,
            "index step"
        );
        Ok(done)
    }

    async fn scan(
        &mut self,
        p: &DynProvider,
        mut from: u64,
        mut to: u64,
        backward: bool,
        extract: &impl Fn(&Log) -> Vec<Key>,
        requests: &mut usize,
    ) -> Result<(u64, u64)> {
        loop {
            *requests += 1;
            tokio::time::sleep(self.cfg.throttle).await;
            let filter = Filter::new()
                .address(self.cfg.addresses.clone())
                .event_signature(self.cfg.topics.clone())
                .from_block(from)
                .to_block(to);
            match p
                .get_logs(&filter)
                .await
                .with_context(|| format!("{} log scan", self.cfg.name))
            {
                Ok(logs) => {
                    for l in &logs {
                        self.state.accounts.extend(extract(l));
                    }
                    return Ok((from, to));
                }
                // Too many results / range too wide: shrink and retry. Rate limits bubble up.
                Err(e) if to > from + 1_000 && !mev_rpc::is_rate_limit(&e) => {
                    if backward {
                        from = to - (to - from) / 4;
                    } else {
                        to = from + (to - from) / 4;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.cfg.cache_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.cfg.cache_path, serde_json::to_vec(&self.state)?)?;
        Ok(())
    }
}

/// Address from an indexed topic.
pub fn topic_address(log: &Log, i: usize) -> Option<Address> {
    log.topics().get(i).map(|t| Address::from_word(*t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(path: PathBuf) -> IndexConfig {
        IndexConfig {
            addresses: vec![],
            topics: vec![],
            from_block: 100,
            step: 10,
            max_requests: 5,
            throttle: Duration::ZERO,
            cache_path: path,
            name: "test",
        }
    }

    #[test]
    fn cache_round_trips_and_backfill_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        let mut idx = EventIndex::open(cfg(path.clone()));
        assert!(idx.is_empty());
        assert!(!idx.backfilled(), "uninitialized index is not backfilled");
        idx.insert((Address::with_last_byte(1), Address::with_last_byte(2)));
        idx.state.next_block = 500;
        idx.state.back_block = 100;
        idx.save().unwrap();

        let idx2 = EventIndex::open(cfg(path));
        assert_eq!(idx2.len(), 1);
        assert!(idx2.backfilled());
    }

    #[test]
    fn corrupt_cache_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(EventIndex::open(cfg(path)).is_empty());
    }
}
