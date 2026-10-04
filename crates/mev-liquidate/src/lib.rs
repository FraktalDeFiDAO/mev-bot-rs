//! Protocol-agnostic liquidation runner.
//!
//! Drives any set of [`LiquidationAdapter`]s: incremental account indexing, periodic health sweeps
//! (sweep RPC), and at-risk polling (fast RPC). Polling is event-driven: when the sequencer feed
//! shows a transaction to a price feed an adapter depends on, that adapter's at-risk accounts are
//! re-checked in a short burst (state lands ~0.5–1 s after the feed). A slow fallback poll covers
//! anything the feed misses. Liquidatable accounts are planned by their adapter and sent out.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use alloy::primitives::Address;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan};
use mev_oracle::FeedWatcher;
use mev_sequencer::SequencedTx;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Index more often while history is being back-filled.
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
/// Don't re-plan the same account more often than this.
/// Re-plan cooldown per account: starts here and doubles each time the same account is still
/// liquidatable (a real opportunity gets liquidated and disappears; one that persists is
/// unworkable — bad debt, no route, too large — and each re-plan costs paid RPC calls).
const REPLAN_COOLDOWN: Duration = Duration::from_secs(3);
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
    /// Liquidatable but nothing worth taking (bad debt, dust, unsupported shape).
    NoPlan(Health),
    /// Price-feed transaction seen in the sequencer feed (for metrics).
    FeedTrigger {
        adapter: &'static str,
        lead: Duration,
    },
}

struct Slot {
    adapter: Box<dyn LiquidationAdapter>,
    at_risk: HashMap<AccountRef, Health>,
    backfilled: bool,
    /// Active burst: when it started and the next offset index to poll.
    burst: Option<(Instant, usize)>,
    /// Feed contracts updated since the burst started: only accounts they affect are re-polled.
    burst_feeds: Vec<Address>,
    /// After an index error, don't retry before this (exponential backoff; rate limits).
    index_retry_at: Option<Instant>,
    index_backoff: Duration,
}

const INDEX_BACKOFF_MIN: Duration = Duration::from_secs(15);
const INDEX_BACKOFF_MAX: Duration = Duration::from_secs(600);

pub struct Runner {
    cfg: RunnerConfig,
    slots: Vec<Slot>,
    feeds: FeedWatcher<usize>,
    /// When each account was last planned and its current cooldown.
    last_plan: HashMap<AccountRef, (Instant, Duration)>,
    /// When each account was last backrun (one attempt per account per `BACKRUN_COOLDOWN`).
    last_backrun: HashMap<AccountRef, Instant>,
}

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
            slots.push(Slot {
                adapter,
                at_risk: HashMap::new(),
                backfilled: false,
                burst: None,
                burst_feeds: Vec::new(),
                index_retry_at: None,
                index_backoff: INDEX_BACKOFF_MIN,
            });
        }
        info!(
            adapters = slots.len(),
            watched_feeds = feeds.len(),
            "liquidation runner ready"
        );
        Self {
            cfg,
            slots,
            feeds,
            last_plan: HashMap::new(),
            last_backrun: HashMap::new(),
        }
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

    /// Health-sweep every adapter concurrently (one slow venue must not delay the others).
    async fn sweep_all(&mut self) {
        let jobs = self.slots.iter().map(|s| async move {
            let accounts = s.adapter.accounts();
            let res = s.adapter.health(&accounts, false).await;
            (accounts.len(), res)
        });
        let results = futures::future::join_all(jobs).await;
        for (i, (n, res)) in results.into_iter().enumerate() {
            self.apply_sweep(i, n, res);
        }
    }

    #[cfg(test)]
    async fn sweep(&mut self, i: usize) {
        let accounts = self.slots[i].adapter.accounts();
        let res = self.slots[i].adapter.health(&accounts, false).await;
        self.apply_sweep(i, accounts.len(), res);
    }

    fn apply_sweep(&mut self, i: usize, accounts: usize, res: anyhow::Result<Vec<Health>>) {
        let id = self.slots[i].adapter.id();
        match res {
            Ok(hs) => {
                let at_risk: HashMap<AccountRef, Health> = hs
                    .into_iter()
                    .filter(|h| h.health_factor < self.cfg.at_risk_hf && self.worth_watching(h))
                    .map(|h| (h.account, h))
                    .collect();
                let liquidatable = at_risk.values().filter(|h| h.liquidatable()).count();
                info!(
                    protocol = id,
                    accounts,
                    at_risk = at_risk.len(),
                    liquidatable,
                    "sweep"
                );
                self.slots[i].at_risk = at_risk;
            }
            Err(e) => warn!(protocol = id, error = %format!("{e:#}"), "sweep failed"),
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
        let hs = match self.slots[i].adapter.health(&accounts, true).await {
            Ok(h) => h,
            Err(e) => {
                warn!(protocol = self.slots[i].adapter.id(), error = %format!("{e:#}"), "poll failed");
                return true;
            }
        };
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
                Ok(Some(p)) => RunnerEvent::Plan(p),
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
        let report = mev_oracle::report_answer(st.to, &st.input).filter(|(agg, _)| *agg == feed);
        let mut predicted: Vec<Plan> = Vec::new();
        for i in hits.iter().copied() {
            let Some((_, answer)) = report else { break };
            predicted.extend(self.slots[i].adapter.predict(feed, answer));
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
                let _ = tx.send(RunnerEvent::Backrun { plan, lead });
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
            debug!(protocol = self.slots[i].adapter.id(), tx = %st.hash, "price-feed update in sequencer feed");
            let _ = tx.send(RunnerEvent::FeedTrigger {
                adapter: self.slots[i].adapter.id(),
                lead: st.received.elapsed(),
            });
        }
    }

    /// Run until the event receiver is dropped.
    pub async fn run(
        mut self,
        mut feed: Option<mpsc::UnboundedReceiver<SequencedTx>>,
        tx: mpsc::UnboundedSender<RunnerEvent>,
    ) {
        if self.slots.is_empty() {
            return;
        }
        let sweep_every = Duration::from_secs(self.cfg.sweep_secs);
        let mut index_tick = tokio::time::interval(BACKFILL_EVERY);
        let mut sweep = tokio::time::interval(sweep_every);
        let mut fallback = tokio::time::interval(Duration::from_secs(self.cfg.fallback_poll_secs));
        let mut burst = tokio::time::interval(BURST_TICK);
        let mut backfilled = false;
        loop {
            tokio::select! {
                _ = index_tick.tick() => {
                    if self.index_all().await && !backfilled {
                        backfilled = true;
                        info!("liquidation history back-filled; indexing once per sweep");
                        index_tick = tokio::time::interval(sweep_every);
                        index_tick.reset();
                    }
                }
                _ = sweep.tick() => self.sweep_all().await,
                _ = fallback.tick() => {
                    for i in 0..self.slots.len() {
                        if !self.poll(i, None, &tx).await { return; }
                    }
                }
                Some(st) = recv_opt(&mut feed) => self.on_feed_tx(&st, &tx),
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
                }
            }
        }
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
        fn predict(&self, feed: Address, answer: f64) -> Vec<Plan> {
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
}
