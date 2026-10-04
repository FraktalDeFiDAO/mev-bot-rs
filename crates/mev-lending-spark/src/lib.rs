//! Spark (SparkLend) — **not deployed on Arbitrum**.
//!
//! SparkLend (an Aave v3 fork) runs on Ethereum and Gnosis. On Arbitrum, Spark operates Spark
//! Savings / the Spark Liquidity Layer (deposits since 2025-02-03), which has no borrowers and
//! nothing to liquidate. If SparkLend ever deploys here, the Aave v3 adapter applies almost as-is
//! (same `liquidationCall`, data provider, and oracle shape) with Spark addresses.

use alloy::primitives::Address;
use async_trait::async_trait;
use mev_lending::{AccountRef, AdapterStatus, Health, LiquidationAdapter, Plan, ProtocolId};

pub const ID: ProtocolId = "spark";

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
            "no SparkLend market on Arbitrum; Spark is present only as Savings / Liquidity Layer",
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
