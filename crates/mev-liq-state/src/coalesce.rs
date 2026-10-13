//! Trigger coalescing (spec section 4): identical borrower keys collapse into one entry that keeps
//! the most urgent reason and the latest chain position. The structure is bounded, but overload
//! never drops a trigger: keys beyond the cap are counted and the batch escalates to "mark every
//! active borrower" plus a `backlog_escalated` pool flag.

use std::collections::BTreeMap;

use alloy::primitives::Address;

use crate::domain::{ChainPoint, DirtyReason};
use crate::index::{Effect, FLAG_BACKLOG_ESCALATED};

#[derive(Debug, Clone, Copy)]
struct Slot {
    reason: DirtyReason,
    at: ChainPoint,
    /// Position of the first registering event (`Borrow`) in the coalesced set.
    register_at: Option<ChainPoint>,
    stale_deps: bool,
}

#[derive(Debug)]
pub struct Coalescer {
    cap: usize,
    accounts: BTreeMap<Address, Slot>,
    assets: BTreeMap<(Address, DirtyReason), Option<ChainPoint>>,
    overflow: u64,
    overflow_reason: Option<DirtyReason>,
    overflow_at: Option<ChainPoint>,
}

fn later(a: Option<ChainPoint>, b: Option<ChainPoint>) -> Option<ChainPoint> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.block_number >= x.block_number {
            y
        } else {
            x
        }),
        (x, None) => x,
        (None, y) => y,
    }
}

impl Coalescer {
    /// `cap` bounds the number of distinct borrower keys held (asset targets are bounded by the
    /// reserve count and not capped).
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            accounts: BTreeMap::new(),
            assets: BTreeMap::new(),
            overflow: 0,
            overflow_reason: None,
            overflow_at: None,
        }
    }

    pub fn push_account(
        &mut self,
        borrower: Address,
        reason: DirtyReason,
        at: ChainPoint,
        register: bool,
        stale_deps: bool,
    ) {
        if let Some(s) = self.accounts.get_mut(&borrower) {
            if reason.urgency() > s.reason.urgency() {
                s.reason = reason;
            }
            if at.block_number >= s.at.block_number {
                s.at = at;
            }
            if register && s.register_at.is_none() {
                s.register_at = Some(at);
            }
            s.stale_deps |= stale_deps;
        } else if self.accounts.len() < self.cap {
            self.accounts.insert(
                borrower,
                Slot {
                    reason,
                    at,
                    register_at: register.then_some(at),
                    stale_deps,
                },
            );
        } else {
            self.overflow += 1;
            if self
                .overflow_reason
                .is_none_or(|r| reason.urgency() > r.urgency())
            {
                self.overflow_reason = Some(reason);
            }
            self.overflow_at = later(self.overflow_at, Some(at));
        }
    }

    pub fn push_asset(&mut self, asset: Address, reason: DirtyReason, at: Option<ChainPoint>) {
        let e = self.assets.entry((asset, reason)).or_insert(at);
        *e = later(*e, at);
    }

    pub fn overflowed(&self) -> u64 {
        self.overflow
    }

    pub fn len(&self) -> usize {
        self.accounts.len() + self.assets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.assets.is_empty() && self.overflow == 0
    }

    /// Effects in deterministic order, most urgent first. Overflow adds an `AllDirty` with the
    /// most urgent overflowed reason and the backlog flag; nothing is lost.
    pub fn into_effects(self) -> Vec<Effect> {
        let mut out = vec![];
        let mut accts: Vec<_> = self.accounts.into_iter().collect();
        accts.sort_by(|a, b| {
            b.1.reason
                .urgency()
                .cmp(&a.1.reason.urgency())
                .then(a.0.cmp(&b.0))
        });
        for (borrower, s) in accts {
            out.push(Effect::Touch {
                borrower,
                reason: s.reason,
                at: s.at,
                register: s.register_at,
                stale_deps: s.stale_deps,
            });
        }
        let mut assets: Vec<_> = self.assets.into_iter().collect();
        assets.sort_by(|a, b| {
            b.0.1
                .urgency()
                .cmp(&a.0.1.urgency())
                .then(a.0.0.cmp(&b.0.0))
        });
        for ((asset, reason), at) in assets {
            out.push(Effect::AssetDirty { asset, reason, at });
        }
        if let Some(reason) = self.overflow_reason {
            out.push(Effect::AllDirty {
                reason,
                at: self.overflow_at,
            });
            out.push(Effect::Flag {
                flag: FLAG_BACKLOG_ESCALATED.to_string(),
                at: self.overflow_at,
                detail: format!("{} trigger keys beyond the coalescing cap", self.overflow),
            });
        }
        out
    }
}
