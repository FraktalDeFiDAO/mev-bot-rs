//! In-process simulation of sequencer-feed transactions: revm over a state cache that fills
//! itself from RPC.
//!
//! Stage 1 (measurement): a cache miss returns zero and is recorded, so the result is flagged
//! incomplete and the caller fetches the missing keys in the background — the next transaction
//! touching the same contracts runs entirely from memory. Entries older than `ttl_blocks` are still
//! used but reported as stale. Whether stale state is accurate enough is measured against
//! `eth_simulateV1` by `mev-localsim-probe` before anything trades on it.

pub mod diff;
pub mod worker;

use std::{
    collections::HashMap,
    convert::Infallible,
    time::{Duration, Instant},
};

use alloy::{
    consensus::{Transaction, TxEnvelope, transaction::SignerRecoverable},
    primitives::{Address, B256, Bytes, U256, keccak256},
    providers::{DynProvider, Provider},
};
use revm::{
    Context, Database, ExecuteEvm, MainBuilder, MainContext,
    bytecode::Bytecode,
    context::{TxEnv, result::ExecutionResult},
    primitives::TxKind,
    state::AccountInfo,
};

/// A piece of state the EVM read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Account(Address),
    Slot(Address, U256),
}

#[derive(Debug, Clone)]
struct Entry<T> {
    value: T,
    block: u64,
}

/// Accounts (with code) and storage slots, each stamped with the block it was read at.
#[derive(Debug, Default)]
pub struct StateCache {
    accounts: HashMap<Address, Entry<AccountInfo>>,
    storage: HashMap<(Address, U256), Entry<U256>>,
    codes: HashMap<B256, Bytecode>,
    /// Latest block number known to the caller.
    pub head: u64,
    /// Entries older than this many blocks are reported as stale.
    pub ttl_blocks: u64,
    /// Last block whose state diff has been applied (diff-fed mode): the cache is exact as of it.
    pub applied: u64,
    /// Changes of the most recent applied blocks, replayed onto keys fetched at an older block.
    recent: std::collections::VecDeque<(u64, Vec<diff::Change>)>,
}

/// Blocks of changes kept for replaying onto late fills (~16 s of Arbitrum blocks).
const RECENT_BLOCKS: usize = 64;

impl StateCache {
    pub fn new(ttl_blocks: u64) -> Self {
        Self {
            ttl_blocks,
            ..Default::default()
        }
    }

    pub fn len(&self) -> (usize, usize) {
        (self.accounts.len(), self.storage.len())
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.storage.is_empty()
    }

    pub fn insert_account(
        &mut self,
        addr: Address,
        balance: U256,
        nonce: u64,
        code: Bytes,
        block: u64,
    ) {
        let bytecode = Bytecode::new_raw(code.clone());
        let code_hash = if code.is_empty() {
            revm::primitives::KECCAK_EMPTY
        } else {
            keccak256(&code)
        };
        self.codes.insert(code_hash, bytecode.clone());
        let info = AccountInfo {
            balance,
            nonce,
            code_hash,
            code: Some(bytecode),
            ..Default::default()
        };
        self.accounts.insert(addr, Entry { value: info, block });
    }

    pub fn insert_slot(&mut self, addr: Address, slot: U256, value: U256, block: u64) {
        self.storage.insert((addr, slot), Entry { value, block });
    }

    fn stale(&self, block: u64) -> bool {
        self.head.saturating_sub(block) > self.ttl_blocks
    }
}

/// Read-only view of the cache for one execution; records what was missing or stale.
struct SnapshotDb<'a> {
    cache: &'a StateCache,
    overlay: Option<&'a Overlay>,
    missing: Vec<Key>,
    stale: Vec<Key>,
}

impl Database for SnapshotDb<'_> {
    type Error = Infallible;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Infallible> {
        if let Some(o) = self.overlay
            && let Some(acc) = o.accounts.get(&address)
        {
            return Ok(acc.clone());
        }
        match self.cache.accounts.get(&address) {
            Some(e) => {
                if self.cache.stale(e.block) {
                    self.stale.push(Key::Account(address));
                }
                Ok(Some(e.value.clone()))
            }
            // The beneficiary and system addresses (precompiles, Arbitrum's ArbSys & co. at
            // 0x64..) are not state to fetch.
            None if is_system(address) => Ok(None),
            None => {
                self.missing.push(Key::Account(address));
                Ok(None)
            }
        }
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Infallible> {
        if let Some(c) = self.overlay.and_then(|o| o.codes.get(&code_hash)) {
            return Ok(c.clone());
        }
        Ok(self
            .cache
            .codes
            .get(&code_hash)
            .cloned()
            .unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Infallible> {
        if let Some(o) = self.overlay {
            if let Some(v) = o.storage.get(&(address, index)) {
                return Ok(*v);
            }
            if o.destroyed.contains(&address) {
                return Ok(U256::ZERO);
            }
        }
        match self.cache.storage.get(&(address, index)) {
            Some(e) => {
                if self.cache.stale(e.block) {
                    self.stale.push(Key::Slot(address, index));
                }
                Ok(e.value)
            }
            None => {
                self.missing.push(Key::Slot(address, index));
                Ok(U256::ZERO)
            }
        }
    }

    fn block_hash(&mut self, _number: u64) -> Result<B256, Infallible> {
        Ok(B256::ZERO)
    }
}

/// Zero address and the low system range (`<= 0xffff`).
fn is_system(a: Address) -> bool {
    a.0[..18].iter().all(|b| *b == 0)
}

/// Logs the transaction emitted, as (address, topics, data).
pub type Logs = Vec<(Address, Vec<B256>, Bytes)>;

#[derive(Debug, Clone)]
pub struct Outcome {
    /// Every key the EVM read was in the cache (stale or not).
    pub complete: bool,
    pub success: bool,
    pub logs: Logs,
    pub missing: Vec<Key>,
    pub stale: Vec<Key>,
    pub elapsed: Duration,
}

/// Execute `tx` (as its signer) on the cached state at block `applied + 1`, `timestamp`.
pub fn simulate(cache: &StateCache, tx: &TxEnvelope, timestamp: u64) -> anyhow::Result<Outcome> {
    execute(cache, None, tx, timestamp).map(|(o, _)| o)
}

/// Execute `tx` on base state plus `overlay` (pending txs); also returns the state it touched.
pub fn execute(
    cache: &StateCache,
    overlay: Option<&Overlay>,
    tx: &TxEnvelope,
    timestamp: u64,
) -> anyhow::Result<(Outcome, revm::state::EvmState)> {
    let start = Instant::now();
    let from = tx.recover_signer()?;
    let db = SnapshotDb {
        cache,
        overlay,
        missing: Vec::new(),
        stale: Vec::new(),
    };
    let mut evm = Context::mainnet()
        .with_db(db)
        .modify_cfg_chained(|c| {
            c.chain_id = 42161;
            c.tx_chain_id_check = false;
            c.disable_nonce_check = true;
            c.disable_balance_check = true;
            c.disable_base_fee = true;
            c.disable_block_gas_limit = true;
        })
        .modify_block_chained(|b| {
            b.number = U256::from(cache.applied.max(cache.head) + 1);
            b.timestamp = U256::from(timestamp);
            b.basefee = 0;
            b.gas_limit = u64::MAX / 2;
        })
        .build_mainnet();
    let kind = match tx.to() {
        Some(to) => TxKind::Call(to),
        None => TxKind::Create,
    };
    let env = TxEnv::builder()
        .caller(from)
        .kind(kind)
        .data(tx.input().clone())
        .value(tx.value())
        .gas_limit(tx.gas_limit())
        .gas_price(0)
        .chain_id(tx.chain_id())
        .build_fill();
    let result = evm.transact(env);
    let db = &mut evm.ctx.journaled_state.database;
    let (missing, stale) = (
        std::mem::take(&mut db.missing),
        std::mem::take(&mut db.stale),
    );
    let (success, logs, state) = match result {
        Ok(r) => match r.result {
            ExecutionResult::Success { logs, .. } => (
                true,
                logs.into_iter()
                    .map(|l| (l.address, l.data.topics().to_vec(), l.data.data))
                    .collect(),
                r.state,
            ),
            _ => (false, Vec::new(), r.state),
        },
        Err(_) => (false, Vec::new(), Default::default()),
    };
    Ok((
        Outcome {
            complete: missing.is_empty(),
            success,
            logs,
            missing: dedup(missing),
            stale: dedup(stale),
            elapsed: start.elapsed(),
        },
        state,
    ))
}

/// Writes of locally executed pending (sequenced, not yet in an applied block) transactions, on
/// top of the base. Reads check it first.
#[derive(Debug, Default, Clone)]
pub struct Overlay {
    /// `None` = destroyed.
    accounts: HashMap<Address, Option<AccountInfo>>,
    storage: HashMap<(Address, U256), U256>,
    codes: HashMap<B256, Bytecode>,
    destroyed: std::collections::HashSet<Address>,
}

impl Overlay {
    /// Fold a transaction's written state in.
    pub fn commit(&mut self, state: &revm::state::EvmState) {
        for (addr, acc) in state {
            if !acc.is_touched() {
                continue;
            }
            if acc.is_selfdestructed() {
                self.accounts.insert(*addr, None);
                self.destroyed.insert(*addr);
                self.storage.retain(|(a, _), _| a != addr);
                continue;
            }
            let mut info = acc.info.clone();
            if let Some(code) = &info.code {
                self.codes.insert(info.code_hash, code.clone());
            }
            info.code = None; // resolved through code_by_hash
            self.accounts.insert(*addr, Some(info));
            for (k, slot) in &acc.storage {
                if slot.is_changed() {
                    self.storage.insert((*addr, *k), slot.present_value);
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.storage.is_empty()
    }
}

/// Sequenced transactions not yet covered by an applied block, executed in order into an overlay:
/// cache + overlay tracks the sequencer's live state instead of the last block.
#[derive(Debug, Default)]
pub struct Pending {
    txs: std::collections::VecDeque<(B256, TxEnvelope, u64)>,
    pub overlay: Overlay,
    /// Pending txs whose effects are missing from the overlay (incomplete locally).
    pub gaps: usize,
}

/// More pending txs than this means blocks aren't being applied; the oldest are dropped.
const MAX_PENDING: usize = 512;

impl Pending {
    /// Execute a newly sequenced tx on base + overlay and commit its writes if it ran from complete
    /// state. Returns the outcome (its logs are the post-state prediction).
    pub fn push(
        &mut self,
        cache: &StateCache,
        hash: B256,
        tx: TxEnvelope,
        timestamp: u64,
    ) -> anyhow::Result<Outcome> {
        let (out, state) = execute(cache, Some(&self.overlay), &tx, timestamp)?;
        if out.complete {
            self.overlay.commit(&state);
        } else {
            self.gaps += 1;
        }
        self.txs.push_back((hash, tx, timestamp));
        while self.txs.len() > MAX_PENDING {
            self.txs.pop_front();
        }
        Ok(out)
    }

    /// A block was applied to the base: drop the txs it contained and rebuild the overlay from
    /// the rest, re-executed in order on the new base.
    pub fn on_block(&mut self, cache: &StateCache, confirmed: &std::collections::HashSet<B256>) {
        self.txs.retain(|(h, _, _)| !confirmed.contains(h));
        self.overlay = Overlay::default();
        self.gaps = 0;
        let txs: Vec<_> = self.txs.iter().cloned().collect();
        for (_, tx, ts) in &txs {
            match execute(cache, Some(&self.overlay), tx, *ts) {
                Ok((out, state)) if out.complete => self.overlay.commit(&state),
                _ => self.gaps += 1,
            }
        }
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
}

fn dedup(mut v: Vec<Key>) -> Vec<Key> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|k| seen.insert(*k));
    v
}

/// Fetch `keys` as of `block` (so they are consistent with a cache applied up to it). Keys
/// that fail are left out.
pub async fn fill(p: &DynProvider, keys: &[Key], block: u64) -> Vec<(Key, Fetched)> {
    use futures::StreamExt;
    // Bounded fan-out: a burst of reads gets free endpoints to rate-limit the whole IP.
    futures::stream::iter(keys.to_vec())
        .map(|k| {
            let p = p.clone();
            async move { fetch_one(&p, k, block).await.ok().map(|f| (k, f)) }
        })
        .buffer_unordered(FILL_CONCURRENCY)
        .filter_map(|r| async move { r })
        .collect()
        .await
}

async fn fetch_one(p: &DynProvider, k: Key, block: u64) -> anyhow::Result<Fetched> {
    let at: alloy::eips::BlockId = block.into();
    Ok(match k {
        Key::Account(a) => {
            let (bal, nonce, code) = tokio::try_join!(
                p.get_balance(a).block_id(at).into_future(),
                p.get_transaction_count(a).block_id(at).into_future(),
                p.get_code_at(a).block_id(at).into_future(),
            )?;
            Fetched::Account(bal, nonce, code)
        }
        Key::Slot(a, s) => Fetched::Slot(p.get_storage_at(a, s).block_id(at).await?),
    })
}

/// Concurrent state reads per fill batch.
const FILL_CONCURRENCY: usize = 16;

/// A fetched value for a `Key`.
#[derive(Debug, Clone)]
pub enum Fetched {
    Account(U256, u64, Bytes),
    Slot(U256),
}

impl StateCache {
    /// Insert values fetched at `block`, then replay the changes of later applied blocks that
    /// touch them, so they match the rest of the cache (exact as of `applied`).
    pub fn apply(&mut self, fetched: Vec<(Key, Fetched)>, block: u64) {
        let keys: std::collections::HashSet<Key> = fetched.iter().map(|(k, _)| *k).collect();
        self.insert_fetched(fetched, block);
        let later: Vec<diff::Change> = self
            .recent
            .iter()
            .filter(|(n, _)| *n > block)
            .flat_map(|(_, cs)| cs.iter().filter(|c| touches(c, &keys)).cloned())
            .collect();
        let at = self.applied.max(block);
        for c in &later {
            self.apply_change(c, at);
        }
    }

    fn insert_fetched(&mut self, fetched: Vec<(Key, Fetched)>, block: u64) {
        for (k, f) in fetched {
            match (k, f) {
                (Key::Account(a), Fetched::Account(bal, nonce, code)) => {
                    self.insert_account(a, bal, nonce, code, block)
                }
                (Key::Slot(a, s), Fetched::Slot(v)) => self.insert_slot(a, s, v, block),
                _ => {}
            }
        }
    }
}

impl StateCache {
    /// Apply a block's state changes. Only state already cached is updated (we track what our
    /// simulations need, not the whole chain); a destroyed account is dropped.
    /// Apply one change to cached state only. Returns whether anything cached changed.
    fn apply_change(&mut self, c: &diff::Change, block: u64) -> bool {
        match c {
            diff::Change::Balance(a, v) => self.accounts.get_mut(a).map(|e| {
                e.value.balance = *v;
                e.block = block;
            }),
            diff::Change::Nonce(a, v) => self.accounts.get_mut(a).map(|e| {
                e.value.nonce = *v;
                e.block = block;
            }),
            diff::Change::Code(a, code) => {
                let prev = self
                    .accounts
                    .get(a)
                    .map(|e| (e.value.balance, e.value.nonce));
                prev.map(|(bal, nonce)| self.insert_account(*a, bal, nonce, code.clone(), block))
            }
            diff::Change::Slot(a, k, v) => self.storage.get_mut(&(*a, *k)).map(|e| {
                e.value = *v;
                e.block = block;
            }),
            // A destroyed account's storage goes too, even if only its slots were cached.
            diff::Change::Destroyed(a) => {
                let had_account = self.accounts.remove(a).is_some();
                let before = self.storage.len();
                self.storage.retain(|(addr, _), _| addr != a);
                (had_account || self.storage.len() != before).then_some(())
            }
        }
        .is_some()
    }

    /// Apply a block's state changes. Only state already cached is updated (we track what our
    /// simulations need, not the whole chain); a destroyed account is dropped.
    pub fn apply_changes(&mut self, changes: &[diff::Change], block: u64) -> usize {
        let applied = changes
            .iter()
            .filter(|c| self.apply_change(c, block))
            .count();
        // Everything cached is now known as of `block` (unchanged entries are still correct).
        for e in self.accounts.values_mut() {
            e.block = block;
        }
        for e in self.storage.values_mut() {
            e.block = block;
        }
        applied
    }

    /// A block's diff couldn't be fetched: everything cached may now be wrong. Start over (cold)
    /// as of block `n` rather than simulate on silently inconsistent state.
    pub fn reset(&mut self, n: u64) {
        self.accounts.clear();
        self.storage.clear();
        self.recent.clear();
        self.applied = n;
        self.head = self.head.max(n);
    }

    /// Diff-fed mode: apply block `n` (blocks must be applied in order) and remember its changes.
    pub fn apply_block(&mut self, n: u64, changes: Vec<diff::Change>) -> usize {
        let applied = self.apply_changes(&changes, n);
        self.recent.push_back((n, changes));
        while self.recent.len() > RECENT_BLOCKS {
            self.recent.pop_front();
        }
        self.applied = n;
        self.head = self.head.max(n);
        applied
    }
}

fn touches(c: &diff::Change, keys: &std::collections::HashSet<Key>) -> bool {
    match c {
        diff::Change::Balance(a, _) | diff::Change::Nonce(a, _) | diff::Change::Code(a, _) => {
            keys.contains(&Key::Account(*a))
        }
        diff::Change::Slot(a, k, _) => keys.contains(&Key::Slot(*a, *k)),
        diff::Change::Destroyed(a) => keys.iter().any(|k| match k {
            Key::Account(x) | Key::Slot(x, _) => x == a,
        }),
    }
}

/// Same logs, compared as (address, topics, data) multisets in order.
pub fn same_logs(a: &Logs, b: &Logs) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        consensus::{SignableTransaction, TxEip1559},
        network::TxSignerSync,
        signers::local::PrivateKeySigner,
    };

    fn signed(to: Address, input: Vec<u8>) -> TxEnvelope {
        let signer = PrivateKeySigner::random();
        let mut t = TxEip1559 {
            chain_id: 42161,
            to: TxKind::Call(to),
            input: input.into(),
            gas_limit: 200_000,
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut t).unwrap();
        TxEnvelope::Eip1559(t.into_signed(sig))
    }

    /// A contract that emits LOG1(topic = 0x2a) and stops: PUSH1 0x2a PUSH1 0 PUSH1 0 LOG1 STOP.
    const LOGGER: [u8; 9] = [0x60, 0x2a, 0x60, 0x00, 0x60, 0x00, 0xa1, 0x00, 0x00];

    #[test]
    fn executes_from_cache_and_reports_misses() {
        let target = Address::repeat_byte(0x42); // outside the system range (<= 0xffff)
        let tx = signed(target, vec![]);
        let mut cache = StateCache::new(4);
        cache.head = 100;

        // Nothing cached: the target and the sender are missing.
        let first = simulate(&cache, &tx, 1_700_000_000).unwrap();
        assert!(!first.complete);
        assert!(first.missing.contains(&Key::Account(target)));

        // Cache the code and the sender: complete, and the log comes out.
        let from = tx.recover_signer().unwrap();
        cache.insert_account(target, U256::ZERO, 0, LOGGER.to_vec().into(), 100);
        cache.insert_account(from, U256::ZERO, 0, Bytes::new(), 100);
        let second = simulate(&cache, &tx, 1_700_000_000).unwrap();
        assert!(second.complete, "missing: {:?}", second.missing);
        assert!(second.success);
        assert_eq!(second.logs.len(), 1);
        assert_eq!(second.logs[0].0, target);
        assert_eq!(second.logs[0].1, vec![B256::with_last_byte(0x2a)]);
        assert!(second.stale.is_empty());

        // Five blocks later the same entries are stale but still used.
        cache.head = 105;
        let third = simulate(&cache, &tx, 1_700_000_000).unwrap();
        assert!(third.complete && third.stale.contains(&Key::Account(target)));
    }

    #[test]
    fn late_fill_replays_newer_block_changes() {
        let pool = Address::repeat_byte(0x77);
        let slot = U256::from(4);
        let mut cache = StateCache::new(0);
        // Blocks 11 and 12 applied while the slot wasn't cached yet: nothing to update then.
        cache.apply_block(11, vec![diff::Change::Slot(pool, slot, U256::from(111))]);
        cache.apply_block(12, vec![diff::Change::Slot(pool, slot, U256::from(222))]);
        assert_eq!(cache.applied, 12);
        // A fill pinned at block 10 lands late: block 11 and 12 changes are replayed onto it.
        cache.apply(
            vec![(Key::Slot(pool, slot), Fetched::Slot(U256::from(100)))],
            10,
        );
        let mut db = SnapshotDb {
            cache: &cache,
            overlay: None,
            missing: vec![],
            stale: vec![],
        };
        assert_eq!(db.storage(pool, slot).unwrap(), U256::from(222));
        // A fill at the applied block needs no replay; a cleared slot reads zero after destruction.
        cache.apply_block(13, vec![diff::Change::Destroyed(pool)]);
        let mut db = SnapshotDb {
            cache: &cache,
            overlay: None,
            missing: vec![],
            stale: vec![],
        };
        assert_eq!(db.storage(pool, slot).unwrap(), U256::ZERO);
        assert_eq!(
            db.missing,
            vec![Key::Slot(pool, slot)],
            "destroyed state must be refetched"
        );
    }

    /// slot0 += 1; LOG1(topic = new value). Bytes:
    /// PUSH1 0 SLOAD PUSH1 1 ADD DUP1 PUSH1 0 SSTORE PUSH1 0 PUSH1 0 LOG1 STOP
    const COUNTER: [u8; 16] = [
        0x60, 0x00, 0x54, 0x60, 0x01, 0x01, 0x80, 0x60, 0x00, 0x55, 0x60, 0x00, 0x60, 0x00, 0xa1,
        0x00,
    ];

    #[test]
    fn pending_txs_chain_through_the_overlay_and_rebuild_on_block() {
        let counter = Address::repeat_byte(0x33);
        let (t1, t2) = (signed(counter, vec![]), signed(counter, vec![]));
        let mut cache = StateCache::new(u64::MAX);
        cache.applied = 50;
        cache.insert_account(counter, U256::ZERO, 0, COUNTER.to_vec().into(), 50);
        cache.insert_slot(counter, U256::ZERO, U256::ZERO, 50);
        for t in [&t1, &t2] {
            let from = t.recover_signer().unwrap();
            cache.insert_account(from, U256::ZERO, 0, Bytes::new(), 50);
        }
        let topic = |o: &Outcome| o.logs[0].1[0];

        let mut p = Pending::default();
        let o1 = p.push(&cache, B256::with_last_byte(1), t1, 0).unwrap();
        let o2 = p.push(&cache, B256::with_last_byte(2), t2, 0).unwrap();
        assert_eq!(topic(&o1), B256::with_last_byte(1));
        assert_eq!(
            topic(&o2),
            B256::with_last_byte(2),
            "second tx sees the first's write"
        );
        // The base is untouched by pending execution.
        assert_eq!(
            simulate(&cache, &signed(counter, vec![]), 0).unwrap().logs[0].1[0],
            B256::with_last_byte(1)
        );

        // Block 51 contains tx 1 (slot0 = 1): overlay rebuilt from tx 2 on the new base.
        cache.apply_block(
            51,
            vec![diff::Change::Slot(counter, U256::ZERO, U256::from(1))],
        );
        p.on_block(&cache, &[B256::with_last_byte(1)].into_iter().collect());
        assert_eq!(p.len(), 1);
        assert_eq!(p.gaps, 0);
        let after = execute(&cache, Some(&p.overlay), &signed(counter, vec![]), 0)
            .unwrap()
            .0;
        assert_eq!(
            topic(&after),
            B256::with_last_byte(3),
            "base 1 + pending tx 2 + this one"
        );
    }
}
