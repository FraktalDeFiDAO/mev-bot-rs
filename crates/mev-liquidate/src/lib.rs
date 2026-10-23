//! Protocol-agnostic liquidation runner.
//!
//! Drives any set of [`LiquidationAdapter`]s: incremental account indexing, periodic health sweeps
//! (sweep RPC), and at-risk polling (fast RPC). Polling is event-driven: when the sequencer feed
//! shows a transaction to a price feed an adapter depends on, that adapter's at-risk accounts are
//! re-checked in a short burst (state lands ~0.5–1 s after the feed). A slow fallback poll covers
//! anything the feed misses. Liquidatable accounts are planned by their adapter and sent out.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use alloy::primitives::Address;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, WatchDetail};
use mev_oracle::FeedWatcher;
use mev_sequencer::SequencedTx;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

mod shadow;
pub use shadow::{
    AdvisoryShadow, DecisionFn, EVENT_ADVISORY, EVENT_COUNTERS, EVENT_SCHEDULE, HealthFn,
    ShadowSchedule, ShadowSink,
};

/// Index more often while history is being back-filled.
/// Delay before the single retry of an adapter whose sweep failed (instead of a full `sweep_secs`).
const SWEEP_RETRY: Duration = Duration::from_secs(60);
const BACKFILL_EVERY: Duration = Duration::from_secs(5);
/// After a price-feed tx, re-poll at these offsets: the block containing the update lands
/// ~0.5-1 s after the feed. Three polls instead of every 250 ms for 1.8 s (7 polls) — the
/// bursts were the largest paid-RPC consumer in live logs (~60 calls/min).
const BURST_OFFSETS: [Duration; 3] = [
    Duration::from_millis(400),
    Duration::from_millis(900),
    Duration::from_millis(1_600),
];
const BURST_TICK: Duration = Duration::from_millis(100);
/// Pending delayed schedules kept per adapter (oldest dropped first).
const MAX_DELAYED: usize = 8;

/// Re-poll offsets (from when we see the update) for prices that apply `delay` after the round's
/// timestamp. We see an update 0.2-2 s after its block's timestamp, so it takes effect
/// delay-2 s .. delay-0.2 s after we see it: poll across that window and twice after. Health is
/// read at `pending` on Base, so the first poll inside the unlocked block sees the new price.
fn delayed_offsets(delay: Duration) -> Vec<Duration> {
    if delay.is_zero() {
        return Vec::new();
    }
    let d = delay.as_millis() as u64;
    [2_000u64, 1_400, 800, 200]
        .iter()
        .map(|b| Duration::from_millis(d.saturating_sub(*b)))
        .chain([d + 400, d + 1_200].map(Duration::from_millis))
        .collect()
}

/// Advance `slot`'s delayed schedules to `now`; the feeds whose next offset is due.
fn due_delayed(slot: &mut Slot, now: Instant) -> Vec<Address> {
    let mut due = Vec::new();
    let n = slot.delayed_offsets.len();
    for (start, next, feed) in &mut slot.delayed {
        if now.duration_since(*start) >= slot.delayed_offsets[*next] {
            *next += 1;
            if !due.contains(feed) {
                due.push(*feed);
            }
        }
    }
    slot.delayed.retain(|(_, next, _)| *next < n);
    due
}
/// Don't re-plan the same account more often than this.
/// Re-plan cooldown per account: starts here and doubles each time the same account is still
/// liquidatable (a real opportunity gets liquidated and disappears; one that persists is
/// unworkable — bad debt, no route, too large — and each re-plan costs paid RPC calls).
const REPLAN_COOLDOWN: Duration = Duration::from_secs(3);

/// Positions below this health factor go in the watchlist: liquidatable after a uniform
/// collateral price drop of up to 20%.
const WATCH_HF: f64 = 1.25;
/// Price drops (percent) the watchlist totals the unlockable bonus for; 0 = liquidatable now.
const WATCH_DROPS: [f64; 6] = [0.0, 1.0, 2.0, 5.0, 10.0, 20.0];
const REPLAN_COOLDOWN_MAX: Duration = Duration::from_secs(1_800);

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Accounts below this HF (with enough debt and collateral) are "at risk" and polled.
    pub at_risk_hf: f64,
    pub min_debt_usd: f64,
    /// Full health sweep period.
    pub sweep_secs: u64,
    /// Fallback at-risk poll period (feed-triggered bursts handle the fast path).
    pub fallback_poll_secs: u64,
    /// Passed to adapters building plans.
    pub executor: Address,
}

/// What the runner reports to the engine.
#[derive(Debug, Clone)]
pub enum RunnerEvent {
    Plan(Plan),
    /// Predicted from a price update seen in the sequencer feed, before it is in a block: send
    /// now, unsimulated (the chain doesn't have the price yet). `lead` = time since the feed frame.
    Backrun {
        plan: Plan,
        lead: Duration,
    },
    /// Predicted from an SVR auction's hints (the oracle update a solver can bundle with its
    /// liquidation): one event per auction, so both parallel auctions of an update get a bid.
    AtlasBackrun {
        plan: Plan,
        auction_id: String,
        lead: Duration,
    },
    /// Liquidatable but nothing worth taking (bad debt, dust, unsupported shape).
    NoPlan(Health),
    /// Price-feed transaction seen in the sequencer feed (for metrics).
    FeedTrigger {
        adapter: &'static str,
        lead: Duration,
    },
}

/// An oracle update announced by an SVR auction (Atlas hints): `feed` will report `answer`
/// (raw aggregator units, like `mev_oracle::report_answer`).
#[derive(Debug, Clone)]
pub struct OracleHint {
    pub feed: Address,
    pub answer: f64,
    pub auction_id: String,
    pub received: Instant,
}

struct Slot {
    adapter: Box<dyn LiquidationAdapter>,
    reconciled: bool,
    at_risk: HashMap<AccountRef, Health>,
    backfilled: bool,
    /// Active burst: when it started and the next offset index to poll.
    burst: Option<(Instant, usize)>,
    /// Feed contracts updated since the burst started: only accounts they affect are re-polled.
    burst_feeds: Vec<Address>,
    /// Re-poll offsets around the moment a feed update takes effect, for adapters that apply
    /// prices late (`feed_unlock_delay`); empty when updates apply at once.
    delayed_offsets: Vec<Duration>,
    /// Pending delayed re-poll schedules: (update seen, next offset index, feed). One per update,
    /// so a later update doesn't cancel an earlier one's unlock.
    delayed: Vec<(Instant, usize, Address)>,
    /// After an index error, don't retry before this (exponential backoff; rate limits).
    index_retry_at: Option<Instant>,
    index_backoff: Duration,
    /// Accounts whose last sweep read showed debt below `min_debt_usd` (repaid or dust): left out
    /// of sweeps except every `DORMANT_FULL_EVERY`-th, which reads everything again.
    dormant: HashSet<AccountRef>,
    /// Positions the last successful sweep found within `WATCH_HF` (with enough debt), for the
    /// watchlist file.
    watch: Vec<(Health, WatchDetail)>,
    /// Sweeps attempted, to schedule the full ones.
    sweeps: u64,
}

/// Every this-many sweeps reads every account, dormant ones included (12 x 300 s = hourly).
/// Most indexed borrowers have repaid (70% of Aave's 188k had zero debt, 2026-10-14), and an
/// account cannot borrow straight into liquidation, so an hourly re-read is enough to wake one;
/// the full sweep of every account every 300 s was most of the bot's RPC volume.
const DORMANT_FULL_EVERY: u64 = 12;

const INDEX_BACKOFF_MIN: Duration = Duration::from_secs(15);
const INDEX_BACKOFF_MAX: Duration = Duration::from_secs(600);

pub struct Runner {
    revision: std::sync::Arc<std::sync::atomic::AtomicU64>,
    confidence: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cfg: RunnerConfig,
    slots: Vec<Slot>,
    feeds: FeedWatcher<usize>,
    /// When each account was last planned and its current cooldown.
    last_plan: HashMap<AccountRef, (Instant, Duration)>,
    /// When each account was last backrun (one attempt per account per `BACKRUN_COOLDOWN`).
    last_backrun: HashMap<AccountRef, Instant>,
    /// Feeds whose public updates are held back (SVR): re-polled, never backrun instantly.
    held: HashSet<Address>,
    /// The watched feed contracts, shared with the feed fan-out that pre-filters transactions;
    /// updated when adapters' feeds change (indexing finds new markets).
    feed_set: std::sync::Arc<std::sync::RwLock<HashSet<Address>>>,
    /// Auction hints (SVR); taken by `run`.
    hints: Option<mpsc::UnboundedReceiver<OracleHint>>,
    /// Optional shadow-only mandatory scheduler pass (JEV-LIQ-005); never alters dispatch.
    shadow: Option<ShadowSchedule>,
    /// Plans predicted for a (feed, answer) update, reused by the parallel auction of the same
    /// update (adapters move their prediction state on each `predict`).
    hint_plans: HashMap<(Address, u64), (Instant, Vec<Plan>)>,
    /// Where each sweep writes the watchlist of near-liquidation positions (JSON), if anywhere.
    watchlist: Option<std::path::PathBuf>,
}

/// How long predicted plans are reused for the same update.
const HINT_PLAN_TTL: Duration = Duration::from_secs(20);

/// A failed backrun costs gas; don't retry the same account on every subsequent update.
const BACKRUN_COOLDOWN: Duration = Duration::from_secs(30);
/// Per price update, only the biggest predicted liquidations are sent (bounds gas lost to
/// mispredictions in a crash; the regular re-poll picks up the rest).
const MAX_BACKRUNS_PER_UPDATE: usize = 3;

impl Runner {
    /// Adapters that aren't `Live` are logged and skipped.
    pub fn new(cfg: RunnerConfig, adapters: Vec<Box<dyn LiquidationAdapter>>) -> Self {
        let mut feeds = FeedWatcher::default();
        let mut slots = Vec::new();
        for adapter in adapters {
            if adapter.status() != AdapterStatus::Live {
                info!(protocol = adapter.id(), status = ?adapter.status(), "liquidation adapter not live; skipping");
                continue;
            }
            for f in adapter.price_feeds() {
                feeds.watch(f, slots.len());
            }
            let delayed_offsets = delayed_offsets(adapter.feed_unlock_delay());
            slots.push(Slot {
                adapter,
                reconciled: false,
                at_risk: HashMap::new(),
                backfilled: false,
                burst: None,
                burst_feeds: Vec::new(),
                delayed_offsets,
                delayed: Vec::new(),
                index_retry_at: None,
                index_backoff: INDEX_BACKOFF_MIN,
                dormant: HashSet::new(),
                watch: Vec::new(),
                sweeps: 0,
            });
        }
        info!(
            adapters = slots.len(),
            watched_feeds = feeds.len(),
            "liquidation runner ready"
        );
        let feed_set = std::sync::Arc::new(std::sync::RwLock::new(
            feeds.contracts().into_iter().collect::<HashSet<_>>(),
        ));
        Self {
            revision: Default::default(),
            confidence: Default::default(),
            cfg,
            slots,
            feeds,
            last_plan: HashMap::new(),
            last_backrun: HashMap::new(),
            held: HashSet::new(),
            feed_set,
            hints: None,
            shadow: None,
            hint_plans: HashMap::new(),
            watchlist: None,
        }
    }

    fn stamp_plan(&self, mut plan: Plan) -> Plan {
        plan.dependencies.push(mev_types::StateVersion {
            revision: self.revision.clone(),
            confidence: self.confidence.clone(),
            expected: self.revision.load(std::sync::atomic::Ordering::Acquire),
        });
        plan
    }

    fn invalidate(&mut self, picked: &[usize]) {
        self.confidence
            .store(false, std::sync::atomic::Ordering::Release);
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        for &i in picked {
            self.slots[i].reconciled = false;
        }
    }

    fn restore_confidence(&self) {
        self.confidence.store(
            !self.slots.is_empty() && self.slots.iter().all(|s| s.reconciled),
            std::sync::atomic::Ordering::Release,
        );
    }

    /// Take oracle updates announced by SVR auctions as another source of backruns.
    /// Price feeds whose public updates are held back (Chainlink SVR `DualAggregator`s): their
    /// reports still trigger a re-poll, but no instant backrun, which would see the old price and
    /// revert (all 11 Arbitrum Aave backruns, 2026-10-08..10).
    pub fn with_held_feeds(mut self, held: HashSet<Address>) -> Self {
        self.held = held;
        self
    }

    /// Write the watchlist of near-liquidation positions to `path` after every sweep.
    pub fn with_watchlist(mut self, path: std::path::PathBuf) -> Self {
        self.watchlist = Some(path);
        self
    }

    pub fn with_hints(mut self, rx: mpsc::UnboundedReceiver<OracleHint>) -> Self {
        self.hints = Some(rx);
        self
    }

    /// Attach the shadow mandatory-scheduler pass. It only records; it is skipped unless
    /// `schedule.enabled` (default false).
    pub fn with_shadow_schedule(mut self, schedule: ShadowSchedule) -> Self {
        self.shadow = Some(schedule);
        self
    }

    /// Feed contracts the live adapters watch.
    pub fn watched_feeds(&self) -> Vec<Address> {
        self.feeds.contracts()
    }

    /// The watched feed contracts as a live set (kept current by `refresh_feeds`).
    pub fn watched_feed_set(&self) -> std::sync::Arc<std::sync::RwLock<HashSet<Address>>> {
        self.feed_set.clone()
    }

    /// Rebuild the feed watcher from the adapters' current feeds (they grow as indexing finds
    /// markets). Returns whether the watched set changed.
    fn refresh_feeds(&mut self) -> bool {
        let mut feeds = FeedWatcher::default();
        for (i, s) in self.slots.iter().enumerate() {
            for f in s.adapter.price_feeds() {
                feeds.watch(f, i);
            }
        }
        let now: HashSet<Address> = feeds.contracts().into_iter().collect();
        if *self.feed_set.read().unwrap() == now {
            return false;
        }
        info!(
            before = self.feeds.len(),
            watched_feeds = now.len(),
            "watched price feeds changed"
        );
        self.feeds = feeds;
        *self.feed_set.write().unwrap() = now;
        true
    }

    /// Tokens across all live adapters (for swap-route discovery).
    pub fn assets(&self) -> Vec<Address> {
        let mut v: Vec<Address> = self.slots.iter().flat_map(|s| s.adapter.assets()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Targets every live adapter needs allowlisted on the executor.
    pub fn targets(&self) -> Vec<(&'static str, Address)> {
        self.slots
            .iter()
            .flat_map(|s| {
                s.adapter
                    .targets()
                    .into_iter()
                    .map(move |t| (s.adapter.id(), t))
            })
            .collect()
    }

    fn worth_watching(&self, h: &Health) -> bool {
        h.debt_usd >= self.cfg.min_debt_usd && h.collateral_usd >= self.cfg.min_debt_usd
    }

    async fn index_all(&mut self) -> bool {
        self.invalidate(&(0..self.slots.len()).collect::<Vec<_>>());
        let mut all_done = true;
        for s in &mut self.slots {
            if s.index_retry_at.is_some_and(|t| Instant::now() < t) {
                all_done &= s.backfilled;
                continue;
            }
            match s.adapter.index().await {
                Ok(done) => {
                    s.backfilled = done;
                    s.index_retry_at = None;
                    s.index_backoff = INDEX_BACKOFF_MIN;
                }
                Err(e) => {
                    warn!(
                        protocol = s.adapter.id(),
                        error = %format!("{e:#}"),
                        retry_in_s = s.index_backoff.as_secs(),
                        "index failed"
                    );
                    s.index_retry_at = Some(Instant::now() + s.index_backoff);
                    s.index_backoff = (s.index_backoff * 2).min(INDEX_BACKOFF_MAX);
                }
            }
            all_done &= s.backfilled;
        }
        all_done
    }

    /// Health-sweep the adapters `which` (all when `None`) concurrently (one slow venue must not
    /// delay the others). Returns the ones that failed.
    async fn sweep_all(&mut self, which: Option<&[usize]>) -> Vec<usize> {
        let picked: Vec<usize> = match which {
            Some(w) => w.to_vec(),
            None => (0..self.slots.len()).collect(),
        };
        // Adapter reads may update their own prediction caches before returning.
        self.invalidate(&picked);
        let jobs = picked.iter().map(|&i| {
            let s = &self.slots[i];
            let accounts = sweep_set(s);
            async move {
                let res = s.adapter.health(&accounts, false).await;
                (i, accounts.len(), res)
            }
        });
        let results = futures::future::join_all(jobs).await;
        let mut failed = Vec::new();
        for (i, n, res) in results {
            if !self.apply_sweep(i, n, res) {
                failed.push(i);
            }
        }
        self.write_watchlist();
        failed
    }

    /// Write every venue's watched positions, with the bonus a liquidator could take at each
    /// price drop. Off the hot path (after a sweep); the file is replaced atomically.
    fn write_watchlist(&self) {
        let Some(path) = self.watchlist.clone() else {
            return;
        };
        let doc = watchlist_json(&self.slots, std::time::SystemTime::now());
        if let Some(b) = doc["bonus_by_price_drop"].as_array() {
            let at = |d: f64| {
                b.iter()
                    .find(|x| x["drop_pct"].as_f64() == Some(d))
                    .and_then(|x| x["est_bonus_usd"].as_f64())
                    .unwrap_or(0.0)
            };
            info!(
                now_usd = format!("{:.0}", at(0.0)),
                within_5pct_usd = format!("{:.0}", at(5.0)),
                within_20pct_usd = format!("{:.0}", at(20.0)),
                "liquidation watchlist"
            );
        }
        tokio::task::spawn_blocking(move || {
            let tmp = path.with_extension("json.tmp");
            let res = serde_json::to_vec_pretty(&doc)
                .map_err(anyhow::Error::from)
                .and_then(|b| Ok(std::fs::write(&tmp, b)?))
                .and_then(|()| Ok(std::fs::rename(&tmp, &path)?));
            if let Err(e) = res {
                warn!(error = %format!("{e:#}"), path = %path.display(), "watchlist write failed");
            }
        });
    }

    #[cfg(test)]
    async fn sweep(&mut self, i: usize) {
        self.invalidate(&[i]);
        let accounts = sweep_set(&self.slots[i]);
        let res = self.slots[i].adapter.health(&accounts, false).await;
        self.apply_sweep(i, accounts.len(), res);
    }

    /// Store a sweep's at-risk accounts; false if it failed.
    fn apply_sweep(&mut self, i: usize, accounts: usize, res: anyhow::Result<Vec<Health>>) -> bool {
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        let res = res.and_then(|hs| {
            anyhow::ensure!(hs.len() == accounts, "incomplete borrower health sweep");
            Ok(hs)
        });
        self.slots[i].reconciled = res.is_ok();
        self.slots[i].sweeps += 1;
        self.restore_confidence();
        let id = self.slots[i].adapter.id();
        match res {
            Ok(hs) => {
                let min_debt = self.cfg.min_debt_usd;
                let dormant = &mut self.slots[i].dormant;
                for h in &hs {
                    if h.debt_usd < min_debt {
                        dormant.insert(h.account);
                    } else {
                        dormant.remove(&h.account);
                    }
                }
                let dormant = dormant.len();
                let watch: Vec<(Health, WatchDetail)> = hs
                    .iter()
                    .filter(|h| h.health_factor < WATCH_HF && self.worth_watching(h))
                    .map(|h| (*h, self.slots[i].adapter.watch_detail(h)))
                    .collect();
                self.slots[i].watch = watch;
                let at_risk: HashMap<AccountRef, Health> = hs
                    .into_iter()
                    .filter(|h| h.health_factor < self.cfg.at_risk_hf && self.worth_watching(h))
                    .map(|h| (h.account, h))
                    .collect();
                let liquidatable = at_risk.values().filter(|h| h.liquidatable()).count();
                info!(
                    protocol = id,
                    accounts,
                    dormant,
                    at_risk = at_risk.len(),
                    liquidatable,
                    "sweep"
                );
                self.slots[i].at_risk = at_risk;
                true
            }
            Err(e) => {
                warn!(protocol = id, error = %format!("{e:#}"), "sweep failed");
                false
            }
        }
    }

    /// Re-check one adapter's at-risk accounts on the fast RPC; plan the liquidatable ones.
    /// With `feeds`, only accounts one of those price feeds can affect. Returns false when the
    /// event receiver is gone.
    async fn poll(
        &mut self,
        i: usize,
        feeds: Option<&[Address]>,
        tx: &mpsc::UnboundedSender<RunnerEvent>,
    ) -> bool {
        let slot = &self.slots[i];
        let accounts: Vec<AccountRef> = slot
            .at_risk
            .keys()
            .filter(|a| feeds.is_none_or(|fs| fs.iter().any(|f| slot.adapter.affected(*f, a))))
            .copied()
            .collect();
        if accounts.is_empty() {
            return true;
        }
        let previously_reconciled = self.slots[i].reconciled;
        self.invalidate(&[i]);
        let hs = match self.slots[i].adapter.health(&accounts, true).await {
            Ok(h) => h,
            Err(e) => {
                warn!(protocol = self.slots[i].adapter.id(), error = %format!("{e:#}"), "poll failed");
                return true;
            }
        };
        if hs.len() != accounts.len() {
            warn!(
                protocol = self.slots[i].adapter.id(),
                "incomplete borrower health poll"
            );
            return true;
        }
        // A subset poll cannot restore confidence lost by a failed full sweep.
        self.slots[i].reconciled = previously_reconciled;
        self.restore_confidence();
        for h in hs {
            self.slots[i].at_risk.insert(h.account, h);
            if !h.liquidatable() || !self.worth_watching(&h) {
                self.last_plan.remove(&h.account); // healthy again: reset its backoff
                continue;
            }
            let cooldown = match self.last_plan.get(&h.account) {
                Some((at, cd)) if at.elapsed() < *cd => continue,
                Some((_, cd)) => (*cd * 2).min(REPLAN_COOLDOWN_MAX),
                None => REPLAN_COOLDOWN,
            };
            self.last_plan.insert(h.account, (Instant::now(), cooldown));
            let ev = match self.slots[i].adapter.plan(&h, self.cfg.executor).await {
                Ok(Some(p)) => RunnerEvent::Plan(self.stamp_plan(p)),
                Ok(None) => RunnerEvent::NoPlan(h),
                Err(e) => {
                    warn!(protocol = self.slots[i].adapter.id(), account = %h.account, error = %format!("{e:#}"), "plan failed");
                    continue;
                }
            };
            if tx.send(ev).is_err() {
                return false;
            }
        }
        true
    }

    /// Mark adapters watching the feed contract `to` for a burst of re-polls.
    fn on_feed_tx(&mut self, st: &SequencedTx, tx: &mpsc::UnboundedSender<RunnerEvent>) {
        let hits: Vec<usize> = self.feeds.matches_tx(st.to, &st.input).to_vec();
        let Some(feed) = self.feeds.matched_contract(st.to, &st.input) else {
            return;
        };
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        let report = mev_oracle::report_answer(st.to, &st.input)
            .filter(|(agg, _)| *agg == feed && !self.held.contains(agg));
        let mut predicted: Vec<Plan> = Vec::new();
        for i in hits.iter().copied() {
            let Some((_, answer)) = report else { break };
            predicted.extend(
                self.slots[i]
                    .adapter
                    .predict(feed, answer, self.cfg.executor),
            );
        }
        predicted.sort_by(|a, b| b.bonus_usd.total_cmp(&a.bonus_usd));
        {
            for plan in predicted.into_iter().take(MAX_BACKRUNS_PER_UPDATE) {
                let recent = self
                    .last_backrun
                    .get(&plan.account)
                    .is_some_and(|t| t.elapsed() < BACKRUN_COOLDOWN);
                if recent {
                    continue;
                }
                self.last_backrun.insert(plan.account, Instant::now());
                let lead = st.received.elapsed();
                let _ = tx.send(RunnerEvent::Backrun {
                    plan: self.stamp_plan(plan),
                    lead,
                });
            }
        }
        for i in hits {
            let slot = &mut self.slots[i];
            if slot.burst.is_none() {
                slot.burst_feeds.clear();
            }
            if !slot.burst_feeds.contains(&feed) {
                slot.burst_feeds.push(feed);
            }
            slot.burst = Some((Instant::now(), 0));
            if !slot.delayed_offsets.is_empty() {
                if slot.delayed.len() >= MAX_DELAYED {
                    slot.delayed.remove(0);
                }
                slot.delayed.push((Instant::now(), 0, feed));
            }
            debug!(protocol = self.slots[i].adapter.id(), tx = %st.hash, "price-feed update in sequencer feed");
            let _ = tx.send(RunnerEvent::FeedTrigger {
                adapter: self.slots[i].adapter.id(),
                lead: st.received.elapsed(),
            });
        }
    }

    /// An SVR auction announced `h`: predict the liquidations that price makes possible, like
    /// `on_feed_tx` does for a report seen in the sequencer feed, and emit one event per auction.
    fn on_hint(&mut self, h: &OracleHint, tx: &mpsc::UnboundedSender<RunnerEvent>) {
        self.hint_plans.retain(|_, (at, plans)| {
            at.elapsed() < HINT_PLAN_TTL
                && plans
                    .iter()
                    .all(|p| p.dependencies.iter().all(|v| v.current()))
        });
        let key = (h.feed, h.answer.to_bits());
        if !self.hint_plans.contains_key(&key) {
            self.revision
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            let mut predicted: Vec<Plan> = Vec::new();
            for &i in self.feeds.slots_for(h.feed) {
                predicted.extend(self.slots[i].adapter.predict(
                    h.feed,
                    h.answer,
                    self.cfg.executor,
                ));
            }
            predicted.sort_by(|a, b| b.bonus_usd.total_cmp(&a.bonus_usd));
            predicted.truncate(MAX_BACKRUNS_PER_UPDATE);
            let predicted = predicted.into_iter().map(|p| self.stamp_plan(p)).collect();
            self.hint_plans.insert(key, (Instant::now(), predicted));
        }
        for plan in &self.hint_plans[&key].1 {
            let _ = tx.send(RunnerEvent::AtlasBackrun {
                plan: plan.clone(),
                auction_id: h.auction_id.clone(),
                lead: h.received.elapsed(),
            });
        }
    }

    /// Run until the event receiver is dropped.
    pub async fn run(
        mut self,
        mut feed: Option<mpsc::Receiver<SequencedTx>>,
        tx: mpsc::UnboundedSender<RunnerEvent>,
    ) {
        if self.slots.is_empty() {
            // No live adapter, but the shadow pass only reads the repository: keep ticking it
            // until the event receiver is dropped.
            let Some(s) = self.shadow.clone().filter(|s| s.enabled) else {
                return;
            };
            let mut iv = tokio::time::interval(s.every);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = iv.tick() => s.spawn_pass(),
                    () = tx.closed() => return,
                }
            }
        }
        let mut hints = self.hints.take();
        let mut shadow_iv = self.shadow.as_ref().filter(|s| s.enabled).map(|s| {
            let mut iv = tokio::time::interval(s.every);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            iv
        });
        let sweep_every = Duration::from_secs(self.cfg.sweep_secs);
        let mut index_tick = tokio::time::interval(BACKFILL_EVERY);
        let mut sweep = tokio::time::interval(sweep_every);
        let mut fallback = tokio::time::interval(Duration::from_secs(self.cfg.fallback_poll_secs));
        let mut burst = tokio::time::interval(BURST_TICK);
        // Adapters whose last sweep failed (rate limits at start-up): retried on their own.
        let mut retry: Vec<usize> = Vec::new();
        let mut retry_at = Instant::now();
        let mut backfilled = false;
        loop {
            tokio::select! {
                _ = shadow::tick_opt(&mut shadow_iv) => {
                    if let Some(s) = &self.shadow { s.spawn_pass(); }
                }
                _ = index_tick.tick() => {
                    let done = self.index_all().await;
                    self.refresh_feeds();
                    if done && !backfilled {
                        backfilled = true;
                        info!("liquidation history back-filled; indexing once per sweep");
                        index_tick = tokio::time::interval(sweep_every);
                        index_tick.reset();
                    }
                }
                _ = sweep.tick() => {
                    retry = self.sweep_all(None).await;
                    retry_at = Instant::now() + SWEEP_RETRY;
                }
                _ = tokio::time::sleep_until(retry_at.into()), if !retry.is_empty() => {
                    // One retry only: a venue still failing waits for the regular sweep, so retries
                    // never keep a rate-limited endpoint saturated (a 30 s loop did, 2026-10-05).
                    let again = std::mem::take(&mut retry);
                    self.sweep_all(Some(&again)).await;
                }
                _ = fallback.tick() => {
                    for i in 0..self.slots.len() {
                        if !self.poll(i, None, &tx).await { return; }
                    }
                }
                Some(st) = recv_feed(&mut feed) => self.on_feed_tx(&st, &tx),
                Some(h) = recv_opt(&mut hints) => self.on_hint(&h, &tx),
                _ = burst.tick() => {
                    let now = Instant::now();
                    for i in 0..self.slots.len() {
                        let Some((start, next)) = self.slots[i].burst else { continue };
                        if now.duration_since(start) < BURST_OFFSETS[next] {
                            continue;
                        }
                        self.slots[i].burst = (next + 1 < BURST_OFFSETS.len()).then_some((start, next + 1));
                        let feeds = self.slots[i].burst_feeds.clone();
                        if !self.poll(i, Some(&feeds), &tx).await { return; }
                    }
                    for i in 0..self.slots.len() {
                        let due = due_delayed(&mut self.slots[i], now);
                        if !due.is_empty() && !self.poll(i, Some(&due), &tx).await { return; }
                    }
                }
            }
        }
    }
}

/// The watchlist document: per venue, the watched positions (closest to liquidation first) and,
/// across venues, the bonus that a uniform collateral price drop of each `WATCH_DROPS` size would
/// make liquidatable. "Drop" assumes every collateral price falls together and debt prices hold;
/// exact for single-collateral positions, an approximation for pooled ones.
fn watchlist_json(slots: &[Slot], now: std::time::SystemTime) -> serde_json::Value {
    use serde_json::json;
    let drop_pct = |hf: f64| ((1.0 - 1.0 / hf) * 100.0).max(0.0);
    let bonus = |d: &WatchDetail| d.max_repay_usd.zip(d.bonus_rate).map(|(r, b)| r * b);
    let mut buckets = [(0usize, 0.0f64); WATCH_DROPS.len()];
    let mut venues = serde_json::Map::new();
    for s in slots {
        let mut rows: Vec<&(Health, WatchDetail)> = s.watch.iter().collect();
        rows.sort_by(|a, b| a.0.health_factor.total_cmp(&b.0.health_factor));
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|(h, d)| {
                let drop = drop_pct(h.health_factor);
                let b = bonus(d);
                for (k, lim) in WATCH_DROPS.iter().enumerate() {
                    if drop <= *lim {
                        buckets[k].0 += 1;
                        buckets[k].1 += b.unwrap_or(0.0);
                    }
                }
                json!({
                    "account": h.account.to_string(),
                    "health_factor": h.health_factor,
                    "debt_usd": h.debt_usd,
                    "collateral_usd": h.collateral_usd,
                    "drop_to_liquidation_pct": drop,
                    "collateral": d.collateral.map(|a| a.to_string()),
                    "debt": d.debt.map(|a| a.to_string()),
                    "collateral_price_usd": d.collateral_price_usd,
                    "liquidation_price_usd": d.liquidation_price_usd,
                    "max_repay_usd": d.max_repay_usd,
                    "bonus_rate": d.bonus_rate,
                    "est_bonus_usd": b,
                })
            })
            .collect();
        venues.insert(
            s.adapter.id().to_string(),
            json!({ "positions": entries.len(), "entries": entries }),
        );
    }
    let at = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    json!({
        "generated_at_unix": at,
        "watch_hf": WATCH_HF,
        "bonus_by_price_drop": WATCH_DROPS
            .iter()
            .zip(buckets)
            .map(|(d, (n, b))| json!({ "drop_pct": d, "positions": n, "est_bonus_usd": b }))
            .collect::<Vec<_>>(),
        "venues": venues,
    })
}

/// The accounts one sweep of `s` reads: all of them on a full sweep, else the non-dormant ones.
fn sweep_set(s: &Slot) -> Vec<AccountRef> {
    let all = s.adapter.accounts();
    if s.sweeps.is_multiple_of(DORMANT_FULL_EVERY) {
        all
    } else {
        all.into_iter().filter(|a| !s.dormant.contains(a)).collect()
    }
}

async fn recv_feed(rx: &mut Option<mpsc::Receiver<SequencedTx>>) -> Option<SequencedTx> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn recv_opt<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use mev_lending::{ProtocolCall, ProtocolId};
    use std::sync::{Arc, Mutex};

    /// Scripted adapter: one account whose health factor the test controls.
    struct Fake {
        hf: Arc<Mutex<f64>>,
        status: AdapterStatus,
        feed: Address,
    }

    #[async_trait]
    impl LiquidationAdapter for Fake {
        fn id(&self) -> ProtocolId {
            "fake"
        }
        fn status(&self) -> AdapterStatus {
            self.status.clone()
        }
        async fn index(&mut self) -> anyhow::Result<bool> {
            Ok(true)
        }
        fn accounts(&self) -> Vec<AccountRef> {
            vec![AccountRef {
                market: Address::ZERO,
                account: Address::with_last_byte(1),
                sub_id: 0,
            }]
        }
        async fn health(
            &self,
            accounts: &[AccountRef],
            _fast: bool,
        ) -> anyhow::Result<Vec<Health>> {
            let hf = *self.hf.lock().unwrap();
            Ok(accounts
                .iter()
                .map(|a| Health {
                    account: *a,
                    health_factor: hf,
                    debt_usd: 100.0,
                    collateral_usd: 120.0,
                })
                .collect())
        }
        async fn plan(&self, h: &Health, _executor: Address) -> anyhow::Result<Option<Plan>> {
            Ok(Some(Plan {
                protocol: "fake",
                account: h.account,
                debt_asset: Address::ZERO,
                collateral_asset: Address::ZERO,
                flash_amount: alloy::primitives::U256::from(1),
                dependencies: vec![],
                expected_collateral: 1.0,
                bonus_usd: 5.0,
                calls: vec![ProtocolCall {
                    target: Address::ZERO,
                    data: Default::default(),
                    approve_spender: Address::ZERO,
                    approve_amount: Default::default(),
                }],
            }))
        }
        fn price_feeds(&self) -> Vec<Address> {
            vec![self.feed]
        }
        fn affected(&self, feed: Address, _a: &AccountRef) -> bool {
            feed == self.feed
        }
        /// Below $3000 (ETH/USD, 8 decimals) the fake account becomes liquidatable.
        fn predict(&self, feed: Address, answer: f64, _executor: Address) -> Vec<Plan> {
            if feed != self.feed || answer >= 3_000e8 {
                return vec![];
            }
            let h = Health {
                account: self.accounts()[0],
                health_factor: 0.99,
                debt_usd: 100.0,
                collateral_usd: 101.0,
            };
            vec![Plan {
                protocol: "fake",
                account: h.account,
                debt_asset: Address::ZERO,
                collateral_asset: Address::ZERO,
                flash_amount: alloy::primitives::U256::from(1),
                dependencies: vec![],
                expected_collateral: 1.0,
                bonus_usd: 5.0,
                calls: vec![],
            }]
        }
        fn targets(&self) -> Vec<Address> {
            vec![]
        }
        fn assets(&self) -> Vec<Address> {
            vec![]
        }
    }

    fn cfg() -> RunnerConfig {
        RunnerConfig {
            at_risk_hf: 1.05,
            min_debt_usd: 20.0,
            sweep_secs: 3600,
            fallback_poll_secs: 3600,
            executor: Address::ZERO,
        }
    }

    fn fake(hf: f64, status: AdapterStatus) -> (Arc<Mutex<f64>>, Box<dyn LiquidationAdapter>) {
        let h = Arc::new(Mutex::new(hf));
        (
            h.clone(),
            Box::new(Fake {
                hf: h,
                status,
                feed: Address::with_last_byte(0xfe),
            }),
        )
    }

    /// Adapter that learns a price feed when it indexes (a market found by indexing).
    struct Growing {
        feeds: Vec<Address>,
    }

    #[async_trait]
    impl LiquidationAdapter for Growing {
        fn id(&self) -> ProtocolId {
            "growing"
        }
        fn status(&self) -> AdapterStatus {
            AdapterStatus::Live
        }
        async fn index(&mut self) -> anyhow::Result<bool> {
            self.feeds.push(Address::with_last_byte(0xab));
            Ok(true)
        }
        fn accounts(&self) -> Vec<AccountRef> {
            vec![]
        }
        async fn health(&self, _a: &[AccountRef], _f: bool) -> anyhow::Result<Vec<Health>> {
            Ok(vec![])
        }
        async fn plan(&self, _h: &Health, _e: Address) -> anyhow::Result<Option<Plan>> {
            Ok(None)
        }
        fn price_feeds(&self) -> Vec<Address> {
            self.feeds.clone()
        }
        fn targets(&self) -> Vec<Address> {
            vec![]
        }
        fn assets(&self) -> Vec<Address> {
            vec![]
        }
    }

    #[tokio::test]
    async fn feeds_found_by_indexing_are_watched_without_a_restart() {
        let mut r = Runner::new(cfg(), vec![Box::new(Growing { feeds: vec![] })]);
        let shared = r.watched_feed_set();
        assert!(shared.read().unwrap().is_empty());
        assert!(!r.refresh_feeds(), "nothing changed yet");
        r.index_all().await;
        assert!(r.refresh_feeds());
        assert!(
            shared
                .read()
                .unwrap()
                .contains(&Address::with_last_byte(0xab))
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        r.on_feed_tx(&sequenced(Some(Address::with_last_byte(0xab))), &tx);
        assert!(matches!(rx.try_recv(), Ok(RunnerEvent::FeedTrigger { .. })));
    }

    /// Adapter whose index always fails (e.g. rate-limited RPC); counts calls.
    struct Failing(Arc<Mutex<u32>>);

    #[async_trait]
    impl LiquidationAdapter for Failing {
        fn id(&self) -> ProtocolId {
            "failing"
        }
        fn status(&self) -> AdapterStatus {
            AdapterStatus::Live
        }
        async fn index(&mut self) -> anyhow::Result<bool> {
            *self.0.lock().unwrap() += 1;
            anyhow::bail!("HTTP 429 Too Many Requests")
        }
        fn accounts(&self) -> Vec<AccountRef> {
            vec![]
        }
        async fn health(&self, _a: &[AccountRef], _f: bool) -> anyhow::Result<Vec<Health>> {
            Ok(vec![])
        }
        async fn plan(&self, _h: &Health, _e: Address) -> anyhow::Result<Option<Plan>> {
            Ok(None)
        }
        fn price_feeds(&self) -> Vec<Address> {
            vec![]
        }
        fn targets(&self) -> Vec<Address> {
            vec![]
        }
        fn assets(&self) -> Vec<Address> {
            vec![]
        }
    }

    /// Ten borrowers with settable debt; records how many accounts each sweep read.
    struct Population {
        debt: Arc<Mutex<Vec<f64>>>,
        reads: Arc<Mutex<Vec<usize>>>,
    }

    fn borrower(n: u8) -> AccountRef {
        AccountRef {
            market: Address::ZERO,
            account: Address::with_last_byte(n),
            sub_id: 0,
        }
    }

    #[async_trait]
    impl LiquidationAdapter for Population {
        fn id(&self) -> ProtocolId {
            "population"
        }
        fn status(&self) -> AdapterStatus {
            AdapterStatus::Live
        }
        async fn index(&mut self) -> anyhow::Result<bool> {
            Ok(true)
        }
        fn accounts(&self) -> Vec<AccountRef> {
            (0..10).map(borrower).collect()
        }
        async fn health(&self, a: &[AccountRef], fast: bool) -> anyhow::Result<Vec<Health>> {
            if !fast {
                self.reads.lock().unwrap().push(a.len());
            }
            let debt = self.debt.lock().unwrap();
            Ok(a.iter()
                .map(|x| Health {
                    account: *x,
                    health_factor: 2.0,
                    debt_usd: debt[usize::from(x.account.as_slice()[19])],
                    collateral_usd: 1_000.0,
                })
                .collect())
        }
        async fn plan(&self, _h: &Health, _e: Address) -> anyhow::Result<Option<Plan>> {
            Ok(None)
        }
        fn price_feeds(&self) -> Vec<Address> {
            vec![]
        }
        fn targets(&self) -> Vec<Address> {
            vec![]
        }
        fn assets(&self) -> Vec<Address> {
            vec![]
        }
    }

    #[tokio::test]
    async fn sweeps_skip_dormant_accounts_except_the_periodic_full_one() {
        // 0..7 repaid or dust (< $20), 7..10 real debt.
        let debt = Arc::new(Mutex::new(vec![
            0.0, 0.0, 0.0, 0.0, 0.0, 5.0, 19.9, 100.0, 500.0, 20.0,
        ]));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let mut r = Runner::new(
            cfg(),
            vec![Box::new(Population {
                debt: debt.clone(),
                reads: reads.clone(),
            })],
        );
        r.sweep_all(None).await;
        r.sweep_all(None).await;
        assert_eq!(
            *reads.lock().unwrap(),
            vec![10, 3],
            "first is full, then only debt"
        );
        assert_eq!(r.slots[0].dormant.len(), 7);
        // A dormant borrower takes a loan: seen again on the next full sweep, not before.
        debt.lock().unwrap()[0] = 1_000.0;
        for _ in 2..DORMANT_FULL_EVERY {
            r.sweep_all(None).await;
        }
        r.sweep_all(None).await; // sweep number DORMANT_FULL_EVERY: full
        r.sweep_all(None).await;
        let got = reads.lock().unwrap().clone();
        let n = got.len();
        assert!(got[2..n - 2].iter().all(|&x| x == 3), "{got:?}");
        assert_eq!(
            &got[n - 2..],
            &[10, 4],
            "woken by the full sweep, then kept"
        );
        assert!(!r.slots[0].dormant.contains(&borrower(0)));
    }

    #[tokio::test]
    async fn index_errors_back_off_instead_of_hammering() {
        let calls = Arc::new(Mutex::new(0));
        let mut r = Runner::new(cfg(), vec![Box::new(Failing(calls.clone()))]);
        assert!(!r.index_all().await);
        assert!(!r.index_all().await);
        assert!(!r.index_all().await);
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "retries must wait for the backoff window"
        );
        assert_eq!(r.slots[0].index_backoff, INDEX_BACKOFF_MIN * 2);
    }

    #[test]
    fn non_live_adapters_are_skipped() {
        let (_, a) = fake(1.0, AdapterStatus::Inactive("paused"));
        let r = Runner::new(cfg(), vec![a]);
        assert!(r.slots.is_empty());
        assert!(r.feeds.is_empty());
    }

    #[tokio::test]
    async fn one_successful_adapter_cannot_restore_another_failed_adapter() {
        use std::sync::atomic::Ordering::Acquire;
        let (_, a) = fake(1.02, AdapterStatus::Live);
        let (_, b) = fake(1.02, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a, b]);
        r.sweep_all(None).await;
        assert!(r.confidence.load(Acquire));
        let h = *r.slots[0].at_risk.values().next().unwrap();
        let p = r.slots[0]
            .adapter
            .plan(&h, Address::ZERO)
            .await
            .unwrap()
            .unwrap();
        let proof = r.stamp_plan(p);
        assert!(proof.dependencies.iter().all(|d| d.current()));
        r.invalidate(&[1]);
        assert!(!proof.dependencies.iter().all(|d| d.current()));
        assert!(!r.apply_sweep(1, 1, Err(anyhow::anyhow!("RPC failure"))));
        r.sweep_all(Some(&[0])).await;
        assert!(!r.confidence.load(Acquire));
        let (tx, _) = mpsc::unbounded_channel();
        r.poll(1, None, &tx).await;
        assert!(
            !r.confidence.load(Acquire),
            "subset poll cannot certify a failed full sweep"
        );
        r.sweep_all(Some(&[1])).await;
        assert!(r.confidence.load(Acquire));
        assert!(
            !proof.dependencies.iter().all(|d| d.current()),
            "restoring confidence cannot revive an old revision"
        );
    }

    #[tokio::test]
    async fn sweep_writes_watchlist_with_bonus_by_price_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("liq-watchlist.json");
        let (hf, a) = fake(1.02, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a]).with_watchlist(path.clone());
        r.sweep_all(None).await;
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let venue = doc["venues"].as_object().unwrap().values().next().unwrap();
        assert_eq!(venue["positions"], 1);
        let e = &venue["entries"][0];
        assert!(
            (e["drop_to_liquidation_pct"].as_f64().unwrap() - (1.0 - 1.0 / 1.02) * 100.0).abs()
                < 1e-9
        );
        assert!(
            e["est_bonus_usd"].is_null(),
            "no detail from this adapter: bonus unknown"
        );

        // Bucket math with a known bonus: $100 debt, half repayable at 10% = $5, unlocked by a ~2% drop.
        r.slots[0].watch[0].1 = WatchDetail {
            max_repay_usd: Some(50.0),
            bonus_rate: Some(0.10),
            ..WatchDetail::default()
        };
        let doc = watchlist_json(&r.slots, std::time::UNIX_EPOCH);
        let b = |d: f64| {
            doc["bonus_by_price_drop"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["drop_pct"].as_f64() == Some(d))
                .unwrap()["est_bonus_usd"]
                .as_f64()
                .unwrap()
        };
        assert_eq!(b(1.0), 0.0);
        assert!((b(2.0) - 5.0).abs() < 1e-9);
        assert!((b(20.0) - 5.0).abs() < 1e-9);

        // A healthy position (HF above WATCH_HF) leaves the watchlist on the next sweep.
        *hf.lock().unwrap() = 1.5;
        r.sweep_all(None).await;
        assert!(r.slots[0].watch.is_empty());
    }

    #[tokio::test]
    async fn burst_polls_only_accounts_the_updated_feed_affects() {
        let (hf, a) = fake(1.02, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a]);
        r.sweep(0).await;
        *hf.lock().unwrap() = 0.97;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let other = Address::with_last_byte(0x01);
        assert!(r.poll(0, Some(&[other]), &tx).await);
        assert!(rx.try_recv().is_err(), "unrelated feed must not re-poll");
        let watched = Address::with_last_byte(0xfe);
        assert!(r.poll(0, Some(&[other, watched]), &tx).await);
        assert!(matches!(rx.try_recv(), Ok(RunnerEvent::Plan(_))));
    }

    #[tokio::test]
    async fn sweep_then_poll_plans_liquidatable_account_once() {
        let (hf, a) = fake(1.02, AdapterStatus::Live); // at risk but healthy
        let mut r = Runner::new(cfg(), vec![a]);
        r.sweep(0).await;
        assert_eq!(
            r.slots[0].at_risk.len(),
            1,
            "account should be at risk after sweep"
        );

        *hf.lock().unwrap() = 0.97; // price drops
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(r.poll(0, None, &tx).await);
        assert!(matches!(rx.try_recv(), Ok(RunnerEvent::Plan(_))));
        assert!(r.poll(0, None, &tx).await);
        assert!(
            rx.try_recv().is_err(),
            "cooldown prevents immediate re-plan"
        );

        // Persistently liquidatable (unworkable) accounts back off exponentially...
        let acct = *r.slots[0].at_risk.keys().next().unwrap();
        r.last_plan.insert(
            acct,
            (
                Instant::now() - Duration::from_secs(100),
                Duration::from_secs(40),
            ),
        );
        assert!(r.poll(0, None, &tx).await);
        assert!(matches!(rx.try_recv(), Ok(RunnerEvent::Plan(_))));
        assert_eq!(r.last_plan[&acct].1, Duration::from_secs(80));
        // ...and the backoff resets once the account is healthy again.
        *hf.lock().unwrap() = 1.02;
        assert!(r.poll(0, None, &tx).await);
        assert!(!r.last_plan.contains_key(&acct));
    }

    #[tokio::test]
    async fn healthy_accounts_are_not_at_risk_and_dust_is_ignored() {
        let (_, a) = fake(1.20, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a]);
        r.sweep(0).await;
        assert!(r.slots[0].at_risk.is_empty());

        let (_, b) = fake(0.5, AdapterStatus::Live);
        let mut c = cfg();
        c.min_debt_usd = 1_000.0; // fake account has only $100 debt
        let mut r = Runner::new(c, vec![b]);
        r.sweep(0).await;
        assert!(r.slots[0].at_risk.is_empty());
    }

    #[test]
    fn delayed_offsets_straddle_the_unlock() {
        assert!(delayed_offsets(Duration::ZERO).is_empty());
        let o = delayed_offsets(Duration::from_secs(10));
        let ms: Vec<u64> = o.iter().map(|d| d.as_millis() as u64).collect();
        assert_eq!(ms, vec![8_000, 8_600, 9_200, 9_800, 10_400, 11_200]);
    }

    #[tokio::test]
    async fn delayed_feed_updates_schedule_polls_that_later_updates_do_not_cancel() {
        let (_, a) = fake(1.02, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a]);
        assert!(
            r.slots[0].delayed_offsets.is_empty(),
            "no delay: nothing scheduled"
        );
        r.slots[0].delayed_offsets = delayed_offsets(Duration::from_secs(10));
        let (tx, _rx) = mpsc::unbounded_channel();
        let feed = Address::with_last_byte(0xfe);
        r.on_feed_tx(&sequenced(Some(feed)), &tx);
        assert!(r.slots[0].burst.is_some(), "the immediate burst still runs");
        assert_eq!(r.slots[0].delayed.len(), 1);
        let first = r.slots[0].delayed[0].0;

        // Nothing due before the unlock window; a second update queues its own schedule.
        assert!(due_delayed(&mut r.slots[0], first + Duration::from_secs(7)).is_empty());
        r.on_feed_tx(&sequenced(Some(feed)), &tx);
        assert_eq!(r.slots[0].delayed.len(), 2);

        // Each offset fires once per schedule; finished schedules are dropped.
        assert_eq!(
            due_delayed(&mut r.slots[0], first + Duration::from_millis(8_000)),
            vec![feed]
        );
        // The second update (queued just after) reaches its own 8 s poll a moment later.
        assert_eq!(
            due_delayed(&mut r.slots[0], first + Duration::from_millis(8_100)),
            vec![feed]
        );
        assert!(due_delayed(&mut r.slots[0], first + Duration::from_millis(8_200)).is_empty());
        for _ in 0..10 {
            due_delayed(&mut r.slots[0], first + Duration::from_secs(30));
        }
        assert!(r.slots[0].delayed.is_empty());

        // Bounded: a flood of updates keeps only the newest MAX_DELAYED schedules.
        for _ in 0..(MAX_DELAYED + 3) {
            r.on_feed_tx(&sequenced(Some(feed)), &tx);
        }
        assert_eq!(r.slots[0].delayed.len(), MAX_DELAYED);
    }

    #[tokio::test]
    async fn feed_tx_to_watched_contract_starts_a_burst() {
        let (_, a) = fake(1.02, AdapterStatus::Live);
        let mut r = Runner::new(cfg(), vec![a]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let st = sequenced(Some(Address::with_last_byte(0xfe)));
        r.on_feed_tx(&st, &tx);
        assert!(r.slots[0].burst.is_some());
        assert!(matches!(
            rx.try_recv(),
            Ok(RunnerEvent::FeedTrigger {
                adapter: "fake",
                ..
            })
        ));

        // Chainlink reports arrive via per-operator forwarders: forward(aggregator, data).
        r.slots[0].burst = None;
        let mut input = vec![0x6f, 0xad, 0xcf, 0x72];
        input.extend([0u8; 12]);
        input.extend(Address::with_last_byte(0xfe).as_slice());
        let mut fwd = sequenced(Some(Address::with_last_byte(0x77)));
        fwd.input = input.into();
        r.on_feed_tx(&fwd, &tx);
        assert!(r.slots[0].burst.is_some(), "forwarded report must trigger");
        assert_eq!(r.slots[0].burst_feeds, vec![Address::with_last_byte(0xfe)]);

        let other = sequenced(Some(Address::with_last_byte(0x01)));
        r.slots[0].burst = None;
        r.on_feed_tx(&other, &tx);
        assert!(r.slots[0].burst.is_none(), "unrelated tx must not trigger");
    }

    #[tokio::test]
    async fn real_price_report_triggers_one_backrun_per_cooldown() {
        // Real Arbitrum ETH/USD report (answer $2695.91) sent through a Chainlink forwarder.
        let hex = include_str!("../../mev-oracle/tests/fixtures/eth_usd_forward_transmit.hex");
        let input: alloy::primitives::Bytes = alloy::hex::decode(hex.trim()).unwrap().into();
        let agg: Address = "0xa5E1a36938769cbd5a26f5e19D8FCB379f597c83"
            .parse()
            .unwrap();
        let fake = Fake {
            hf: Arc::new(Mutex::new(1.02)),
            status: AdapterStatus::Live,
            feed: agg,
        };
        let mut r = Runner::new(cfg(), vec![Box::new(fake)]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut st = sequenced(Some(Address::with_last_byte(0xf0))); // forwarder
        st.input = input;
        r.on_feed_tx(&st, &tx);
        assert!(
            matches!(rx.try_recv(), Ok(RunnerEvent::Backrun { .. })),
            "backrun first"
        );
        assert!(matches!(rx.try_recv(), Ok(RunnerEvent::FeedTrigger { .. })));
        assert!(r.slots[0].burst.is_some(), "the re-poll burst still runs");
        r.on_feed_tx(&st, &tx);
        assert!(
            matches!(rx.try_recv(), Ok(RunnerEvent::FeedTrigger { .. })),
            "same account within the cooldown: no second backrun"
        );
    }

    #[tokio::test]
    async fn a_held_svr_feed_is_re_polled_but_never_backrun() {
        let hex = include_str!("../../mev-oracle/tests/fixtures/eth_usd_forward_transmit.hex");
        let input: alloy::primitives::Bytes = alloy::hex::decode(hex.trim()).unwrap().into();
        let agg: Address = "0xa5E1a36938769cbd5a26f5e19D8FCB379f597c83"
            .parse()
            .unwrap();
        let fake = Fake {
            hf: Arc::new(Mutex::new(1.02)),
            status: AdapterStatus::Live,
            feed: agg,
        };
        let mut r = Runner::new(cfg(), vec![Box::new(fake)]).with_held_feeds(HashSet::from([agg]));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut st = sequenced(Some(Address::with_last_byte(0xf0)));
        st.input = input;
        r.on_feed_tx(&st, &tx);
        assert!(
            matches!(rx.try_recv(), Ok(RunnerEvent::FeedTrigger { .. })),
            "a trigger, not a backrun"
        );
        assert!(rx.try_recv().is_err());
        assert!(r.slots[0].burst.is_some(), "the re-poll burst still runs");
    }

    #[tokio::test]
    async fn auction_hints_predict_once_and_fan_out_to_each_auction() {
        let agg: Address = "0xa5E1a36938769cbd5a26f5e19D8FCB379f597c83"
            .parse()
            .unwrap();
        let fake = Fake {
            hf: Arc::new(Mutex::new(1.02)),
            status: AdapterStatus::Live,
            feed: agg,
        };
        let mut r = Runner::new(cfg(), vec![Box::new(fake)]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hint = |id: &str| OracleHint {
            feed: agg,
            answer: 269_591_000_000.0,
            auction_id: id.into(),
            received: Instant::now(),
        };
        r.on_hint(&hint("a"), &tx);
        r.on_hint(&hint("b"), &tx); // the parallel auction of the same update
        let ids: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                RunnerEvent::AtlasBackrun { auction_id, .. } => Some(auction_id),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec!["a", "b"],
            "one plan per auction, both auctions served"
        );
        // A feed nobody watches predicts nothing.
        let mut other = hint("c");
        other.feed = Address::with_last_byte(9);
        r.on_hint(&other, &tx);
        assert!(rx.try_recv().is_err());
    }

    fn sequenced(to: Option<Address>) -> SequencedTx {
        use alloy::{
            consensus::{SignableTransaction, TxEip1559, TxEnvelope},
            network::TxSignerSync,
            primitives::TxKind,
            signers::local::PrivateKeySigner,
        };
        let signer = PrivateKeySigner::random();
        let mut t = TxEip1559 {
            chain_id: 42161,
            to: TxKind::Call(to.unwrap_or_default()),
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut t).unwrap();
        let env = TxEnvelope::Eip1559(t.into_signed(sig));
        SequencedTx {
            seq: 1,
            received: Instant::now(),
            hash: *env.tx_hash(),
            to,
            input: Default::default(),
            tx: env,
        }
    }

    fn shadow_for(
        enabled: bool,
        events: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    ) -> (tempfile::TempDir, ShadowSchedule) {
        let dir = tempfile::tempdir().unwrap();
        let repo =
            Arc::new(mev_liq_state::SnapshotRepository::open(&dir.path().join("s.db")).unwrap());
        let pool = Address::repeat_byte(0xaa);
        repo.register_borrowers(42161, pool, &[Address::with_last_byte(9)], None, 1)
            .unwrap();
        let params = mev_liq_sched::ShadowParams {
            chain_id: 42161,
            pool,
            scan_name: "live".into(),
            policy: mev_liq_sched::PolicyConfig::default(),
            max_records: 50,
            bounds: None,
        };
        let sink: ShadowSink =
            Arc::new(move |k, d| events.lock().unwrap().push((k.to_string(), d)));
        let s = ShadowSchedule::new(repo, params)
            .enabled(enabled)
            .every(Duration::from_millis(100))
            .with_sink(sink);
        (dir, s)
    }

    #[tokio::test]
    async fn shadow_schedule_is_recorded_from_the_runner_loop_without_changing_dispatch() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (_d, sched) = shadow_for(true, events.clone());
        let (_hf, a) = fake(0.97, AdapterStatus::Live);
        let c = RunnerConfig {
            fallback_poll_secs: 1,
            ..cfg()
        };
        let r = Runner::new(c, vec![a]).with_shadow_schedule(sched);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(r.run(None, tx));
        // the runner still plans the liquidatable account exactly as without the shadow pass
        let plan = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(RunnerEvent::Plan(p)) = rx.recv().await {
                    break p;
                }
            }
        })
        .await
        .expect("plan");
        assert_eq!(plan.protocol, "fake");
        tokio::time::sleep(Duration::from_millis(400)).await;
        h.abort();
        let ev = events.lock().unwrap();
        let sched_ev = ev
            .iter()
            .find(|(k, _)| k == EVENT_SCHEDULE)
            .expect("schedule event");
        assert_eq!(sched_ev.1["jobs"][0]["decision"]["tier"], "P1");
        assert!(
            ev.iter()
                .any(|(k, d)| k == EVENT_COUNTERS && d["candidates"]["p1"] == 1)
        );
    }

    #[tokio::test]
    async fn shadow_schedule_is_off_unless_enabled() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (_d, sched) = shadow_for(false, events.clone());
        sched.spawn_pass();
        let (_hf, a) = fake(1.5, AdapterStatus::Live);
        let r = Runner::new(cfg(), vec![a]).with_shadow_schedule(sched);
        let (tx, _rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(r.run(None, tx));
        tokio::time::sleep(Duration::from_millis(500)).await;
        h.abort();
        assert!(events.lock().unwrap().is_empty());
    }

    fn advisory_hook(mode: mev_liq_sched::AdvisoryMode) -> AdvisoryShadow {
        AdvisoryShadow {
            policy: mev_liq_sched::AdvisoryPolicy::new(
                mode,
                mev_liq_sched::PolicyConfig::default(),
                mev_liq_sched::AdvisoryConfig::new("policy-v1", "features-v1", vec!["m".into()]),
                mev_liq_sched::JevHealth::Healthy,
            )
            .unwrap(),
            decisions: Arc::new(|_, _| mev_liq_sched::AdvisoryInputs::default()),
            health: Arc::new(|| mev_liq_sched::JevHealth::Healthy),
        }
    }

    async fn run_pass(sched: &ShadowSchedule) {
        sched.spawn_pass();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    #[tokio::test]
    async fn advisory_shadow_emits_its_event_and_leaves_the_schedule_event_unchanged() {
        let plain = Arc::new(Mutex::new(Vec::new()));
        let (_d1, s1) = shadow_for(true, plain.clone());
        run_pass(&s1).await;
        let adv = Arc::new(Mutex::new(Vec::new()));
        let (_d2, s2) = shadow_for(true, adv.clone());
        run_pass(&s2.with_advisory(advisory_hook(mev_liq_sched::AdvisoryMode::Shadow))).await;

        let (p, a) = (plain.lock().unwrap(), adv.lock().unwrap());
        let jobs = |ev: &[(String, serde_json::Value)]| {
            ev.iter().find(|(k, _)| k == EVENT_SCHEDULE).unwrap().1["jobs"].clone()
        };
        assert_eq!(jobs(&p), jobs(&a));
        assert!(!p.iter().any(|(k, _)| k == EVENT_ADVISORY));
        let rep = &a
            .iter()
            .find(|(k, _)| k == EVENT_ADVISORY)
            .expect("advisory event")
            .1;
        assert_eq!(rep["mode"], "Shadow");
        assert_eq!(rep["mandatory_unchanged"], true);
        assert!(rep["counters"]["copper_jev_fallback_total"].is_object());
    }

    #[tokio::test]
    async fn advisory_off_emits_no_advisory_event() {
        let ev = Arc::new(Mutex::new(Vec::new()));
        let (_d, s) = shadow_for(true, ev.clone());
        run_pass(&s.with_advisory(advisory_hook(mev_liq_sched::AdvisoryMode::Off))).await;
        let ev = ev.lock().unwrap();
        assert!(ev.iter().any(|(k, _)| k == EVENT_SCHEDULE));
        assert!(!ev.iter().any(|(k, _)| k == EVENT_ADVISORY));
    }
}
