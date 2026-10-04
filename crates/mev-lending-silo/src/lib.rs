//! Silo v2 (Arbitrum) — **inactive: no meaningful debt to liquidate**.
//!
//! Verified 2026-10-03 from Silo's official deployments (`silo-contracts-v2/silo-core/deployments/
//! arbitrum_one`): SiloFactory `0xAFd8F792cb025A76C4916652CfC8e20eee3b6fe2` (ids 3001–3013),
//! SiloLens `0xBDD37B244E5e02FdE2e3F0510AB0bF3Fd621d089`, SiloHookV1 `0x0b6c649FfBA713E45958a2C394ed8b422ae6a456`.
//! All 13 markets combined held ~88,414 raw USDC of debt (≈ $0.09); nothing to liquidate.
//!
//! When markets grow, implementation plan (fits the trait as-is):
//! - Accounts: `Borrow` events on each market's two silos (`SiloConfig.getSilos()`).
//! - Health: `silo.isSolvent(borrower)` / SiloLens LTV; plan size from the hook's
//!   `maxLiquidation(borrower)` → (collateralToLiquidate, debtToRepay, sTokenRequired).
//! - Execution: `hook.liquidationCall(collateralAsset, debtAsset, borrower, maxDebtToCover, false)`;
//!   approve the **debt silo** (it pulls repayment), call the **hook** — the executor's separate
//!   approve-spender/target fields support this.

use alloy::primitives::Address;
use async_trait::async_trait;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolId};

pub const ID: ProtocolId = "silo-v2";

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
        AdapterStatus::Inactive(
            "Silo v2 Arbitrum markets hold ~$0.09 of debt in total (2026-10-03)",
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
