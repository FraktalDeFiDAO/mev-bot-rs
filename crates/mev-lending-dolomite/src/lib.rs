//! Dolomite (Arbitrum) — **audit pending**.
//!
//! Verified facts:
//! - DolomiteMargin: `0x6Bd780E7fDf01D77e4d475c821f1e7AE05409072` (Arbitrum).
//! - LiquidatorProxyV6: `0x8564F36E53AC4f16A2C0B8449cf98d51C3438efD`; `dolomiteRake()` / 1e18 is the
//!   live rake (10% of the liquidation penalty at launch, Mar 2026, non-isolation-mode only).
//!
//! Integration checklist before `Live`:
//! 1. Accounts are `(owner, accountNumber)` pairs — index from `LogDeposit`/`LogWithdraw`/`LogBorrow`
//!    style events; map to `AccountRef { account: owner, sub_id: number }`.
//! 2. Health: `getAccountValues(account)` (supply vs borrow value with margin ratio).
//! 3. Execution: liquidate via `LiquidatorProxyV6` (it handles the swap); the executor call needs
//!    the proxy allowlisted, and the liquidator account must hold collateral in Dolomite — decide
//!    whether to keep a funded Dolomite account or route a flash loan through it.
//! 4. Measure 30-day liquidation flow and competitors (like Aave/Morpho in DECISIONS) first.

use alloy::primitives::Address;
use async_trait::async_trait;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolId};

pub const ID: ProtocolId = "dolomite";

/// Placeholder adapter: reports its status so the runner logs and skips it. It never produces
/// accounts or plans until the integration is verified and implemented.
#[derive(Debug, Default)]
pub struct Adapter;

impl Adapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LiquidationAdapter for Adapter {
    fn id(&self) -> ProtocolId {
        ID
    }

    fn status(&self) -> AdapterStatus {
        AdapterStatus::AuditPending(
            "account model (owner, number) via DolomiteMargin.operate / LiquidatorProxyV6; 10% rake on the liquidation penalty since Mar 2026",
        )
    }

    async fn index(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }

    fn accounts(&self) -> Vec<AccountRef> {
        vec![]
    }

    async fn health(&self, _accounts: &[AccountRef], _fast: bool) -> anyhow::Result<Vec<Health>> {
        Ok(vec![])
    }

    async fn plan(&self, _health: &Health, _executor: Address) -> anyhow::Result<Option<Plan>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_not_live_and_never_plans() {
        let mut a = Adapter::new();
        assert_ne!(a.status(), AdapterStatus::Live);
        assert!(a.index().await.unwrap());
        assert!(a.accounts().is_empty());
        assert!(a.targets().is_empty());
    }
}
