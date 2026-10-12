//! Durable transaction exposure. Amounts here are integer micro-dollars.
//! A signed transaction stays reserved until a receipt settles it; an RPC error is not settlement.

use crate::{Block, persist::Store};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: u64,
    pub bound_microusd: u64,
    pub arb: bool,
    pub hash: Option<String>,
    pub nonce: Option<u64>,
    #[serde(default)]
    pub native_price_microusd: u64,
    #[serde(default)]
    pub created_secs: u64,
    #[serde(default)]
    pub from: Option<String>,
    pub tip_bound_microusd: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct TipCaps {
    pub per_tx: u64,
    pub per_minute: u64,
    pub per_day: u64,
}
impl Default for TipCaps {
    fn default() -> Self {
        Self {
            per_tx: u64::MAX,
            per_minute: u64::MAX,
            per_day: u64::MAX,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    day: u64,
    spent_microusd: u64,
    arb_spent_microusd: u64,
    consecutive_failures: u32,
    next_id: u64,
    tip_spent_microusd: u64,
    tip_minute: Vec<(u64, u64)>,
    pending: BTreeMap<u64, Pending>,
    #[serde(default)]
    identity: Option<(u64, mev_types::Address)>,
    #[serde(default)]
    reconciled_at: Option<(u64, mev_types::B256)>,
    #[serde(default)]
    settled_nonce_floor: Option<u64>,
}

/// Verified by the operator tool against canonical RPC receipts before producing a candidate file.
#[derive(Debug, Clone)]
pub struct Seed {
    pub chain_id: u64,
    pub owner: mev_types::Address,
    pub block: u64,
    pub block_hash: mev_types::B256,
    pub now: u64,
    pub starting_nonce: u64,
    pub confirmed_nonce: u64,
    pub receipts: Vec<SeedReceipt>,
    pub pending: Vec<Pending>,
}

#[derive(Debug, Clone)]
pub struct SeedReceipt {
    pub nonce: u64,
    pub gas_microusd: u64,
    pub tip_microusd: u64,
    pub timestamp: u64,
    pub arb: bool,
    pub success: bool,
}

#[derive(Debug, Default)]
struct State {
    saved: Saved,
    store: Option<Store>,
    faulted: bool,
    recovering: std::collections::BTreeSet<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Ledger(Arc<Mutex<State>>);

#[derive(Debug, Clone)]
pub struct Reservation {
    pub id: u64,
    ledger: Ledger,
}

impl State {
    fn save(&mut self) -> Result<(), Block> {
        if self.faulted || self.store.as_ref().is_some_and(|s| !s.healthy()) {
            return Err(Block::Persistence);
        }
        if let Some(store) = &self.store
            && store.save_checked(&self.saved).is_err()
        {
            self.faulted = true;
            return Err(Block::Persistence);
        }
        Ok(())
    }

    fn roll(&mut self, now: u64) {
        if self.saved.day != now / 86_400 {
            self.saved.day = now / 86_400;
            self.saved.spent_microusd = 0;
            self.saved.arb_spent_microusd = 0;
            self.saved.consecutive_failures = 0;
            self.saved.tip_spent_microusd = 0;
            // Outstanding liabilities cross midnight and process restarts.
        }
        self.saved
            .tip_minute
            .retain(|(at, _)| now.saturating_sub(*at) < 60);
    }
}

impl Ledger {
    /// Creates a new candidate snapshot; never replaces an existing ledger or legacy snapshot.
    /// The caller must verify receipt completeness, identity and canonical block evidence.
    pub fn initialize(path: PathBuf, mut seed: Seed) -> anyhow::Result<()> {
        anyhow::ensure!(
            seed.chain_id != 0 && !seed.owner.is_zero(),
            "invalid ledger identity"
        );
        anyhow::ensure!(
            seed.receipts.len() <= 10_000 && seed.pending.len() <= 1024,
            "reconciliation too large"
        );
        let count = seed
            .confirmed_nonce
            .checked_sub(seed.starting_nonce)
            .ok_or_else(|| anyhow::anyhow!("nonce range regressed"))?;
        anyhow::ensure!(
            count == seed.receipts.len() as u64,
            "incomplete receipt nonce range"
        );
        seed.receipts.sort_by_key(|r| r.nonce);
        let mut saved = Saved {
            version: 2,
            day: seed.now / 86_400,
            identity: Some((seed.chain_id, seed.owner)),
            reconciled_at: Some((seed.block, seed.block_hash)),
            settled_nonce_floor: Some(seed.confirmed_nonce),
            ..Default::default()
        };
        for (i, r) in seed.receipts.iter().enumerate() {
            anyhow::ensure!(
                r.nonce == seed.starting_nonce + i as u64,
                "duplicate or missing receipt nonce"
            );
            anyhow::ensure!(
                r.timestamp / 86_400 == saved.day && r.timestamp <= seed.now,
                "receipt outside accounting day"
            );
            anyhow::ensure!(r.tip_microusd <= r.gas_microusd, "tip exceeds total fee");
            saved.spent_microusd = saved
                .spent_microusd
                .checked_add(r.gas_microusd)
                .ok_or_else(|| anyhow::anyhow!("gas accounting overflow"))?;
            if r.arb {
                saved.arb_spent_microusd = saved
                    .arb_spent_microusd
                    .checked_add(r.gas_microusd)
                    .ok_or_else(|| anyhow::anyhow!("arb accounting overflow"))?;
            }
            saved.tip_spent_microusd = saved
                .tip_spent_microusd
                .checked_add(r.tip_microusd)
                .ok_or_else(|| anyhow::anyhow!("tip accounting overflow"))?;
            if seed.now.saturating_sub(r.timestamp) < 60 && r.tip_microusd > 0 {
                saved.tip_minute.push((r.timestamp, r.tip_microusd));
            }
            saved.consecutive_failures = if r.success {
                0
            } else {
                saved.consecutive_failures.saturating_add(1)
            };
        }
        anyhow::ensure!(
            saved.tip_minute.len() <= 4096,
            "minute accounting too large"
        );
        seed.pending.sort_by_key(|p| p.nonce);
        let mut hashes = std::collections::BTreeSet::new();
        for (i, mut p) in seed.pending.into_iter().enumerate() {
            anyhow::ensure!(
                p.nonce == seed.confirmed_nonce.checked_add(i as u64),
                "incomplete pending nonce range"
            );
            anyhow::ensure!(
                p.from
                    .as_deref()
                    .and_then(|a| a.parse::<mev_types::Address>().ok())
                    == Some(seed.owner),
                "pending owner mismatch"
            );
            let hash = p
                .hash
                .as_deref()
                .and_then(|h| h.parse::<mev_types::B256>().ok())
                .ok_or_else(|| anyhow::anyhow!("invalid pending hash"))?;
            anyhow::ensure!(hashes.insert(hash), "duplicate pending hash");
            anyhow::ensure!(
                p.native_price_microusd > 0
                    && p.bound_microusd > 0
                    && p.tip_bound_microusd <= p.bound_microusd,
                "invalid pending liability"
            );
            p.id = i as u64;
            saved.pending.insert(p.id, p);
        }
        saved.next_id = saved.pending.len() as u64;
        let store = Store::new(path);
        anyhow::ensure!(
            store.load_checked::<serde_json::Value>()?.is_none(),
            "output already exists; refusing to replace accounting"
        );
        store.save_checked(&saved)
    }

    pub fn require_identity(&self, chain_id: u64, owner: mev_types::Address) -> Result<(), Block> {
        let mut s = self.0.lock().unwrap();
        if s.saved.identity != Some((chain_id, owner))
            || s.saved.reconciled_at.is_none()
            || s.saved.settled_nonce_floor.is_none()
        {
            s.faulted = true;
            return Err(Block::Persistence);
        }
        Ok(())
    }

    pub fn nonce_window(&self) -> Result<(u64, u64), Block> {
        let s = self.0.lock().unwrap();
        if s.faulted || s.store.as_ref().is_some_and(|store| !store.healthy()) {
            return Err(Block::Persistence);
        }
        let floor = s.saved.settled_nonce_floor.ok_or(Block::Persistence)?;
        let upper = s
            .saved
            .pending
            .values()
            .filter_map(|p| p.nonce)
            .try_fold(floor, |upper, n| n.checked_add(1).map(|n| upper.max(n)))
            .ok_or(Block::InvalidInput)?;
        Ok((floor, upper))
    }
    pub fn persistent(path: PathBuf, now: u64) -> Self {
        Self::open(path, now, false)
    }

    pub fn persistent_existing(path: PathBuf, now: u64) -> Self {
        Self::open(path, now, true)
    }

    fn open(path: PathBuf, now: u64, require_existing: bool) -> Self {
        let store = Store::new(path);
        let mut state = State {
            store: Some(store.clone()),
            ..Default::default()
        };
        match store.load_checked::<Saved>() {
            Ok(Some(saved)) if saved.version == 2 => state.saved = saved,
            Ok(None) if !require_existing => {
                state.saved.version = 2;
                state.saved.day = now / 86_400;
            }
            // Legacy snapshots require reconciliation; silently treating them as empty loses money.
            _ => state.faulted = true,
        }
        state.roll(now);
        if !state.faulted {
            // No broadcast can precede durable attach. Unsigned reservations are safe to cancel.
            state.saved.pending.retain(|_, p| p.hash.is_some());
            state.faulted = state.saved.pending.iter().any(|(id, p)| {
                p.id != *id
                    || p.tip_bound_microusd > p.bound_microusd
                    || p.native_price_microusd == 0
                    || p.nonce.is_none()
                    || p.from
                        .as_deref()
                        .is_none_or(|a| a.parse::<mev_types::Address>().is_err())
                    || p.hash
                        .as_deref()
                        .is_none_or(|h| h.parse::<mev_types::B256>().is_err())
            });
            state.recovering = state.saved.pending.keys().copied().collect();
            let _ = state.save();
        }
        Self(Arc::new(Mutex::new(state)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &self,
        now: u64,
        gas: u64,
        arb: bool,
        total_cap: u64,
        arb_cap: u64,
        max_pending: u32,
        max_failures: u32,
    ) -> Result<(), Block> {
        let mut s = self.0.lock().unwrap();
        s.roll(now);
        if s.faulted || s.store.as_ref().is_some_and(|s| !s.healthy()) {
            return Err(Block::Persistence);
        }
        if !s.recovering.is_empty() {
            return Err(Block::Unresolved);
        }
        if s.saved.consecutive_failures >= max_failures {
            return Err(Block::Paused {
                consecutive_failures: s.saved.consecutive_failures,
            });
        }
        if s.saved.pending.len() >= max_pending as usize {
            return Err(Block::InFlight);
        }
        let exposure = s
            .saved
            .pending
            .values()
            .fold(s.saved.spent_microusd, |n, p| {
                n.saturating_add(p.bound_microusd)
            });
        let arb_exposure = s
            .saved
            .pending
            .values()
            .filter(|p| p.arb)
            .fold(s.saved.arb_spent_microusd, |n, p| {
                n.saturating_add(p.bound_microusd)
            });
        if exposure.saturating_add(gas) > total_cap
            || (arb && arb_exposure.saturating_add(gas) > arb_cap)
        {
            return Err(Block::DailyGasCap {
                spent_usd: exposure as f64 / 1_000_000.0,
            });
        }
        Ok(())
    }

    // Admission is owned by the single engine; check + reserve happen without an await.
    pub(crate) fn reserve(&self, now: u64, gas: u64, arb: bool) -> Result<Reservation, Block> {
        self.reserve_with_tip(now, gas, 0, arb)
    }

    pub fn check_tip(&self, now: u64, tip: u64, caps: TipCaps) -> Result<(), Block> {
        let mut s = self.0.lock().unwrap();
        s.roll(now);
        if s.faulted || s.store.as_ref().is_some_and(|store| !store.healthy()) {
            return Err(Block::Persistence);
        }
        let pending = s
            .saved
            .pending
            .values()
            .fold(0u64, |n, p| n.saturating_add(p.tip_bound_microusd));
        let minute = s
            .saved
            .tip_minute
            .iter()
            .fold(pending, |n, (_, t)| n.saturating_add(*t));
        for (window, exposure, cap) in [
            ("transaction", tip, caps.per_tx),
            ("minute", minute.saturating_add(tip), caps.per_minute),
            (
                "day",
                s.saved
                    .tip_spent_microusd
                    .saturating_add(pending)
                    .saturating_add(tip),
                caps.per_day,
            ),
        ] {
            if exposure > cap {
                return Err(Block::TipCap {
                    window,
                    exposure_microusd: exposure,
                    limit_microusd: cap,
                });
            }
        }
        if s.saved.tip_minute.len() >= 4096 {
            return Err(Block::InFlight);
        }
        Ok(())
    }

    pub(crate) fn reserve_with_tip(
        &self,
        now: u64,
        gas: u64,
        tip: u64,
        arb: bool,
    ) -> Result<Reservation, Block> {
        if tip > gas {
            return Err(Block::InvalidInput);
        }
        let mut s = self.0.lock().unwrap();
        if s.faulted || s.store.as_ref().is_some_and(|st| !st.healthy()) {
            return Err(Block::Persistence);
        }
        s.roll(now);
        let id = s.saved.next_id;
        s.saved.next_id = id.checked_add(1).ok_or(Block::InvalidInput)?;
        s.saved.version = 2;
        s.saved.pending.insert(
            id,
            Pending {
                id,
                bound_microusd: gas,
                arb,
                hash: None,
                nonce: None,
                native_price_microusd: 0,
                created_secs: now,
                from: None,
                tip_bound_microusd: tip,
            },
        );
        // No disk write here: an unsigned reservation is not a liability anyone can be hurt by losing in a
        // crash (nothing has been signed). The one fsync that matters — before any endpoint sees signed
        // bytes — is `attach`, which persists this entry together with its hash and nonce.
        Ok(Reservation {
            id,
            ledger: self.clone(),
        })
    }

    pub fn unresolved(&self) -> Vec<(Reservation, Pending)> {
        self.0
            .lock()
            .unwrap()
            .saved
            .pending
            .values()
            .cloned()
            .map(|p| {
                (
                    Reservation {
                        id: p.id,
                        ledger: self.clone(),
                    },
                    p,
                )
            })
            .collect()
    }

    pub fn spent(&self) -> u64 {
        self.0.lock().unwrap().saved.spent_microusd
    }

    pub fn arb_spent(&self) -> u64 {
        self.0.lock().unwrap().saved.arb_spent_microusd
    }

    #[cfg(test)]
    pub fn record_legacy(&self, now: u64, success: bool, gas: u64, arb: bool, lost_race: bool) {
        let mut s = self.0.lock().unwrap();
        s.roll(now);
        s.saved.spent_microusd = s.saved.spent_microusd.saturating_add(gas);
        if arb {
            s.saved.arb_spent_microusd = s.saved.arb_spent_microusd.saturating_add(gas);
        }
        if success {
            s.saved.consecutive_failures = 0;
        } else if !lost_race {
            s.saved.consecutive_failures = s.saved.consecutive_failures.saturating_add(1);
        }
        let _ = s.save();
    }

    #[cfg(test)]
    pub fn add_arb_cost(&self, now: u64, gas: u64) {
        let mut s = self.0.lock().unwrap();
        s.roll(now);
        s.saved.arb_spent_microusd = s.saved.arb_spent_microusd.saturating_add(gas);
        let _ = s.save();
    }
}

impl Reservation {
    pub fn active(&self) -> bool {
        let s = self.ledger.0.lock().unwrap();
        !s.faulted
            && s.saved.pending.contains_key(&self.id)
            && s.store.as_ref().is_none_or(Store::healthy)
    }

    pub fn verify_terms(
        &self,
        gas_limit: u64,
        max_fee_wei: u128,
        priority_fee_wei: u128,
    ) -> Result<(), Block> {
        let s = self.ledger.0.lock().unwrap();
        let p = s.saved.pending.get(&self.id).ok_or(Block::Unresolved)?;
        if gas_limit == 0
            || priority_fee_wei > max_fee_wei
            || native_cost(gas_limit, max_fee_wei, p.native_price_microusd)? > p.bound_microusd
            || native_cost(gas_limit, priority_fee_wei, p.native_price_microusd)?
                > p.tip_bound_microusd
        {
            return Err(Block::InvalidInput);
        }
        Ok(())
    }
    pub fn set_native_price(&self, price_microusd: u64) -> Result<(), Block> {
        if price_microusd == 0 {
            return Err(Block::InvalidInput);
        }
        let mut s = self.ledger.0.lock().unwrap();
        let p = s.saved.pending.get_mut(&self.id).ok_or(Block::Unresolved)?;
        if p.hash.is_some() {
            return Err(Block::Unresolved);
        }
        p.native_price_microusd = price_microusd;
        Ok(()) // persisted by `attach`, before anything is signed or sent
    }

    pub fn receipt_cost(&self, gas_used: u64, effective_price: u128) -> Result<u64, Block> {
        let s = self.ledger.0.lock().unwrap();
        let p = s.saved.pending.get(&self.id).ok_or(Block::Unresolved)?;
        native_cost(gas_used, effective_price, p.native_price_microusd)
    }

    /// Must complete durably before any endpoint receives signed bytes.
    pub fn attach(&self, hash: String, nonce: u64, from: mev_types::Address) -> anyhow::Result<()> {
        anyhow::ensure!(
            hash.parse::<mev_types::B256>().is_ok(),
            "invalid transaction hash"
        );
        let mut s = self.ledger.0.lock().unwrap();
        let p = s
            .saved
            .pending
            .get_mut(&self.id)
            .ok_or_else(|| anyhow::anyhow!("reservation missing"))?;
        anyhow::ensure!(
            p.native_price_microusd > 0
                && p.hash.as_ref().is_none_or(|h| h == &hash)
                && p.nonce.is_none_or(|n| n == nonce)
                && p.from.as_deref().is_none_or(|a| a == from.to_string()),
            "reservation already signed with another hash"
        );
        p.hash = Some(hash);
        p.nonce = Some(nonce);
        p.from = Some(from.to_string());
        s.save()
            .map_err(|b| anyhow::anyhow!("cannot persist transaction: {b:?}"))
    }

    pub fn cancel_unsigned(&self) -> Result<(), Block> {
        let mut s = self.ledger.0.lock().unwrap();
        if s.saved
            .pending
            .get(&self.id)
            .is_some_and(|p| p.hash.is_some())
        {
            return Err(Block::Unresolved);
        }
        s.saved.pending.remove(&self.id);
        s.save()
    }

    /// The broadcaster may cancel signed bytes only before invoking any transport.
    pub fn cancel_before_broadcast(&self, hash: &str) -> Result<(), Block> {
        let mut s = self.ledger.0.lock().unwrap();
        let p = s.saved.pending.get(&self.id).ok_or(Block::Unresolved)?;
        if p.hash.as_deref() != Some(hash) {
            return Err(Block::Unresolved);
        }
        s.saved.pending.remove(&self.id);
        s.save()
    }

    pub fn settle(&self, now: u64, success: bool, gas: u64, lost_race: bool) -> Result<(), Block> {
        let mut s = self.ledger.0.lock().unwrap();
        s.roll(now);
        let p = s.saved.pending.remove(&self.id).ok_or(Block::Unresolved)?;
        if let Some(nonce) = p.nonce {
            let floor = nonce.checked_add(1).ok_or(Block::InvalidInput)?;
            s.saved.settled_nonce_floor = Some(s.saved.settled_nonce_floor.unwrap_or(0).max(floor));
        }
        s.recovering.remove(&self.id);
        s.saved.spent_microusd = s.saved.spent_microusd.saturating_add(gas);
        // Receipt total cost and the priority-fee bound are both upper bounds on the paid tip.
        // Until an exact fee breakdown is verified, release exposure only to this conservative charge.
        let tip = gas.min(p.tip_bound_microusd);
        s.saved.tip_spent_microusd = s.saved.tip_spent_microusd.saturating_add(tip);
        if tip > 0 {
            s.saved.tip_minute.push((now, tip));
        }
        if p.arb {
            s.saved.arb_spent_microusd = s.saved.arb_spent_microusd.saturating_add(gas);
        }
        if success {
            s.saved.consecutive_failures = 0;
        } else if !lost_race {
            s.saved.consecutive_failures = s.saved.consecutive_failures.saturating_add(1);
        }
        s.save()
    }
}

pub fn native_cost(gas: u64, fee_wei: u128, native_price_microusd: u64) -> Result<u64, Block> {
    use mev_types::U256;
    if native_price_microusd == 0 {
        return Err(Block::InvalidInput);
    }
    let numerator = U256::from(gas)
        .checked_mul(U256::from(fee_wei))
        .and_then(|v| v.checked_mul(U256::from(native_price_microusd)))
        .ok_or(Block::InvalidInput)?;
    let divisor = U256::from(1_000_000_000_000_000_000u128);
    let (whole, remainder) = numerator.div_rem(divisor);
    let rounded = whole
        .checked_add(U256::from(u8::from(!remainder.is_zero())))
        .ok_or(Block::InvalidInput)?;
    u64::try_from(rounded).map_err(|_| Block::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_requires_complete_receipts_and_refuses_overwrite() {
        let path = std::env::temp_dir().join(format!("seed-ledger-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let seed = Seed {
            chain_id: 42161,
            owner: mev_types::Address::with_last_byte(1),
            block: 12,
            block_hash: mev_types::B256::repeat_byte(12),
            now: 100,
            starting_nonce: 3,
            confirmed_nonce: 5,
            receipts: vec![
                SeedReceipt {
                    nonce: 3,
                    gas_microusd: 400_000,
                    tip_microusd: 100_000,
                    timestamp: 98,
                    arb: true,
                    success: false,
                },
                SeedReceipt {
                    nonce: 4,
                    gas_microusd: 300_000,
                    tip_microusd: 100_000,
                    timestamp: 99,
                    arb: false,
                    success: false,
                },
            ],
            pending: vec![],
        };
        let mut incomplete = seed.clone();
        incomplete.receipts.pop();
        assert!(Ledger::initialize(path.clone(), incomplete).is_err());
        assert!(!path.exists());
        let mut duplicate = seed.clone();
        duplicate.receipts[1].nonce = 3;
        assert!(Ledger::initialize(path.clone(), duplicate).is_err());
        Ledger::initialize(path.clone(), seed.clone()).unwrap();
        assert!(Ledger::initialize(path.clone(), seed).is_err());
        let l = Ledger::persistent_existing(path.clone(), 100);
        l.require_identity(42161, mev_types::Address::with_last_byte(1))
            .unwrap();
        assert_eq!(l.spent(), 700_000);
        assert_eq!(l.arb_spent(), 400_000);
        assert_eq!(
            l.check(100, 400_000, false, 1_000_000, 1_000_000, 3, 10),
            Err(Block::DailyGasCap { spent_usd: 0.7 })
        );
        assert_eq!(
            l.require_identity(1, mev_types::Address::with_last_byte(1)),
            Err(Block::Persistence)
        );
        assert_eq!(
            l.check(100, 1, false, 1_000_000, 1_000_000, 3, 10),
            Err(Block::Persistence)
        );
        drop(l);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn live_open_cannot_create_an_empty_budget() {
        let path =
            std::env::temp_dir().join(format!("required-ledger-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = Ledger::persistent_existing(path.clone(), 100);
        assert_eq!(
            l.check(100, 1, false, 1_000_000, 1_000_000, 3, 10),
            Err(Block::Persistence)
        );
        assert!(!path.exists());
    }

    #[test]
    fn admission_touches_no_disk_and_the_single_durable_write_is_attach() {
        let path = std::env::temp_dir().join(format!("one-fsync-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = Ledger::persistent(path.clone(), 100);
        let before = std::fs::read(&path).ok();
        let r = l.reserve_with_tip(100, 100_000, 20_000, true).unwrap();
        r.set_native_price(1_000_000).unwrap();
        assert_eq!(
            std::fs::read(&path).ok(),
            before,
            "reserve + price write nothing"
        );
        r.attach(
            mev_types::B256::repeat_byte(7).to_string(),
            1,
            mev_types::Address::with_last_byte(1),
        )
        .unwrap();
        let after = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(
            after.contains(&mev_types::B256::repeat_byte(7).to_string()),
            "attach persisted the liability"
        );
        // A restart sees the signed transaction as an unresolved liability.
        drop(r);
        drop(l);
        let l = Ledger::persistent_existing(path.clone(), 101);
        assert_eq!(l.unresolved().len(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn signed_priority_fee_cannot_exceed_the_reserved_tip_bound() {
        let l = Ledger::default();
        let r = l.reserve_with_tip(100, 100_000, 20_000, false).unwrap();
        r.set_native_price(1_000_000).unwrap();
        // 100_000 gas at 100 gwei priority = 0.01 native = 10_000 micro-USD: inside the 20_000 bound.
        assert!(
            r.verify_terms(100_000, 400_000_000_000, 100_000_000_000)
                .is_ok()
        );
        // 300 gwei priority = 30_000 micro-USD: over the tip bound although inside the gas bound.
        assert_eq!(
            r.verify_terms(100_000, 400_000_000_000, 300_000_000_000),
            Err(Block::InvalidInput)
        );
    }

    #[test]
    fn signed_tip_liabilities_and_charges_survive_restart() {
        let path = std::env::temp_dir().join(format!("tip-ledger-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = Ledger::persistent(path.clone(), 100);
        let r = l.reserve_with_tip(100, 100_000, 20_000, false).unwrap();
        r.set_native_price(1_000_000).unwrap();
        r.attach(
            mev_types::B256::repeat_byte(2).to_string(),
            3,
            mev_types::Address::with_last_byte(1),
        )
        .unwrap();
        drop(r);
        drop(l);
        let l = Ledger::persistent_existing(path.clone(), 101);
        let caps = TipCaps {
            per_tx: 30_000,
            per_minute: 30_000,
            per_day: 30_000,
        };
        assert!(l.check_tip(101, 20_000, caps).is_err());
        let (r, _) = l.unresolved().remove(0);
        r.settle(102, true, 40_000, false).unwrap();
        drop(r);
        drop(l);
        let l = Ledger::persistent_existing(path.clone(), 103);
        assert!(l.check_tip(103, 20_000, caps).is_err());
        assert!(
            l.check_tip(163, 20_000, caps).is_err(),
            "daily charge still applies after minute expiry"
        );
        assert!(l.check_tip(86_401, 20_000, caps).is_ok());
        drop(l);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pending_tips_cross_midnight_and_settle_to_a_conservative_charge() {
        let ledger = Ledger::default();
        let caps = TipCaps {
            per_tx: 20_000,
            per_minute: 30_000,
            per_day: 50_000,
        };
        let r = ledger
            .reserve_with_tip(86_399, 100_000, 20_000, true)
            .unwrap();
        assert!(matches!(
            ledger.check_tip(86_401, 20_000, caps),
            Err(Block::TipCap {
                window: "minute",
                ..
            })
        ));
        r.settle(86_402, false, 10_000, true).unwrap();
        assert!(ledger.check_tip(86_403, 20_000, caps).is_ok());
        assert!(matches!(
            ledger.check_tip(86_403, 20_001, caps),
            Err(Block::TipCap {
                window: "transaction",
                ..
            })
        ));
        assert_eq!(ledger.0.lock().unwrap().saved.tip_spent_microusd, 10_000);
        assert!(ledger.check_tip(86_465, 20_000, caps).is_ok());
    }

    #[test]
    fn pending_liability_survives_restart_and_corruption_blocks_sends() {
        let path = std::env::temp_dir().join(format!("ledger-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = Ledger::persistent(path.clone(), 100);
        let r = l.reserve(100, 700_000, true).unwrap();
        r.set_native_price(1_000_000).unwrap();
        assert!(r.verify_terms(1, 1_000_000_000_000_000_000, 0).is_err());
        r.attach(
            mev_types::B256::repeat_byte(1).to_string(),
            7,
            mev_types::Address::with_last_byte(1),
        )
        .unwrap();
        assert_eq!(r.set_native_price(1), Err(Block::Unresolved));
        let competing = Ledger::persistent(path.clone(), 101);
        assert_eq!(
            competing.check(101, 1, false, 1_000_000, 1_000_000, 3, 10),
            Err(Block::Persistence)
        );
        drop(competing);
        drop(r);
        drop(l);
        let resumed = Ledger::persistent(path.clone(), 101);
        assert_eq!(resumed.unresolved().len(), 1);
        assert!(
            resumed
                .check(101, 400_000, true, 1_000_000, 1_000_000, 3, 10)
                .is_err()
        );
        let (r, _) = resumed.unresolved().remove(0);
        r.settle(101, false, 200_000, true).unwrap();
        assert_eq!(resumed.spent(), 200_000);
        drop(r);
        drop(resumed);
        std::fs::write(&path, b"invalid").unwrap();
        let corrupt = Ledger::persistent(path.clone(), 102);
        assert_eq!(
            corrupt.check(102, 1, false, 1_000_000, 1_000_000, 3, 10),
            Err(Block::Persistence)
        );
        let _ = std::fs::remove_file(path);
    }
}
