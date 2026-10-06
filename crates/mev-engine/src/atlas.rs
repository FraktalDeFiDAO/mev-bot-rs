//! Optional Chainlink SVR / FastLane Atlas solver (config `[atlas]`, off by default).
//!
//! Listens to the auction feed. Each auction on this chain announces the oracle update a solver can
//! bundle with its own call; the hints (feed address and new price) go to the liquidation runner,
//! which predicts the positions that price makes liquidatable, and for each the engine builds a
//! `liquidate` call for the `AtlasSolver` contract, offers a share of the expected profit as the bid
//! and submits the signed solver operation. Observe-only unless `bid` is set. See
//! `docs/research/ATLAS.md` for the funding steps that bring bidding online.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use alloy::{
    primitives::{Address, Bytes, U256},
    signers::local::PrivateKeySigner,
};
use mev_atlas::{Auction, client, sign};
use mev_config::{AtlasConfig, Secrets};
use mev_liquidate::OracleHint;
use mev_rpc::Rpc;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{BACKRUN_BONUS_HAIRCUT, BACKRUN_GAS, BACKRUN_GAS_LIMIT, Engine, Plan, now_secs};

/// Auctions older than this are forgotten (the window is a couple of seconds).
const AUCTION_TTL: Duration = Duration::from_secs(60);
/// A gas price above this (wei) in an auction is not one we'd budget gas for: skip it.
const MAX_AUCTION_GAS_PRICE: u128 = 2_000_000_000;

pub(crate) struct Hub {
    cfg: AtlasConfig,
    chain_id: u64,
    /// The bonded EOA; the `AtlasSolver`'s owner (same key as the bot wallet).
    signer: Option<PrivateKeySigner>,
    pub(crate) auction_rx: Option<mpsc::UnboundedReceiver<Auction>>,
    pub(crate) event_rx: Option<mpsc::UnboundedReceiver<client::Event>>,
    submit_tx: mpsc::UnboundedSender<String>,
    hint_tx: mpsc::UnboundedSender<OracleHint>,
    auctions: HashMap<String, (Instant, Auction)>,
    /// Feeds the liquidation adapters watch.
    watched: HashSet<Address>,
    next_id: u64,
    solver_gas_limit: u64,
    other_chain: u64,
}

/// Connect to the auction feed. Returns `None` when `[atlas] enabled` is false.
pub(crate) async fn setup(
    cfg: &mev_config::ChainConfig,
    secrets: &Secrets,
    rpc: &Rpc,
    hint_tx: mpsc::UnboundedSender<OracleHint>,
    watched: Vec<Address>,
) -> Option<Hub> {
    let a = cfg.atlas.clone();
    if !a.enabled {
        return None;
    }
    let signer = secrets
        .private_key
        .as_deref()
        .and_then(|k| k.trim().parse::<PrivateKeySigner>().ok());
    if a.bid && (signer.is_none() || a.solver.is_none()) {
        warn!("atlas: bid is on but PRIVATE_KEY or [atlas] solver is missing; observing only");
    }
    let solver_gas_limit = read_solver_gas_limit(rpc, a.solver_gas_limit).await;
    let (auction_tx, auction_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (submit_tx, submit_rx) = mpsc::unbounded_channel();
    tokio::spawn(client::run(a.url.clone(), auction_tx, event_tx, submit_rx));
    info!(
        bid = a.bid,
        solver = ?a.solver,
        watched_feeds = watched.len(),
        solver_gas_limit,
        "atlas solver: listening to SVR auctions"
    );
    Some(Hub {
        cfg: a,
        chain_id: cfg.chain_id,
        signer,
        auction_rx: Some(auction_rx),
        event_rx: Some(event_rx),
        submit_tx,
        hint_tx,
        auctions: HashMap::new(),
        watched: watched.into_iter().collect(),
        next_id: 100,
        solver_gas_limit,
        other_chain: 0,
    })
}

alloy::sol! {
    #[sol(rpc)]
    interface IDappControl {
        function getSolverGasLimit() external view returns (uint32);
    }
}

/// `DappControl.getSolverGasLimit()`; the configured fallback if it can't be read.
async fn read_solver_gas_limit(rpc: &Rpc, fallback: u64) -> u64 {
    match IDappControl::new(mev_atlas::ARBITRUM_CONTROL, &rpc.http)
        .getSolverGasLimit()
        .call()
        .await
    {
        Ok(v) if v > 0 => u64::from(v),
        _ => fallback,
    }
}

impl Engine {
    /// An auction arrived: log it, remember it, and pass its oracle update to the runner.
    pub(crate) fn on_atlas_auction(&mut self, a: Auction) {
        let Some(hub) = self.atlas.as_mut() else {
            return;
        };
        if a.chain_id != hub.chain_id {
            hub.other_chain += 1;
            return;
        }
        hub.auctions.retain(|_, (t, _)| t.elapsed() < AUCTION_TTL);
        let hint = a.hints.as_ref().map(|h| {
            let answer: f64 = h.median_price.to_string().parse().unwrap_or(0.0);
            (h.aggregator, answer, hub.watched.contains(&h.aggregator))
        });
        self.metrics.event(
            "atlas_auction",
            json!({
                "id": a.id, "feed": hint.map(|h| h.0), "answer": a.hints.as_ref().map(|h| h.median_price.to_string()),
                "watched": hint.map(|h| h.2), "max_fee_per_gas": a.max_fee_per_gas.to_string(),
                "deadline": a.deadline.to_string(), "control": a.control,
            }),
        );
        if let Some((feed, answer, true)) = hint
            && answer > 0.0
        {
            let _ = hub.hint_tx.send(OracleHint {
                feed,
                answer,
                auction_id: a.id.clone(),
                received: Instant::now(),
            });
        }
        hub.auctions.insert(a.id.clone(), (Instant::now(), a));
    }

    pub(crate) fn on_atlas_event(&mut self, ev: client::Event) {
        match ev {
            client::Event::Connected => self.metrics.event("atlas_conn", json!({ "up": true })),
            client::Event::Disconnected(why) => self
                .metrics
                .event("atlas_conn", json!({ "up": false, "why": why })),
            client::Event::Submitted { id, error } => self
                .metrics
                .event("atlas_submit_reply", json!({ "id": id, "error": error })),
        }
    }

    /// The runner predicted a liquidation the auction's oracle update makes possible.
    pub(crate) async fn on_atlas_backrun(
        &mut self,
        plan: Plan,
        auction_id: String,
        lead: Duration,
    ) {
        let Some(auction) = self
            .atlas
            .as_ref()
            .and_then(|h| h.auctions.get(&auction_id))
            .map(|(_, a)| a.clone())
        else {
            return;
        };
        if auction.max_fee_per_gas > U256::from(MAX_AUCTION_GAS_PRICE) {
            return;
        }
        let Some((hops, base, aave_pool)) = self.liq_precheck(&plan).await else {
            return;
        };
        let (Some(debt_px), Some(native_px)) =
            (self.price(plan.debt_asset), self.price(self.native))
        else {
            return self.metrics.event("liq_no_price", base);
        };
        // The oracle fixes the gas price every solver operation pays.
        let gas_price = u128::try_from(auction.max_fee_per_gas).unwrap_or(0) as f64;
        let gas_usd = BACKRUN_GAS as f64 * gas_price * native_px;
        let gross = plan.bonus_usd * BACKRUN_BONUS_HAIRCUT;
        let net_usd = gross - gas_usd;
        let ev = json!({ "auction": auction.id, "liq": base, "net_usd": net_usd, "gas_usd": gas_usd,
                         "lead_ms": lead.as_millis() as u64 });
        if net_usd <= 0.0 {
            return self.metrics.event("atlas_unprofitable", ev);
        }
        let Some(hub) = self.atlas.as_mut() else {
            return;
        };
        let (Some(signer), Some(solver)) = (hub.signer.clone(), hub.cfg.solver) else {
            return self.metrics.event("atlas_would_bid", ev);
        };
        if !hub.cfg.bid {
            return self.metrics.event("atlas_would_bid", ev);
        }
        // Shadow mode / a reached daily cap, and the kill switch, hold bids back like any send.
        if !self.sending(false) {
            return self.metrics.event("atlas_would_bid", ev);
        }
        if let Err(block) = self.risk.check_concurrent(net_usd, gas_usd, now_secs()) {
            self.on_cap(&block, false);
            return self.metrics.event(
                "risk_block",
                json!({ "atlas": ev, "reason": format!("{block:?}") }),
            );
        }
        let hub = self.atlas.as_mut().expect("checked above");
        let bid_usd = net_usd * hub.cfg.bid_fraction.clamp(0.0, 1.0);
        let max_bid_wei = hub.cfg.max_bid_eth.max(0.0) * 1e18;
        let bid_wei = (bid_usd / native_px).min(max_bid_wei);
        if bid_wei < 1.0 {
            return self.metrics.event("atlas_would_bid", ev);
        }
        // On-chain floor: the liquidation must repay the bid (paid from the contract's WETH float)
        // and the gas, or the whole operation reverts and nothing is paid.
        let min_profit = U256::from(((gas_usd + bid_wei * native_px) / debt_px) as u128);
        let calldata = mev_bindings::liquidation_calldata(&plan, &hops, min_profit, aave_pool);
        let gas = hub.solver_gas_limit.min(BACKRUN_GAS_LIMIT);
        let op = sign::build(sign::OpParams {
            auction: &auction,
            from: signer.address(),
            solver,
            gas: U256::from(gas),
            bid_amount: U256::from(bid_wei as u128),
            data: Bytes::from(calldata),
        });
        let signed = match sign::sign(op, &signer, hub.chain_id) {
            Ok(s) => s,
            Err(e) => return warn!(error = %format!("{e:#}"), "atlas: signing failed"),
        };
        let id = hub.next_id;
        hub.next_id += 1;
        let _ = hub
            .submit_tx
            .send(mev_atlas::submit_frame(id, &auction.id, &signed));
        self.metrics.event(
            "atlas_bid",
            json!({ "id": id, "auction": auction.id, "liq": ev, "bid_wei": (bid_wei as u128).to_string(),
                    "bid_usd": bid_usd, "min_profit": min_profit.to_string() }),
        );
        info!(auction = %auction.id, protocol = plan.protocol, account = %plan.account, bid_usd, "atlas: bid submitted");
    }
}
