//! Aave v3 event dependency indexing (JEV-LIQ-004).
//!
//! `AaveTriggerIndexer` turns deterministic chain triggers into dirty marks and indexes in the
//! durable [`SnapshotRepository`]; nothing here decides, ranks or trades (task 005 consumes the
//! output). It reuses the shared range scanner of `mev-lending-index` (shrink, bounded retry with
//! `Backoff`, explicit gaps) and the engine's log-continuity semantics (removed logs, conflicting
//! block hashes, reordering) via [`LogGuard`].
//!
//! Triggers covered: Pool events (supply/withdraw/borrow/repay/liquidation/collateral toggles/
//! eMode/deficit), `ReserveDataUpdated` (rate and index moves => every holder of the reserve),
//! Pool upgrades (proxy `Upgraded` event and the EIP-1967 implementation slot via the task 002
//! verifier => `full_reverification` flag), oracle source events and price polling, reserve list
//! changes, newly discovered borrowers (live or backfilled), reorgs and coverage gaps.
//! Not covered: see the report in `docs/design/jev-liq-004-event-index.md`.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider},
    rpc::types::Log,
};
use anyhow::{Context, Result};
use mev_bindings::{IAaveOracle, IAavePool};
use mev_config::AaveConfig;
use mev_lending_index::{Fetched, LogQuery, Reorg, RetryPolicy, fetch_range, head_with_retry};
use mev_liq_state::{
    ChainPoint, Coalescer, DirtyReason, Effect, FLAG_FULL_REVERIFICATION, KIND_COLLATERAL,
    KIND_DEBT, NewGap, ReconcileOutcome, RepoError, RewindOutcome, ScanBatch, ScanCursor, ScanDir,
    SnapshotRepository,
};
use mev_rpc::{call, decode, multicall_at, snapshot_block};
use tracing::{info, warn};

use crate::{
    deployment::{compare_deployment, observe_deployment},
    events::{self, PoolEvent},
};

/// Stop recording fresh gaps in one step after this many.
const MAX_GAPS_PER_STEP: usize = 3;
/// Ancestry points consulted for reorg detection.
const ANCESTRY_WINDOW: usize = 64;
const MAX_RESERVE_ID_SLACK: usize = 16;

#[derive(Debug, Clone)]
pub struct TriggerConfig {
    /// Cursor/gap namespace in the repository.
    pub name: String,
    pub chain_id: u64,
    pub from_block: u64,
    pub step: u64,
    pub max_requests: usize,
    pub throttle: Duration,
    pub retry: RetryPolicy,
    /// Distinct borrower keys held while coalescing one range before escalating.
    pub coalesce_cap: usize,
    /// Marks written immediately per asset/all-dirty trigger; the rest is queued durably.
    pub fanout_limit: usize,
    /// Queue entries turned into marks per step.
    pub fanout_drain: usize,
    /// Borrowers whose dependency set is reconciled against chain state per step.
    pub reconcile_batch: usize,
    /// Minimum oracle price move (basis points of the last triggering price) that marks holders.
    pub price_min_move_bps: u64,
    /// Re-attempts of one range after the chain changed underneath the scan.
    pub max_reorg_retries: u32,
}

impl TriggerConfig {
    pub fn for_pool(cfg: &AaveConfig, chain_id: u64) -> Self {
        Self {
            name: "aave-v3-triggers".into(),
            chain_id,
            from_block: cfg.from_block,
            step: cfg.scan_step.max(1),
            max_requests: 20,
            throttle: Duration::from_millis(500),
            retry: RetryPolicy::default(),
            coalesce_cap: 10_000,
            fanout_limit: 500,
            fanout_drain: 500,
            reconcile_batch: 50,
            price_min_move_bps: 2,
            max_reorg_retries: 3,
        }
    }
}

/// What one `step()` did (counters, not a narrative).
#[derive(Debug, Default, Clone)]
pub struct StepReport {
    pub head: u64,
    pub events: usize,
    pub registered: usize,
    pub marked: usize,
    pub queued: usize,
    pub drained: usize,
    pub new_gaps: usize,
    pub healed_gaps: usize,
    pub rewound_to: Option<u64>,
    pub rewind: RewindOutcome,
    pub upgrade_flagged: bool,
    pub reconciled: usize,
    pub deactivated: usize,
    pub price_moves: usize,
    pub warnings: Vec<String>,
}

/// How a scanned log range is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Forward, in order: events drive dirty marks and dependency edits.
    Live,
    /// A forward gap fetched late: marks only; dependencies are flagged for reconciliation instead
    /// of edited (out-of-order edits could contradict newer ones).
    Replay,
    /// History: only discovers borrowers.
    Backfill,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    Removed,
    MissingIdentity,
    ConflictingBlock,
    Reordered,
}

/// The engine's `Continuity` semantics (`mev-engine/src/continuity.rs`) for a batch of logs from
/// one `eth_getLogs` response, plus conflicts against already recorded ancestry. The engine type
/// is crate-private and sits in a higher layer, so the rules are mirrored here.
pub struct LogGuard {
    known: HashMap<u64, B256>,
    last: Option<(u64, B256, u64)>,
}

impl LogGuard {
    pub fn new(known: impl IntoIterator<Item = (u64, B256)>) -> Self {
        Self {
            known: known.into_iter().collect(),
            last: None,
        }
    }

    pub fn accept(&mut self, log: &Log) -> Result<bool, Invalid> {
        if log.removed {
            return Err(Invalid::Removed);
        }
        let (Some(number), Some(hash), Some(index)) =
            (log.block_number, log.block_hash, log.log_index)
        else {
            return Err(Invalid::MissingIdentity);
        };
        if self.known.get(&number).is_some_and(|h| *h != hash) {
            return Err(Invalid::ConflictingBlock);
        }
        if let Some((prior, prior_hash, prior_index)) = self.last {
            if number == prior && hash != prior_hash {
                return Err(Invalid::ConflictingBlock);
            }
            if number < prior || (number == prior && index < prior_index) {
                return Err(Invalid::Reordered);
            }
            if number == prior && index == prior_index {
                return Ok(false);
            }
        }
        self.last = Some((number, hash, index));
        Ok(true)
    }
}

/// Pure mapping from one decoded event to repository effects. `co` collects borrower touches
/// (coalesced), `deps` the ordered dependency edits, `extra` pool-level effects, `found` the
/// borrowers a backfill discovered.
#[allow(clippy::too_many_arguments)]
pub fn apply_event(
    ev: &PoolEvent,
    at: ChainPoint,
    mode: Mode,
    co: &mut Coalescer,
    deps: &mut Vec<Effect>,
    extra: &mut Vec<Effect>,
    found: &mut BTreeSet<Address>,
) {
    if mode == Mode::Backfill {
        if let PoolEvent::Borrow { on_behalf_of, .. } = ev {
            found.insert(*on_behalf_of);
        }
        return;
    }
    let live = mode == Mode::Live;
    let reason = DirtyReason::BorrowerEvent;
    // (register, stale_deps): replayed events never edit deps, so they always flag them stale.
    let touch = |co: &mut Coalescer, who: Address, register: bool, stale: bool| {
        co.push_account(who, reason, at, register, stale || !live);
    };
    match ev {
        PoolEvent::Supply {
            reserve,
            on_behalf_of,
        } => {
            touch(co, *on_behalf_of, false, false);
            if live {
                deps.push(Effect::DepAdd {
                    borrower: *on_behalf_of,
                    asset: *reserve,
                    kind: KIND_COLLATERAL,
                });
            }
        }
        PoolEvent::Withdraw { user, .. } | PoolEvent::Repay { user, .. } => {
            touch(co, *user, false, true);
        }
        PoolEvent::Borrow {
            reserve,
            on_behalf_of,
        } => {
            touch(co, *on_behalf_of, true, false);
            if live {
                deps.push(Effect::DepAdd {
                    borrower: *on_behalf_of,
                    asset: *reserve,
                    kind: KIND_DEBT,
                });
            }
        }
        PoolEvent::Liquidation {
            user,
            liquidator,
            receive_a_token,
            ..
        } => {
            touch(co, *user, false, true);
            if *receive_a_token {
                touch(co, *liquidator, false, true);
            }
        }
        PoolEvent::CollateralEnabled { reserve, user } => {
            touch(co, *user, false, false);
            if live {
                deps.push(Effect::DepAdd {
                    borrower: *user,
                    asset: *reserve,
                    kind: KIND_COLLATERAL,
                });
            }
        }
        PoolEvent::CollateralDisabled { reserve, user } => {
            touch(co, *user, false, false);
            if live {
                deps.push(Effect::DepRemove {
                    borrower: *user,
                    asset: *reserve,
                    kind: KIND_COLLATERAL,
                });
            }
        }
        PoolEvent::EModeSet { user } => touch(co, *user, false, false),
        PoolEvent::DeficitCreated { user, .. } => touch(co, *user, false, true),
        PoolEvent::ReserveDataUpdated { reserve } => {
            co.push_asset(*reserve, DirtyReason::ReserveConfig, Some(at));
        }
        PoolEvent::Upgraded { implementation } => {
            extra.push(Effect::Flag {
                flag: FLAG_FULL_REVERIFICATION.to_string(),
                at: Some(at),
                detail: format!("Pool proxy Upgraded -> {implementation}"),
            });
            extra.push(Effect::AllDirty {
                reason: DirtyReason::ReserveConfig,
                at: Some(at),
            });
        }
        PoolEvent::AssetSourceUpdated { asset, .. } => {
            co.push_asset(*asset, DirtyReason::PriceMove, Some(at));
        }
        PoolEvent::FallbackOracleUpdated => extra.push(Effect::AllDirty {
            reason: DirtyReason::PriceMove,
            at: Some(at),
        }),
    }
}

/// Reverse-index dependencies from Aave's `UserConfigurationMap`: bit `2*id` = borrowing reserve
/// `id`, bit `2*id+1` = using it as collateral.
pub fn deps_from_user_config(mask: U256, ids: &[(u16, Address)]) -> Vec<(Address, u8)> {
    let mut out = vec![];
    for (id, asset) in ids {
        let (b, c) = (2 * usize::from(*id), 2 * usize::from(*id) + 1);
        let mut kind = 0u8;
        if mask.bit(b) {
            kind |= KIND_DEBT;
        }
        if mask.bit(c) {
            kind |= KIND_COLLATERAL;
        }
        if kind != 0 {
            out.push((*asset, kind));
        }
    }
    out
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[derive(Debug)]
enum Processed {
    /// Applied.
    Done,
    /// The chain changed under the scan (removed/conflicting logs); redo after a reorg check.
    ChainChanged(String),
    /// Unusable response; record the range as a gap.
    Bad(String),
}

pub struct AaveTriggerIndexer {
    cfg: TriggerConfig,
    aave: AaveConfig,
    scan: DynProvider,
    fast: DynProvider,
    repo: Arc<SnapshotRepository>,
    /// `(reserve id, asset)` from `getReserveAddressById`.
    reserves: Vec<(u16, Address)>,
    step_hint: u64,
}

impl AaveTriggerIndexer {
    pub async fn new(
        aave: AaveConfig,
        cfg: TriggerConfig,
        scan: DynProvider,
        fast: DynProvider,
        repo: Arc<SnapshotRepository>,
    ) -> Result<Self> {
        let mut me = Self::with_reserves(aave, cfg, scan, fast, repo, vec![]);
        me.reserves = me.fetch_reserves().await?;
        Ok(me)
    }

    /// Build with a known `(reserve id, asset)` list (no RPC); `step()` keeps it current.
    pub fn with_reserves(
        aave: AaveConfig,
        cfg: TriggerConfig,
        scan: DynProvider,
        fast: DynProvider,
        repo: Arc<SnapshotRepository>,
        reserves: Vec<(u16, Address)>,
    ) -> Self {
        Self {
            step_hint: cfg.step,
            cfg,
            aave,
            scan,
            fast,
            repo,
            reserves,
        }
    }

    pub fn repo(&self) -> &Arc<SnapshotRepository> {
        &self.repo
    }

    pub fn pool(&self) -> Address {
        self.aave.pool
    }

    pub fn reserves(&self) -> &[(u16, Address)] {
        &self.reserves
    }

    async fn fetch_reserves(&self) -> Result<Vec<(u16, Address)>> {
        let list = IAavePool::new(self.aave.pool, &self.fast)
            .getReservesList()
            .call()
            .await
            .context("getReservesList")?;
        let calls: Vec<_> = (0..list.len() + MAX_RESERVE_ID_SLACK)
            .map(|id| {
                call(
                    self.aave.pool,
                    IAavePool::getReserveAddressByIdCall { id: id as u16 },
                )
            })
            .collect();
        let res = mev_rpc::multicall(&self.fast, &calls, 200).await?;
        Ok(res
            .iter()
            .enumerate()
            .filter_map(|(id, r)| {
                decode::<IAavePool::getReserveAddressByIdCall>(r).map(|a| (id as u16, a))
            })
            .filter(|(_, a)| !a.is_zero())
            .collect())
    }

    /// Seed the registry from accounts the legacy `Borrow` index already knows (idempotent).
    /// They are marked `Discovered` and have no dependency set yet.
    pub fn import_known_borrowers(&self, borrowers: &[Address]) -> Result<usize, RepoError> {
        self.repo
            .register_borrowers(self.cfg.chain_id, self.aave.pool, borrowers, None, now_ns())
    }

    async fn canonical_point(&self, number: u64) -> Result<ChainPoint> {
        let b = self
            .scan
            .get_block_by_number(number.into())
            .await
            .with_context(|| format!("block {number}"))?
            .with_context(|| format!("block {number} not found"))?;
        Ok(ChainPoint {
            block_number: number,
            block_hash: b.header.hash,
        })
    }

    fn query_parts(&self) -> (Vec<Address>, Vec<B256>) {
        let mut addrs = vec![self.aave.pool];
        if self.aave.oracle != self.aave.pool {
            addrs.push(self.aave.oracle);
        }
        (addrs, events::topics())
    }

    /// One bounded pass: reorg check, upgrade check, new blocks, gap healing, history, fan-out
    /// drain, dependency reconciliation and a price poll. Errors in the auxiliary parts are
    /// reported in `warnings`; a failure to learn the head or to read/write the repository is an
    /// error (nothing is silently skipped).
    pub async fn step(&mut self) -> Result<StepReport> {
        let (chain_id, pool, name) = (self.cfg.chain_id, self.aave.pool, self.cfg.name.clone());
        let mut rep = StepReport::default();
        let head = head_with_retry(&self.scan, &self.cfg.retry).await?;
        rep.head = head;
        let head_pt = self.canonical_point(head).await?;
        let created = self.repo.scan_cursor(&name)?.is_none();
        self.repo.init_scan_cursor(
            &name,
            ScanCursor {
                next_block: head + 1,
                tip: Some(head_pt),
                back_block: head + 1,
            },
        )?;
        if created {
            let mut b = ScanBatch::new(&name, chain_id, pool, now_ns(), self.cfg.fanout_limit);
            b.ancestry.push(head_pt);
            self.repo.apply_scan(&b)?;
        }
        self.check_reorg(&mut rep).await?;
        self.check_upgrade(head_pt, &mut rep).await;
        self.check_reserves(head_pt, &mut rep).await;

        let mut budget = self.cfg.max_requests;
        self.forward(head, &mut budget, &mut rep).await?;
        self.heal_gaps(&mut budget, &mut rep).await?;
        self.backfill(&mut budget, &mut rep).await?;

        rep.drained = self
            .repo
            .drain_fanout(chain_id, pool, self.cfg.fanout_drain)?;
        if let Err(e) = self.reconcile(&mut rep).await {
            rep.warnings.push(format!("reconcile: {e:#}"));
        }
        if let Err(e) = self.poll_prices(&mut rep).await {
            rep.warnings.push(format!("price poll: {e:#}"));
        }
        for w in &rep.warnings {
            warn!(index = %name, "{w}");
        }
        info!(
            index = %name, head, events = rep.events, registered = rep.registered, marked = rep.marked,
            queued = rep.queued, new_gaps = rep.new_gaps, healed = rep.healed_gaps,
            rewound = ?rep.rewound_to, "trigger step"
        );
        Ok(rep)
    }

    async fn check_reorg(&self, rep: &mut StepReport) -> Result<()> {
        let ring: Vec<(u64, B256)> = self
            .repo
            .scan_ancestry(&self.cfg.name, ANCESTRY_WINDOW)?
            .iter()
            .map(|p| (p.block_number, p.block_hash))
            .collect();
        match mev_lending_index::locate_reorg(&self.scan, &ring)
            .await
            .context("reorg check")?
        {
            Reorg::Consistent => {}
            Reorg::Unverified { height } => rep.warnings.push(format!(
                "provider does not know scanned block {height}; reorg check skipped this step"
            )),
            Reorg::Rewind { ancestor } => {
                let out = self.repo.rewind_scan(
                    &self.cfg.name,
                    self.cfg.chain_id,
                    self.aave.pool,
                    ChainPoint {
                        block_number: ancestor.0,
                        block_hash: ancestor.1,
                    },
                    now_ns(),
                )?;
                warn!(index = %self.cfg.name, ancestor = ancestor.0, ?out, "reorg: cursor rewound");
                rep.rewound_to = Some(ancestor.0);
                rep.rewind = out;
            }
        }
        Ok(())
    }

    /// Compare the live deployment with the manifest (task 002 verifier). Any mismatch sets the
    /// `full_reverification` flag once and marks every active borrower for recheck.
    async fn check_upgrade(&self, head: ChainPoint, rep: &mut StepReport) {
        let Some(m) = self.aave.manifest.as_ref() else {
            return;
        };
        let (chain_id, pool) = (self.cfg.chain_id, self.aave.pool);
        match self.repo.has_flag(chain_id, pool, FLAG_FULL_REVERIFICATION) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => {
                rep.warnings.push(format!("flag read: {e}"));
                return;
            }
        }
        let observed = match observe_deployment(&self.fast, m).await {
            Ok(o) => o,
            Err(e) => {
                rep.warnings.push(format!("upgrade check: {e}"));
                return;
            }
        };
        if let Err(diff) = compare_deployment(&self.aave, &observed) {
            let mut b = ScanBatch::new(
                &self.cfg.name,
                chain_id,
                pool,
                now_ns(),
                self.cfg.fanout_limit,
            );
            b.effects.push(Effect::Flag {
                flag: FLAG_FULL_REVERIFICATION.to_string(),
                at: Some(head),
                detail: format!("deployment differs from manifest: {diff}"),
            });
            b.effects.push(Effect::AllDirty {
                reason: DirtyReason::ReserveConfig,
                at: Some(head),
            });
            match self.repo.apply_scan(&b) {
                Ok(o) => {
                    rep.upgrade_flagged = true;
                    rep.marked += o.marked;
                    rep.queued += o.queued;
                }
                Err(e) => rep.warnings.push(format!("flagging upgrade: {e}")),
            }
        }
    }

    /// A changed reserve list (new asset) invalidates cached dependency assumptions.
    async fn check_reserves(&mut self, head: ChainPoint, rep: &mut StepReport) {
        let fresh = match self.fetch_reserves().await {
            Ok(r) => r,
            Err(e) => {
                rep.warnings.push(format!("reserve list: {e:#}"));
                return;
            }
        };
        if fresh == self.reserves {
            return;
        }
        let added: Vec<Address> = fresh
            .iter()
            .filter(|(_, a)| !self.reserves.iter().any(|(_, b)| a == b))
            .map(|(_, a)| *a)
            .collect();
        self.reserves = fresh;
        if added.is_empty() {
            return;
        }
        let mut b = ScanBatch::new(
            &self.cfg.name,
            self.cfg.chain_id,
            self.aave.pool,
            now_ns(),
            self.cfg.fanout_limit,
        );
        b.effects.push(Effect::AllDirty {
            reason: DirtyReason::ReserveConfig,
            at: Some(head),
        });
        match self.repo.apply_scan(&b) {
            Ok(o) => {
                rep.marked += o.marked;
                rep.queued += o.queued;
            }
            Err(e) => rep.warnings.push(format!("new reserve: {e}")),
        }
    }

    async fn forward(&mut self, head: u64, budget: &mut usize, rep: &mut StepReport) -> Result<()> {
        let (addrs, topics) = self.query_parts();
        let mut gaps = 0;
        let mut reorg_retries = 0;
        loop {
            let cur = self
                .repo
                .scan_cursor(&self.cfg.name)?
                .context("cursor missing")?;
            if cur.next_block > head || *budget == 0 || gaps >= MAX_GAPS_PER_STEP {
                return Ok(());
            }
            let from = cur.next_block;
            let to = (from + self.step_hint.max(1) - 1).min(head);
            let q = LogQuery {
                addresses: &addrs,
                topics: &topics,
                name: &self.cfg.name,
            };
            let fetched = fetch_range(
                &self.scan,
                &q,
                from,
                to,
                false,
                &mut self.step_hint,
                self.cfg.throttle,
                &self.cfg.retry,
                budget,
            )
            .await;
            match fetched {
                Fetched::Logs { logs, from, to } => {
                    match self.process(&logs, from, to, Mode::Live, None, rep).await? {
                        Processed::Done => {}
                        Processed::ChainChanged(why) => {
                            if reorg_retries < self.cfg.max_reorg_retries {
                                reorg_retries += 1;
                                warn!(index = %self.cfg.name, %why, "chain changed during scan; re-checking ancestry");
                                self.check_reorg(rep).await?;
                                continue;
                            }
                            self.skip_as_gap(from, to, format!("chain kept changing: {why}"), rep)
                                .await?;
                            gaps += 1;
                        }
                        Processed::Bad(why) => {
                            self.skip_as_gap(from, to, why, rep).await?;
                            gaps += 1;
                        }
                    }
                }
                Fetched::Gap { from, to, reason } => {
                    self.skip_as_gap(from, to, reason, rep).await?;
                    gaps += 1;
                }
                Fetched::Deferred => return Ok(()),
            }
        }
    }

    /// Record `[from, to]` as an open gap and advance the forward cursor past it, atomically.
    async fn skip_as_gap(
        &self,
        from: u64,
        to: u64,
        reason: String,
        rep: &mut StepReport,
    ) -> Result<()> {
        let tip = self.canonical_point(to).await.ok();
        let mut b = ScanBatch::new(
            &self.cfg.name,
            self.cfg.chain_id,
            self.aave.pool,
            now_ns(),
            self.cfg.fanout_limit,
        );
        b.new_gaps.push(NewGap {
            dir: ScanDir::Forward,
            from_block: from,
            to_block: to,
            reason,
        });
        b.next_block = Some(to + 1);
        b.tip = tip;
        self.repo.apply_scan(&b)?;
        rep.new_gaps += 1;
        Ok(())
    }

    /// Decode, guard, coalesce and persist one fetched range. `gap_id` marks the open gap this
    /// range heals (resolved in the same transaction; a part left over becomes a new gap).
    async fn process(
        &self,
        logs: &[Log],
        from: u64,
        to: u64,
        mode: Mode,
        gap: Option<(i64, u64, u64, ScanDir)>,
        rep: &mut StepReport,
    ) -> Result<Processed> {
        let known: Vec<(u64, B256)> = self
            .repo
            .scan_ancestry(&self.cfg.name, ANCESTRY_WINDOW)?
            .iter()
            .map(|p| (p.block_number, p.block_hash))
            .collect();
        let mut guard = LogGuard::new(if mode == Mode::Live { known } else { vec![] });
        let mut co = Coalescer::new(self.cfg.coalesce_cap);
        let (mut deps, mut extra) = (vec![], vec![]);
        let mut found = BTreeSet::new();
        let mut points: Vec<ChainPoint> = vec![];
        let mut events_n = 0;
        let mut first_found: HashMap<Address, ChainPoint> = HashMap::new();
        for log in logs {
            match guard.accept(log) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(Invalid::Removed) => {
                    return Ok(Processed::ChainChanged("removed log".into()));
                }
                Err(Invalid::ConflictingBlock) => {
                    return Ok(Processed::ChainChanged("conflicting block hash".into()));
                }
                Err(Invalid::Reordered) => {
                    return Ok(Processed::Bad("logs out of order".into()));
                }
                Err(Invalid::MissingIdentity) => {
                    return Ok(Processed::Bad("log without block identity".into()));
                }
            }
            let at = ChainPoint {
                block_number: log.block_number.unwrap_or_default(),
                block_hash: log.block_hash.unwrap_or_default(),
            };
            let ev = match events::decode(log, self.aave.pool, self.aave.oracle) {
                Ok(Some(ev)) => ev,
                Ok(None) => continue,
                Err(e) => return Ok(Processed::Bad(e.to_string())),
            };
            events_n += 1;
            if points.last() != Some(&at) {
                points.push(at);
            }
            if let PoolEvent::Borrow { on_behalf_of, .. } = &ev {
                first_found.entry(*on_behalf_of).or_insert(at);
            }
            apply_event(&ev, at, mode, &mut co, &mut deps, &mut extra, &mut found);
        }
        let now = now_ns();
        let mut b = ScanBatch::new(
            &self.cfg.name,
            self.cfg.chain_id,
            self.aave.pool,
            now,
            self.cfg.fanout_limit,
        );
        // Order: borrower touches (registration first), dependency edits in log order, then
        // asset-level and pool-level effects.
        let (touches, rest): (Vec<_>, Vec<_>) = co
            .into_effects()
            .into_iter()
            .partition(|e| matches!(e, Effect::Touch { .. }));
        b.effects.extend(touches);
        b.effects.extend(deps);
        b.effects.extend(rest);
        b.effects.extend(extra);
        for who in &found {
            if let Some(at) = first_found.get(who) {
                b.effects.push(Effect::Discover {
                    borrower: *who,
                    at: *at,
                });
            }
        }
        match (mode, &gap) {
            (Mode::Live, None) => {
                let tip = self.canonical_point(to).await?;
                if logs
                    .iter()
                    .any(|l| l.block_number == Some(to) && l.block_hash != Some(tip.block_hash))
                {
                    return Ok(Processed::ChainChanged("tip block replaced".into()));
                }
                b.next_block = Some(to + 1);
                b.tip = Some(tip);
            }
            (Mode::Backfill, None) => b.back_block = Some(from),
            _ => {}
        }
        if let Some((id, gfrom, gto, dir)) = gap {
            b.resolved_gaps.push(id);
            let rest = match dir {
                ScanDir::Forward if to < gto => Some((to + 1, gto)),
                ScanDir::Backward if from > gfrom => Some((gfrom, from - 1)),
                _ => None,
            };
            if let Some((f, t)) = rest {
                b.new_gaps.push(NewGap {
                    dir,
                    from_block: f,
                    to_block: t,
                    reason: format!("remainder of gap {id}"),
                });
            }
        }
        b.ancestry = if mode == Mode::Live { points } else { vec![] };
        let out = self.repo.apply_scan(&b)?;
        rep.events += events_n;
        rep.registered += out.registered;
        rep.marked += out.marked;
        rep.queued += out.queued;
        Ok(Processed::Done)
    }

    async fn heal_gaps(&mut self, budget: &mut usize, rep: &mut StepReport) -> Result<()> {
        let (addrs, topics) = self.query_parts();
        for g in self.repo.open_gaps(&self.cfg.name)? {
            if *budget == 0 {
                break;
            }
            let backward = g.dir == ScanDir::Backward;
            let q = LogQuery {
                addresses: &addrs,
                topics: &topics,
                name: &self.cfg.name,
            };
            let fetched = fetch_range(
                &self.scan,
                &q,
                g.from_block,
                g.to_block,
                backward,
                &mut self.step_hint,
                self.cfg.throttle,
                &self.cfg.retry,
                budget,
            )
            .await;
            let failed = |me: &Self| -> Result<()> {
                let mut b = ScanBatch::new(
                    &me.cfg.name,
                    me.cfg.chain_id,
                    me.aave.pool,
                    now_ns(),
                    me.cfg.fanout_limit,
                );
                b.failed_gap_attempts.push(g.id);
                me.repo.apply_scan(&b)?;
                Ok(())
            };
            match fetched {
                Fetched::Logs { logs, from, to } => {
                    let mode = if backward {
                        Mode::Backfill
                    } else {
                        Mode::Replay
                    };
                    let p = self
                        .process(
                            &logs,
                            from,
                            to,
                            mode,
                            Some((g.id, g.from_block, g.to_block, g.dir)),
                            rep,
                        )
                        .await?;
                    match p {
                        Processed::Done => rep.healed_gaps += 1,
                        Processed::ChainChanged(_) | Processed::Bad(_) => failed(self)?,
                    }
                }
                Fetched::Gap { .. } => failed(self)?,
                Fetched::Deferred => break,
            }
        }
        Ok(())
    }

    async fn backfill(&mut self, budget: &mut usize, rep: &mut StepReport) -> Result<()> {
        let (addrs, topics) = self.query_parts();
        let mut gaps = 0;
        loop {
            let cur = self
                .repo
                .scan_cursor(&self.cfg.name)?
                .context("cursor missing")?;
            if cur.back_block <= self.cfg.from_block || *budget == 0 || gaps >= MAX_GAPS_PER_STEP {
                return Ok(());
            }
            let to = cur.back_block - 1;
            let from = to
                .saturating_sub(self.step_hint.max(1) - 1)
                .max(self.cfg.from_block);
            let q = LogQuery {
                addresses: &addrs,
                topics: &topics,
                name: &self.cfg.name,
            };
            let fetched = fetch_range(
                &self.scan,
                &q,
                from,
                to,
                true,
                &mut self.step_hint,
                self.cfg.throttle,
                &self.cfg.retry,
                budget,
            )
            .await;
            match fetched {
                Fetched::Logs { logs, from, to } => {
                    match self
                        .process(&logs, from, to, Mode::Backfill, None, rep)
                        .await?
                    {
                        Processed::Done => {}
                        Processed::ChainChanged(why) | Processed::Bad(why) => {
                            self.backward_gap(from, to, why, rep)?;
                            gaps += 1;
                        }
                    }
                }
                Fetched::Gap { from, to, reason } => {
                    self.backward_gap(from, to, reason, rep)?;
                    gaps += 1;
                }
                Fetched::Deferred => return Ok(()),
            }
        }
    }

    fn backward_gap(&self, from: u64, to: u64, reason: String, rep: &mut StepReport) -> Result<()> {
        let mut b = ScanBatch::new(
            &self.cfg.name,
            self.cfg.chain_id,
            self.aave.pool,
            now_ns(),
            self.cfg.fanout_limit,
        );
        b.new_gaps.push(NewGap {
            dir: ScanDir::Backward,
            from_block: from,
            to_block: to,
            reason,
        });
        b.back_block = Some(from);
        self.repo.apply_scan(&b)?;
        rep.new_gaps += 1;
        Ok(())
    }

    /// Reconcile the dependency sets of borrowers that never had one (or went stale) against
    /// `getUserConfiguration` + `getUserAccountData` read at one pinned block. Borrowers without
    /// debt are deactivated and leave the reverse index.
    pub async fn reconcile(&self, rep: &mut StepReport) -> Result<()> {
        let (chain_id, pool) = (self.cfg.chain_id, self.aave.pool);
        let list = self
            .repo
            .list_unreconciled(chain_id, pool, self.cfg.reconcile_batch)?;
        if list.is_empty() {
            return Ok(());
        }
        let (block, number) = snapshot_block(&self.fast).await?;
        let hash = block
            .as_block_hash()
            .context("snapshot_block returned a non-hash block id")?;
        let at = ChainPoint {
            block_number: number,
            block_hash: hash,
        };
        let mut calls = Vec::with_capacity(list.len() * 2);
        for u in &list {
            calls.push(call(pool, IAavePool::getUserConfigurationCall { user: *u }));
            calls.push(call(pool, IAavePool::getUserAccountDataCall { user: *u }));
        }
        let res = multicall_at(&self.fast, &calls, 200, block).await?;
        let ids = self.reserves.clone();
        for (i, u) in list.iter().enumerate() {
            let cfg = decode::<IAavePool::getUserConfigurationCall>(&res[2 * i]);
            let acct = decode::<IAavePool::getUserAccountDataCall>(&res[2 * i + 1]);
            let (Some(mask), Some(acct)) = (cfg, acct) else {
                rep.warnings
                    .push(format!("reconcile {u}: call failed at block {number}"));
                continue;
            };
            let has_debt = !acct.totalDebtBase.is_zero();
            match self.repo.reconcile_borrower(
                chain_id,
                pool,
                *u,
                &deps_from_user_config(mask, &ids),
                has_debt,
                at,
            )? {
                ReconcileOutcome::Active => rep.reconciled += 1,
                ReconcileOutcome::Deactivated => {
                    rep.reconciled += 1;
                    rep.deactivated += 1;
                }
                ReconcileOutcome::Superseded | ReconcileOutcome::Unknown => {}
            }
        }
        Ok(())
    }

    /// Oracle price polling (independent of any oracle event): compare `getAssetPrice` per
    /// reserve, at one pinned block, with the stored baseline; a move of at least
    /// `price_min_move_bps` marks every holder (bounded fan-out) and becomes the new baseline.
    /// The baseline only moves when it triggers, so slow drift accumulates and fires.
    pub async fn poll_prices(&self, rep: &mut StepReport) -> Result<()> {
        let (chain_id, pool) = (self.cfg.chain_id, self.aave.pool);
        let (block, number) = snapshot_block(&self.fast).await?;
        let hash = block
            .as_block_hash()
            .context("snapshot_block returned a non-hash block id")?;
        let at = ChainPoint {
            block_number: number,
            block_hash: hash,
        };
        let calls: Vec<_> = self
            .reserves
            .iter()
            .map(|(_, a)| {
                call(
                    self.aave.oracle,
                    IAaveOracle::getAssetPriceCall { asset: *a },
                )
            })
            .collect();
        let res = multicall_at(&self.fast, &calls, 200, block).await?;
        let mut seed = vec![];
        let mut moved = vec![];
        for ((_, asset), r) in self.reserves.iter().zip(&res) {
            let Some(px) = decode::<IAaveOracle::getAssetPriceCall>(r) else {
                continue;
            };
            match self.repo.oracle_price(chain_id, pool, *asset)? {
                None => seed.push((*asset, px)),
                Some(old) if moved_enough(old, px, self.cfg.price_min_move_bps) => {
                    moved.push((*asset, px));
                }
                Some(_) => {}
            }
        }
        if !seed.is_empty() {
            self.repo.seed_prices(chain_id, pool, &seed, Some(at))?;
        }
        if !moved.is_empty() {
            let o = self.repo.record_price_moves(
                chain_id,
                pool,
                &moved,
                Some(at),
                now_ns(),
                self.cfg.fanout_limit,
            )?;
            rep.price_moves += moved.len();
            rep.marked += o.marked;
            rep.queued += o.queued;
        }
        Ok(())
    }

    /// Oracle trigger from the sequencer-feed path: `assets` come from the adapter's
    /// `FeedWatcher` match of a pending price-update transaction. Provisional by nature, so no
    /// chain position is attached.
    pub fn mark_assets_price_moved(&self, assets: &[Address]) -> Result<usize, RepoError> {
        self.marker().mark_assets_price_moved(assets)
    }

    /// A cheap handle that marks price-move holders without the indexer, so the indexer itself
    /// can step in its own task while the sequencer path keeps marking.
    pub fn marker(&self) -> PriceMarker {
        PriceMarker {
            repo: self.repo.clone(),
            chain_id: self.cfg.chain_id,
            pool: self.aave.pool,
            fanout_limit: self.cfg.fanout_limit,
        }
    }

    /// Step forever, `every` apart (start to start; a step longer than `every` is followed at
    /// once by the next). Errors are logged and the loop continues. Never returns.
    pub async fn run(mut self, every: Duration) {
        let mut iv = tokio::time::interval(every);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            if let Err(e) = self.step().await {
                warn!(index = %self.cfg.name, error = %format!("{e:#}"), "trigger index step");
            }
        }
    }
}

/// Marks the holders of reserves whose price moved (`DirtyReason::PriceMove`), bounded by the
/// indexer's fan-out limit. Cloneable; shares the indexer's repository.
#[derive(Clone)]
pub struct PriceMarker {
    repo: Arc<SnapshotRepository>,
    chain_id: u64,
    pool: Address,
    fanout_limit: usize,
}

impl PriceMarker {
    pub fn mark_assets_price_moved(&self, assets: &[Address]) -> Result<usize, RepoError> {
        let mut marked = 0;
        for a in assets {
            let o = self.repo.mark_asset_holders(
                self.chain_id,
                self.pool,
                *a,
                DirtyReason::PriceMove,
                None,
                now_ns(),
                self.fanout_limit,
            )?;
            marked += o.marked;
        }
        Ok(marked)
    }
}

/// `|new - old| * 10_000 >= old * min_bps` in checked 256-bit arithmetic. A zero baseline moves
/// on any change.
pub fn moved_enough(old: U256, new: U256, min_bps: u64) -> bool {
    if old == new {
        return false;
    }
    if old.is_zero() {
        return true;
    }
    let diff = if new > old { new - old } else { old - new };
    match (
        diff.checked_mul(U256::from(10_000u64)),
        old.checked_mul(U256::from(min_bps.max(1))),
    ) {
        (Some(l), Some(r)) => l >= r,
        _ => true, // overflow means an astronomically large move: treat as moved
    }
}
