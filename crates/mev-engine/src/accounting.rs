//! Admission and receipt accounting. Strategy estimates never credit a spend budget.

use super::{Engine, ROUTE_TIP_CAPACITY, TIP_BOOK_HORIZON_SECS, TxDone, now_secs};
use mev_exec::Landed;
use mev_risk::bid::TipBook;
use serde_json::json;
use std::time::Instant;
use tracing::{info, warn};

impl Engine {
    pub(crate) fn admit(
        &mut self,
        gross_usd: f64,
        gas_limit: u64,
        fees: mev_exec::Fees,
        _native_px: f64,
        arb: bool,
    ) -> Option<(mev_risk::ledger::Reservation, u64)> {
        let quote = self.native_price_integer()?;
        let bound = mev_risk::ledger::native_cost(gas_limit, fees.max_fee_per_gas, quote).ok()?;
        let tip =
            mev_risk::ledger::native_cost(gas_limit, fees.max_priority_fee_per_gas, quote).ok()?;
        let profit = mev_risk::limit_microusd(gross_usd)
            .ok()?
            .checked_sub(bound)?;
        match self
            .risk
            .reserve_integer(profit, bound, tip, arb, now_secs())
        {
            Ok(reservation) => {
                if reservation.set_native_price(quote).is_err() {
                    self.halt(
                        true,
                        "cannot persist native price for admitted transaction".into(),
                    );
                    return None;
                }
                Some((reservation, bound))
            }
            Err(block) => {
                self.on_cap(&block, arb);
                self.metrics
                    .event("risk_block", json!({"reason": format!("{block:?}")}));
                None
            }
        }
    }

    pub(crate) fn settle_receipt(
        &mut self,
        reservation: &mev_risk::ledger::Reservation,
        landed: &Landed,
        lost_race: bool,
    ) -> Option<f64> {
        let result = reservation
            .receipt_cost(landed.gas_used, landed.effective_gas_price)
            .and_then(|cost| {
                reservation.settle(now_secs(), landed.success, cost, lost_race)?;
                Ok(cost)
            });
        match result {
            Ok(cost) => Some(cost as f64 / 1_000_000.0),
            Err(block) => {
                self.halt(true, format!("receipt settlement failed: {block:?}"));
                None
            }
        }
    }

    pub(crate) fn realized_profit_usd(&self, landed: &Landed) -> Option<f64> {
        if !landed.success {
            return Some(0.0);
        }
        if landed.profits.is_empty() {
            return None;
        }
        landed.profits.iter().try_fold(0.0, |sum, (token, amount)| {
            self.price(*token).map(|px| sum + f64::from(*amount) * px)
        })
    }

    pub(crate) fn on_tx_done(&mut self, done: TxDone) {
        match done {
            TxDone::Landed {
                reservation,
                hash,
                kind,
                est_profit_usd,
                landed,
                route,
            } => {
                if let Some(r) = route {
                    self.release_pools(r);
                }
                if let (true, Some(r)) = (kind == "arb", route) {
                    self.gate.record(r, landed.success, now_secs());
                } else if kind == "arb" {
                    self.gate.record_global(
                        landed.success
                            && (landed.batch_ok.is_empty() || landed.batch_ok.iter().any(|ok| *ok)),
                        now_secs(),
                    );
                }
                if let (false, Some(r)) = (landed.success, route) {
                    // Lost race or mispriced: the next attempt on this route likely fails the same
                    // way. Spend gas on uncontested (long-tail) routes instead.
                    self.cooldown.insert(r, Instant::now());
                }
                let Some(gas_usd) = self.settle_receipt(
                    &reservation,
                    &landed,
                    !landed.success && kind != "liquidation",
                ) else {
                    return;
                };
                let actual_profit_usd = self.realized_profit_usd(&landed);
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
                        c.profit_usd += actual_profit_usd.unwrap_or(0.0);
                    } else {
                        c.landed_reverted += 1;
                    }
                });
                self.metrics.event(
                    "landed",
                    json!({ "hash": hash, "success": landed.success, "gas_used": landed.gas_used,
                            "gas_usd": gas_usd, "block": landed.block, "est_profit_usd": est_profit_usd, "actual_profit_usd": actual_profit_usd, "kind": kind }),
                );
                info!(%hash, success = landed.success, gas_usd = format!("{gas_usd:.4}"), "landed");
            }
            TxDone::BatchLanded {
                reservation,
                hash,
                landed,
                items,
            } => self.on_batch_landed(reservation, hash, landed, items),
            TxDone::BatchFailed {
                reservation,
                hash,
                items,
                error,
            } => self.on_batch_failed(reservation, hash, items, error),
            TxDone::TipSeen { tip, route } => {
                self.tips.record(tip, now_secs());
                if let Some(r) = route {
                    self.route_tips
                        .entry(r)
                        .or_insert_with(|| TipBook::new(ROUTE_TIP_CAPACITY, TIP_BOOK_HORIZON_SECS))
                        .record(tip, now_secs());
                }
            }
            TxDone::Failed {
                reservation,
                hash,
                error,
            } => {
                if reservation.cancel_unsigned().is_err() {
                    self.halt(
                        true,
                        "signed transaction unresolved; exposure retained".into(),
                    );
                }
                self.metrics
                    .event("send_failed", json!({ "hash": hash, "error": error }));
                warn!(?hash, %error, "send failed");
            }
        }
    }
}
