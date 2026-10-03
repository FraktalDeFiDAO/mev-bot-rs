//! Radiant Capital (Arbitrum) — **inactive**.
//!
//! Radiant paused its Arbitrum lending markets after a $4.5M flash-loan exploit (Jan 2024) and a
//! >$50M breach on BNB Chain and Arbitrum (Oct 2024). No live markets to liquidate as of
//! 2026-10-03. Radiant v2 is an Aave v2 fork: if markets reopen, reuse the Aave adapter shape
//! (`liquidationCall`) after verifying the LendingPool address, oracle, and that `liquidationCall`
//! is not paused.

use alloy::primitives::Address;
use async_trait::async_trait;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolId};

pub const ID: ProtocolId = "radiant";

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
            "Arbitrum markets halted after the Jan 2024 ($4.5M) and Oct 2024 (>$50M) exploits",
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
