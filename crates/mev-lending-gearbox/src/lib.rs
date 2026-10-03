//! Gearbox v3 (Arbitrum) — **audit pending**.
//!
//! Verified facts: anyone can liquidate an unhealthy Credit Account through
//! `CreditFacadeV3.liquidateCreditAccount(creditAccount, to, calls)`. The liquidator supplies
//! multicalls (through Gearbox adapters) that convert collateral into the underlying to repay the
//! debt; leftovers go to `to`. Health is threshold-weighted collateral vs debt (USD).
//!
//! Integration checklist before `Live`:
//! 1. Enumerate Credit Managers (from the Address Provider / market configurators) and their
//!    credit accounts (`creditAccounts()` per manager).
//! 2. Health: `CreditManagerV3.calcDebtAndCollateral(account, mode)` / `isLiquidatable`.
//! 3. Execution shape differs from flash-loan + external swap: the swap happens inside the
//!    facade multicall through Gearbox adapters. The executor would call the facade with the
//!    multicall and receive the remainder; needs a dedicated plan builder and fork tests.
//! 4. Measure 30-day liquidation flow and competitors first.

use alloy::primitives::Address;
use async_trait::async_trait;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolId};

pub const ID: ProtocolId = "gearbox";

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
            "credit-account liquidation via CreditFacadeV3.liquidateCreditAccount with liquidator-supplied multicalls",
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
