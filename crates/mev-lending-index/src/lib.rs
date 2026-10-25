//! Event-scan account index, shared by lending adapters.
//!
//! Finds accounts from protocol events (e.g. Aave `Borrow`, Comet `Withdraw`, Morpho `Borrow`):
//! scans new blocks forward from the head and history backward (newest first, so recently active
//! accounts are found first), shrinks ranges when a response is too large, throttles requests,
//! and persists progress to a JSON cache so restarts resume instead of rescanning.
//!
//! Durability and failure handling (JEV-LIQ-004): the cache is written atomically (temp file,
//! fsync, rename, directory fsync); a range that still fails after bounded retries with backoff is
//! recorded as a coverage gap and retried on later steps instead of aborting the step; block-hash
//! ancestry detects reorgs and rewinds the forward cursor to the common ancestor.

pub mod background;
pub mod scanner;

pub use background::BackgroundIndex;

use std::{
    collections::BTreeSet,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use alloy::{
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::Log,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

pub use scanner::{
    Fetched, LogQuery, Reorg, RetryPolicy, fetch_range, head_with_retry, locate_reorg,
};

/// (market, account) pairs. `market` is a 32-byte id: a market contract address left-padded
/// (`Address::into_word`) for Aave/Comet-style venues, or the native market id (Morpho `bytes32`).
pub type Key = (B256, Address);

/// Recorded scan points kept for reorg detection.
const RING_KEEP: usize = 64;
/// Stop recording new gaps in one step after this many (a dead provider would otherwise turn the
/// whole history into gaps in a single call).
const MAX_GAPS_PER_STEP: usize = 3;
/// After this many consecutive full-size successful requests, a shrunken step doubles back toward
/// the configured one. Without it one transient error shrank the step for good: a step of a few
/// blocks then spent every request keeping up with the head and the backfill stalled (Moonwell
/// on Base, 2026-10-16). A real provider limit costs one refused request per growth attempt.
const GROW_AFTER: u32 = 8;

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

/// A block range whose logs could not be fetched. Open until a later step fetches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub from: u64,
    pub to: u64,
    pub backward: bool,
    pub reason: String,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Next block to scan going forward (0 = not initialized).
    next_block: u64,
    /// History below this block is not scanned yet.
    back_block: u64,
    accounts: BTreeSet<Key>,
    /// Open coverage gaps.
    #[serde(default)]
    gaps: Vec<Gap>,
    /// Newest-first `(block, hash)` points of forward scanning, for reorg detection.
    #[serde(default)]
    ring: Vec<(u64, B256)>,
}

pub struct EventIndex {
    cfg: IndexConfig,
    state: State,
    retry: RetryPolicy,
    /// The configured step: the most a shrunken step grows back to.
    step_cap: u64,
    /// Consecutive successful requests at the current step.
    streak: u32,
}

/// Where an interrupted [`atomic_write`] stops (crash injection for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStage {
    AfterTempWrite,
    AfterTempSync,
    AfterRename,
}

/// Replace `path` with `bytes` so a crash leaves either the old or the new content, never a mix:
/// write a sibling temp file, fsync it, rename over the target, fsync the directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_stages(path, bytes, None)
}

fn write_stages(path: &Path, bytes: &[u8], stop_after: Option<WriteStage>) -> std::io::Result<()> {
    let crash = |s| {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            format!("injected crash {s:?}"),
        ))
    };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(d) = dir {
        std::fs::create_dir_all(d)?;
    }
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("cache path has no file name"))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    if stop_after == Some(WriteStage::AfterTempWrite) {
        return crash(WriteStage::AfterTempWrite);
    }
    f.sync_all()?;
    drop(f);
    if stop_after == Some(WriteStage::AfterTempSync) {
        return crash(WriteStage::AfterTempSync);
    }
    std::fs::rename(&tmp, path)?;
    if stop_after == Some(WriteStage::AfterRename) {
        return crash(WriteStage::AfterRename);
    }
    File::open(dir.unwrap_or_else(|| Path::new(".")))?.sync_all()?;
    Ok(())
}

impl EventIndex {
    /// Load progress from the cache (or start fresh). Caches written with address-typed market
    /// keys (pre-B256) are migrated in place.
    pub fn open(cfg: IndexConfig) -> Self {
        let state = std::fs::read(&cfg.cache_path)
            .ok()
            .and_then(|b| parse_state(&b))
            .unwrap_or_default();
        info!(
            index = cfg.name,
            accounts = state.accounts.len(),
            gaps = state.gaps.len(),
            "account index loaded"
        );
        Self {
            step_cap: cfg.step.max(1),
            streak: 0,
            cfg,
            state,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
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

    /// Drop an account (e.g. its debt is zero on chain). It returns if a later scan finds it
    /// again. Returns true if it was present.
    pub fn remove(&mut self, key: &Key) -> bool {
        self.state.accounts.remove(key)
    }

    pub fn backfilled(&self) -> bool {
        self.state.next_block != 0
            && self.state.back_block <= self.cfg.from_block
            && self.state.gaps.is_empty()
    }

    /// Ranges that could not be fetched and are still open.
    pub fn gaps(&self) -> &[Gap] {
        &self.state.gaps
    }

    /// Persist now (atomic). `step` does this itself.
    pub fn flush(&self) -> Result<()> {
        self.save()
    }

    /// Scan new blocks, then retry open gaps, then a bounded slice of history. `extract` maps a
    /// log to accounts. Returns true once history is complete. Progress made before an error is
    /// always persisted; ranges that fail after bounded retries become gaps, not errors.
    pub async fn step(
        &mut self,
        p: &DynProvider,
        extract: impl Fn(&Log) -> Vec<Key>,
    ) -> Result<bool> {
        let work = self.step_inner(p, &extract).await;
        let saved = self.save();
        work?;
        saved?;
        let done = self.backfilled();
        info!(
            index = self.cfg.name,
            accounts = self.state.accounts.len(),
            history_left_blocks = self.state.back_block.saturating_sub(self.cfg.from_block),
            open_gaps = self.state.gaps.len(),
            done,
            "index step"
        );
        Ok(done)
    }

    async fn step_inner(
        &mut self,
        p: &DynProvider,
        extract: &impl Fn(&Log) -> Vec<Key>,
    ) -> Result<()> {
        let head = scanner::head_with_retry(p, &self.retry).await?;
        let mut advanced = false;
        if self.state.next_block == 0 {
            self.state.next_block = head + 1;
            self.state.back_block = head + 1;
            advanced = true; // record the starting head as the first ancestry point
        }
        self.rewind_on_reorg(p).await;
        let mut budget = self.cfg.max_requests;
        let mut new_gaps = 0usize;
        while self.state.next_block <= head && budget > 0 && new_gaps < MAX_GAPS_PER_STEP {
            let step = self.cfg.step.max(1);
            let from = self.state.next_block;
            match self
                .fetch(p, from, (from + step - 1).min(head), false, &mut budget)
                .await
            {
                Fetched::Logs { logs, to, .. } => {
                    for l in &logs {
                        self.state.accounts.extend(extract(l));
                    }
                    self.state.next_block = to + 1;
                    advanced = true;
                }
                Fetched::Gap { from, to, reason } => {
                    self.state.gaps.push(Gap {
                        from,
                        to,
                        backward: false,
                        reason,
                    });
                    self.state.next_block = to + 1;
                    new_gaps += 1;
                    advanced = true;
                }
                Fetched::Deferred => break,
            }
        }
        if advanced {
            self.record_tip(p).await;
        }
        self.retry_gaps(p, extract, &mut budget).await;
        while self.state.back_block > self.cfg.from_block
            && budget > 0
            && new_gaps < MAX_GAPS_PER_STEP
        {
            let step = self.cfg.step.max(1);
            let to = self.state.back_block - 1;
            let from = to.saturating_sub(step - 1).max(self.cfg.from_block);
            match self.fetch(p, from, to, true, &mut budget).await {
                Fetched::Logs { logs, from, .. } => {
                    for l in &logs {
                        self.state.accounts.extend(extract(l));
                    }
                    self.state.back_block = from;
                }
                Fetched::Gap { from, to, reason } => {
                    self.state.gaps.push(Gap {
                        from,
                        to,
                        backward: true,
                        reason,
                    });
                    self.state.back_block = from;
                    new_gaps += 1;
                }
                Fetched::Deferred => break,
            }
        }
        Ok(())
    }

    async fn fetch(
        &mut self,
        p: &DynProvider,
        from: u64,
        to: u64,
        backward: bool,
        budget: &mut usize,
    ) -> Fetched {
        let q = LogQuery {
            addresses: &self.cfg.addresses,
            topics: &self.cfg.topics,
            name: self.cfg.name,
        };
        let mut step = self.cfg.step;
        let r = fetch_range(
            p,
            &q,
            from,
            to,
            backward,
            &mut step,
            self.cfg.throttle,
            &self.retry,
            budget,
        )
        .await;
        if step < self.cfg.step || !matches!(r, Fetched::Logs { .. }) {
            self.streak = 0;
        } else {
            self.streak += 1;
            if self.streak >= GROW_AFTER && step < self.step_cap {
                step = (step * 2).min(self.step_cap);
                self.streak = 0;
            }
        }
        self.cfg.step = step;
        r
    }

    /// Re-fetch open gaps oldest first. A fetched part shrinks the gap; the rest stays open.
    async fn retry_gaps(
        &mut self,
        p: &DynProvider,
        extract: &impl Fn(&Log) -> Vec<Key>,
        budget: &mut usize,
    ) {
        let mut i = 0;
        while i < self.state.gaps.len() && *budget > 0 {
            let g = self.state.gaps[i].clone();
            match self.fetch(p, g.from, g.to, g.backward, budget).await {
                Fetched::Logs { logs, from, to } => {
                    for l in &logs {
                        self.state.accounts.extend(extract(l));
                    }
                    let gap = &mut self.state.gaps[i];
                    if from == gap.from && to == gap.to {
                        self.state.gaps.remove(i);
                        continue;
                    }
                    if gap.backward {
                        gap.to = from.saturating_sub(1).max(gap.from);
                    } else {
                        gap.from = to + 1;
                    }
                    // Remaining part is retried on the next step (budget spent on this one).
                    i += 1;
                }
                Fetched::Gap { reason, .. } => {
                    self.state.gaps[i].reason = reason;
                    i += 1;
                }
                Fetched::Deferred => break,
            }
        }
    }

    /// Compare the newest recorded scan point with the canonical chain; on mismatch rewind the
    /// forward cursor to the common ancestor so the orphaned range is scanned again. Accounts
    /// found on the dropped fork are kept (the set is a superset; a recheck sorts them out).
    async fn rewind_on_reorg(&mut self, p: &DynProvider) {
        match locate_reorg(p, &self.state.ring).await {
            Ok(Reorg::Consistent) => {}
            Ok(Reorg::Unverified { height }) => {
                warn!(
                    index = self.cfg.name,
                    height, "provider lacks the newest scanned block; reorg check skipped"
                );
            }
            Ok(Reorg::Rewind { ancestor }) => {
                warn!(
                    index = self.cfg.name,
                    ancestor = ancestor.0,
                    was = self.state.next_block,
                    "reorg: rewinding index cursor"
                );
                self.state.ring.retain(|(n, _)| *n <= ancestor.0);
                self.state.next_block = ancestor.0 + 1;
            }
            Err(e) => {
                warn!(index = self.cfg.name, error = %format!("{e:#}"), "reorg check failed; will retry next step");
            }
        }
    }

    async fn record_tip(&mut self, p: &DynProvider) {
        let n = self.state.next_block.saturating_sub(1);
        if let Ok(Some(b)) = p.get_block_by_number(n.into()).await {
            self.state.ring.retain(|(m, _)| *m != n);
            self.state.ring.insert(0, (n, b.header.hash));
            self.state.ring.truncate(RING_KEEP);
        }
    }

    fn save(&self) -> Result<()> {
        atomic_write(&self.cfg.cache_path, &serde_json::to_vec(&self.state)?)?;
        Ok(())
    }
}

fn parse_state(bytes: &[u8]) -> Option<State> {
    if let Ok(s) = serde_json::from_slice::<State>(bytes) {
        return Some(s);
    }
    #[derive(Deserialize)]
    struct AddressKeyed {
        next_block: u64,
        back_block: u64,
        accounts: BTreeSet<(Address, Address)>,
    }
    let old: AddressKeyed = serde_json::from_slice(bytes).ok()?;
    Some(State {
        next_block: old.next_block,
        back_block: old.back_block,
        accounts: old
            .accounts
            .into_iter()
            .map(|(m, a)| (m.into_word(), a))
            .collect(),
        gaps: vec![],
        ring: vec![],
    })
}

/// Market key for venues whose markets are contracts.
pub fn market_key(address: Address) -> B256 {
    address.into_word()
}

/// Address from an indexed topic.
pub fn topic_address(log: &Log, i: usize) -> Option<Address> {
    log.topics().get(i).map(|t| Address::from_word(*t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_limits_below_a_thousand_blocks_shrink_and_are_remembered() {
        use alloy::{
            providers::{Provider, ProviderBuilder},
            transports::mock::Asserter,
        };
        let mock = Asserter::new();
        mock.push_success(&"0x64");
        // The head block lookup that records the first reorg ancestry point (best effort).
        mock.push_failure_msg("block lookup unavailable");
        // Each range is retried once as is before it shrinks (transient errors), so a real
        // limit costs two refusals per shrink.
        for _ in 0..4 {
            mock.push_failure_msg("block range exceeds provider limit");
        }
        mock.push_success(&Vec::<Log>::new());
        let provider = ProviderBuilder::new()
            .connect_mocked_client(mock.clone())
            .erased();
        let dir = tempfile::tempdir().unwrap();
        let mut config = cfg(dir.path().join("index.json"));
        config.from_block = 0;
        config.step = 100;
        config.max_requests = 5;
        let mut index = EventIndex::open(config);
        assert!(!index.step(&provider, |_| vec![]).await.unwrap());
        assert!(index.cfg.step < 10);
        assert!(index.state.back_block < 100);
        assert!(mock.read_q().is_empty());
    }

    #[tokio::test]
    async fn one_transient_error_is_retried_without_shrinking_the_step() {
        use alloy::{
            providers::{Provider, ProviderBuilder},
            transports::mock::Asserter,
        };
        let mock = Asserter::new();
        mock.push_failure_msg("HTTP error 400 with empty body");
        mock.push_success(&Vec::<Log>::new());
        let provider = ProviderBuilder::new()
            .connect_mocked_client(mock.clone())
            .erased();
        let dir = tempfile::tempdir().unwrap();
        let mut index = EventIndex::open(cfg(dir.path().join("index.json")));
        let mut budget = 5;
        let got = index.fetch(&provider, 100, 109, false, &mut budget).await;
        assert!(matches!(
            got,
            Fetched::Logs {
                from: 100,
                to: 109,
                ..
            }
        ));
        assert_eq!(index.cfg.step, 10);
        assert_eq!(budget, 3);
    }

    #[tokio::test]
    async fn a_shrunken_step_grows_back_after_a_run_of_successes() {
        use alloy::{
            providers::{Provider, ProviderBuilder},
            transports::mock::Asserter,
        };
        let mock = Asserter::new();
        for _ in 0..40 {
            mock.push_success(&Vec::<Log>::new());
        }
        let provider = ProviderBuilder::new()
            .connect_mocked_client(mock.clone())
            .erased();
        let dir = tempfile::tempdir().unwrap();
        let mut index = EventIndex::open(cfg(dir.path().join("index.json")));
        index.step_cap = 1_000;
        index.cfg.step = 10; // as if a transient error had shrunk it
        let mut budget = 40;
        for k in 0..(2 * GROW_AFTER) {
            index.fetch(&provider, 0, 9, false, &mut budget).await;
            if k + 1 == GROW_AFTER {
                assert_eq!(index.cfg.step, 20, "doubles after {GROW_AFTER} successes");
            }
        }
        assert_eq!(index.cfg.step, 40);
    }

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
        idx.insert((
            market_key(Address::with_last_byte(1)),
            Address::with_last_byte(2),
        ));
        idx.state.next_block = 500;
        idx.state.back_block = 100;
        idx.save().unwrap();

        let idx2 = EventIndex::open(cfg(path));
        assert_eq!(idx2.len(), 1);
        assert!(idx2.backfilled());
    }

    #[test]
    fn address_keyed_cache_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        let (m, a) = (Address::with_last_byte(0xaa), Address::with_last_byte(1));
        let old = serde_json::json!({ "next_block": 9, "back_block": 100, "accounts": [[m, a]] });
        std::fs::write(&path, old.to_string()).unwrap();
        let idx = EventIndex::open(cfg(path));
        assert_eq!(idx.accounts().next(), Some(&(market_key(m), a)));
        assert!(idx.backfilled());
    }

    #[test]
    fn interrupted_write_never_exposes_a_torn_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        let mut idx = EventIndex::open(cfg(path.clone()));
        idx.insert((
            market_key(Address::with_last_byte(1)),
            Address::with_last_byte(2),
        ));
        idx.state.next_block = 10;
        idx.state.back_block = 100;
        idx.save().unwrap();
        let old = std::fs::read(&path).unwrap();

        idx.insert((
            market_key(Address::with_last_byte(1)),
            Address::with_last_byte(3),
        ));
        let new = serde_json::to_vec(&idx.state).unwrap();
        for stage in [WriteStage::AfterTempWrite, WriteStage::AfterTempSync] {
            assert!(write_stages(&path, &new, Some(stage)).is_err());
            assert_eq!(
                std::fs::read(&path).unwrap(),
                old,
                "{stage:?}: old content intact"
            );
            // A leftover temp file does not confuse loading.
            assert_eq!(EventIndex::open(cfg(path.clone())).len(), 1);
        }
        assert!(write_stages(&path, &new, Some(WriteStage::AfterRename)).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            new,
            "rename is the commit point"
        );
        assert_eq!(EventIndex::open(cfg(path.clone())).len(), 2);
        atomic_write(&path, &old).unwrap();
        assert_eq!(EventIndex::open(cfg(path)).len(), 1);
    }

    #[test]
    fn removed_accounts_stay_removed_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        let key = (
            market_key(Address::with_last_byte(1)),
            Address::with_last_byte(2),
        );
        let mut idx = EventIndex::open(cfg(path.clone()));
        idx.insert(key);
        idx.state.next_block = 1;
        idx.save().unwrap();
        assert!(idx.remove(&key));
        assert!(!idx.remove(&key));
        idx.save().unwrap();
        assert!(EventIndex::open(cfg(path)).is_empty());
    }

    #[test]
    fn corrupt_cache_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idx.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(EventIndex::open(cfg(path)).is_empty());
    }
}
