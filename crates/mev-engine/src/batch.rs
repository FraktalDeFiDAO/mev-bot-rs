//! Batching (config `[batch]`, off by default): trades that become ready within a short window
//! (liquidations, backruns and arbitrages alike) go out as ONE transaction through the executor's
//! `batch`. One nonce, one base fee and one priority-fee bid cover the lot, and each trade still
//! succeeds or fails on its own (its own route checks, allowlist and `minProfit`).
//!
//! A trade is decided exactly as it is for a single send (risk checks, busy pools, profit); only
//! the final "bid and send" step is deferred to the flush. The bid is sized for the batch: the sum
//! of the trades' expected profit and gas, so the tip is split across them and each trade's on-chain
//! `minProfit` covers its own gas plus its share of the tip.

use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use mev_exec::Landed;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{Engine, Mode, TxDone, now_secs};

/// Extra gas allowed per batched trade for the self-call and event.
const PER_ITEM_OVERHEAD_GAS: u64 = 25_000;

/// One trade waiting for the flush.
pub(crate) struct Item {
    pub kind: &'static str,
    /// Executor calldata for this trade given its on-chain `minProfit` (token units).
    pub build: Box<dyn FnOnce(U256) -> Vec<u8> + Send>,
    pub gas: u64,
    pub gas_limit: u64,
    pub gas_usd: f64,
    /// Expected profit before any gas (what the bid policy sizes the tip from).
    pub gross_usd: f64,
    /// Expected profit credited if it lands.
    pub est_profit_usd: f64,
    /// USD per token unit of the trade's profit token.
    pub px: f64,
    /// Arbitrage route whose pools were claimed (released when it lands or fails).
    pub route: Option<(Address, Address)>,
    pub seen: Instant,
}

#[derive(Default)]
pub(crate) struct Queue {
    items: Vec<Item>,
    since: Option<Instant>,
}

/// What `on_tx_done` needs to know about a trade after its transaction landed.
#[derive(Debug, Clone)]
pub(crate) struct Done {
    pub kind: &'static str,
    pub est_profit_usd: f64,
    pub route: Option<(Address, Address)>,
}

/// Each trade's on-chain `minProfit` in its token's units: its own gas plus its gas-weighted share
/// of the shared tip, converted at its token price. `parts` = (gas_usd, gas, px).
pub(crate) fn min_profits(parts: &[(f64, u64, f64)], tip_usd: f64) -> Vec<u128> {
    let total_gas: u64 = parts.iter().map(|p| p.1).sum();
    parts
        .iter()
        .map(|&(gas_usd, gas, px)| {
            let share = if total_gas > 0 {
                tip_usd * gas as f64 / total_gas as f64
            } else {
                0.0
            };
            if px > 0.0 {
                ((gas_usd + share) / px) as u128
            } else {
                0
            }
        })
        .collect()
}

/// Per-trade success: the executor's `BatchItem` events when present, else the whole tx's status.
pub(crate) fn item_results(landed: &Landed, n: usize) -> Vec<bool> {
    if landed.batch_ok.len() == n {
        landed.batch_ok.clone()
    } else {
        vec![landed.success; n]
    }
}

impl Engine {
    /// Batching is on, supported by the deployed executor, and we are sending for real.
    pub(crate) fn batch_on(&self) -> bool {
        self.mode == Mode::Live
            && self.cfg.batch.enabled
            && self.cfg.executor_batch
            && self.sender.is_some()
    }

    /// When the queue must be flushed, if it has anything in it.
    pub(crate) fn batch_deadline(&self) -> Option<Instant> {
        self.batch
            .since
            .map(|t| t + Duration::from_millis(self.cfg.batch.window_ms))
    }

    /// Queue a decided trade; flush now when the batch is full.
    pub(crate) async fn enqueue(&mut self, item: Item, tx_done: &mpsc::UnboundedSender<TxDone>) {
        if self.batch.items.is_empty() {
            self.batch.since = Some(Instant::now());
        }
        self.batch.items.push(item);
        let max = self.cfg.batch.max_items.clamp(1, mev_bindings::MAX_BATCH);
        if self.batch.items.len() >= max {
            self.flush_batch(tx_done).await;
        }
    }

    /// Bid once for everything queued and send it.
    pub(crate) async fn flush_batch(&mut self, tx_done: &mpsc::UnboundedSender<TxDone>) {
        let items = std::mem::take(&mut self.batch.items);
        self.batch.since = None;
        if items.is_empty() {
            return;
        }
        let release = |s: &mut Engine, items: &[Item]| {
            for it in items {
                if let Some(r) = it.route {
                    s.release_pools(r);
                }
            }
        };
        let Some(sender) = self.sender.clone() else {
            return release(self, &items);
        };
        let native_px = self.price(self.native).unwrap_or(0.0);
        let gas_price = self.gas_price().await;
        let gross: f64 = items.iter().map(|i| i.gross_usd).sum();
        let gas: u64 = items.iter().map(|i| i.gas).sum();
        let kinds: Vec<&str> = items.iter().map(|i| i.kind).collect();
        let ev = json!({ "batch": items.len(), "kinds": kinds, "gross_usd": gross });
        let Some((fees, tip_usd)) = self.pga_bid(gross, gas, gas_price, native_px, &ev) else {
            return release(self, &items);
        };
        let parts: Vec<(f64, u64, f64)> = items.iter().map(|i| (i.gas_usd, i.gas, i.px)).collect();
        let mins = min_profits(&parts, tip_usd);
        let gas_limit: u64 = items
            .iter()
            .map(|i| i.gas_limit + PER_ITEM_OVERHEAD_GAS)
            .sum::<u64>();
        let done: Vec<Done> = items
            .iter()
            .map(|i| Done {
                kind: i.kind,
                est_profit_usd: i.est_profit_usd,
                route: i.route,
            })
            .collect();
        let seen = items
            .iter()
            .map(|i| i.seen)
            .min()
            .unwrap_or_else(Instant::now);
        let mut calldatas: Vec<Vec<u8>> = items
            .into_iter()
            .zip(mins)
            .map(|(it, mp)| (it.build)(U256::from(mp)))
            .collect();
        self.risk.mark_sent();
        self.metrics.update(|c| c.sent += 1);
        let (tx_done, metrics) = (tx_done.clone(), self.metrics.clone());
        let n = calldatas.len();
        tokio::spawn(async move {
            let sent = if n == 1 {
                sender
                    .send_prebuilt(calldatas.pop().expect("one item"), gas_limit, fees)
                    .await
            } else {
                sender.send_batch(calldatas, gas_limit, fees).await
            };
            let sent = match sent {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx_done.send(TxDone::BatchFailed {
                        hash: None,
                        items: done,
                        error: e.to_string(),
                    });
                    return;
                }
            };
            let hash = sent.hash;
            let decide_ms = super::race::ms(seen, sent.signed);
            metrics.event(
                "batch_sent",
                json!({ "hash": hash, "items": n, "decide_ms": decide_ms, "tip_usd": tip_usd,
                        "kinds": done.iter().map(|d| d.kind).collect::<Vec<_>>() }),
            );
            info!(%hash, items = n, decide_ms, tip_usd = format!("{tip_usd:.4}"), "batch sent");
            match sender.wait(hash, Duration::from_secs(30)).await {
                Ok(landed) => {
                    let _ = tx_done.send(TxDone::BatchLanded {
                        hash,
                        landed,
                        items: done,
                    });
                }
                Err(e) => {
                    let _ = tx_done.send(TxDone::BatchFailed {
                        hash: Some(hash),
                        items: done,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    /// A batch (or single deferred trade) landed: settle every trade in it, the gas once.
    pub(crate) fn on_batch_landed(
        &mut self,
        hash: alloy::primitives::TxHash,
        landed: Landed,
        items: Vec<Done>,
    ) {
        let native_px = self.price(self.native).unwrap_or(0.0);
        let oks = item_results(&landed, items.len());
        for (it, ok) in items.iter().zip(&oks) {
            if let Some(r) = it.route {
                self.release_pools(r);
                if !ok {
                    // Lost race or mispriced: don't retry the route straight away.
                    self.cooldown.insert(r, Instant::now());
                }
            }
        }
        let gas_usd = landed.gas_used as f64 * landed.effective_gas_price as f64 * native_px;
        if items.iter().any(|i| i.kind == "arb") {
            self.risk.record_arb_gas(gas_usd, now_secs());
        }
        if !landed.success && items.iter().any(|i| i.kind != "liquidation") {
            self.risk.record_lost_race(gas_usd, now_secs());
        } else {
            self.risk.record(landed.success, gas_usd, now_secs());
        }
        let won: f64 = items
            .iter()
            .zip(&oks)
            .filter(|(_, ok)| **ok)
            .map(|(i, _)| i.est_profit_usd)
            .sum();
        if landed.success {
            self.risk.record_profit(won - gas_usd, now_secs());
        }
        let bidding = self.cfg.pga.enabled;
        self.metrics.update(|c| {
            if bidding {
                if landed.success {
                    c.bids_won += 1
                } else {
                    c.bids_lost += 1
                }
            }
            c.gas_usd += gas_usd;
            if landed.success {
                c.landed_ok += 1;
                c.profit_usd += won;
            } else {
                c.landed_reverted += 1;
            }
        });
        self.metrics.event(
            "landed",
            json!({ "hash": hash, "success": landed.success, "kind": "batch", "items": items.len(),
                    "item_ok": oks, "gas_used": landed.gas_used, "gas_usd": gas_usd,
                    "block": landed.block, "est_profit_usd": won }),
        );
        info!(%hash, success = landed.success, items = items.len(), gas_usd = format!("{gas_usd:.4}"), "batch landed");
    }

    /// A batch never landed (send or receipt failure): free its pools and count the failure.
    pub(crate) fn on_batch_failed(
        &mut self,
        hash: Option<alloy::primitives::TxHash>,
        items: Vec<Done>,
        error: String,
    ) {
        for it in &items {
            if let Some(r) = it.route {
                self.release_pools(r);
            }
        }
        self.risk.record(false, 0.0, now_secs());
        self.metrics.event(
            "send_failed",
            json!({ "hash": hash, "error": error, "items": items.len() }),
        );
        warn!(?hash, %error, items = items.len(), "batch send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tip_is_split_by_gas_and_each_trade_covers_its_share() {
        // Two trades, 300k and 100k gas, a $0.04 shared tip: the big one carries $0.03.
        let m = min_profits(&[(0.02, 300_000, 1.0), (0.01, 100_000, 1.0)], 0.04);
        assert_eq!(
            m,
            vec![((0.02 + 0.03) / 1.0) as u128, ((0.01 + 0.01) / 1.0) as u128]
        );
        // Token units: a $2,000 token needs far fewer units for the same USD.
        let m = min_profits(&[(0.02, 100, 2000.0)], 0.0);
        assert_eq!(m, vec![0]);
        let m = min_profits(&[(10.0, 100, 2000.0)], 0.0);
        assert_eq!(m, vec![0], "sub-unit amounts round down, never up");
        let m = min_profits(&[(4000.0, 100, 2000.0)], 0.0);
        assert_eq!(m, vec![2]);
    }

    #[test]
    fn a_missing_price_or_gas_gives_a_zero_floor_not_a_panic() {
        assert_eq!(min_profits(&[(1.0, 0, 0.0)], 5.0), vec![0]);
        assert!(min_profits(&[], 1.0).is_empty());
    }

    fn landed(success: bool, ok: Vec<bool>) -> Landed {
        Landed {
            success,
            gas_used: 1,
            effective_gas_price: 1,
            block: None,
            tx_index: None,
            batch_ok: ok,
        }
    }

    #[test]
    fn item_results_use_the_events_when_they_match_and_the_tx_status_otherwise() {
        assert_eq!(
            item_results(&landed(true, vec![true, false]), 2),
            vec![true, false]
        );
        // No events (a single deferred trade, or an event count that doesn't match): the tx status.
        assert_eq!(item_results(&landed(true, vec![]), 2), vec![true, true]);
        assert_eq!(item_results(&landed(false, vec![]), 1), vec![false]);
        assert_eq!(item_results(&landed(true, vec![true]), 3), vec![true; 3]);
    }
}
