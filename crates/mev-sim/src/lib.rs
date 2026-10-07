//! Exact simulation of an opportunity by calling the deployed executor against live state.

pub mod pga;

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::SolCall,
};
use anyhow::Result;
use mev_bindings::ArbExecutor;
use mev_lending::Plan;
use mev_types::{Hop, Opportunity};

#[derive(Debug, Clone)]
pub enum SimOutcome {
    Profitable { profit: U256, gas: u64 },
    Reverted(String),
}

pub struct Simulator {
    provider: DynProvider,
    executor: Address,
    owner: Address,
    /// The deployed executor has `executeFlashSwap` (must match the sender's setting).
    pub flash_swaps: bool,
}

impl Simulator {
    pub fn new(provider: DynProvider, executor: Address, owner: Address) -> Self {
        Self {
            provider,
            executor,
            owner,
            flash_swaps: false,
        }
    }

    /// The executor call for `opp` as its owner (flash swap or Balancer, see `arb_calldata`).
    fn arb_request(&self, opp: &Opportunity, min_profit: U256) -> TransactionRequest {
        TransactionRequest::default()
            .from(self.owner)
            .to(self.executor)
            .input(mev_bindings::arb_calldata(opp, min_profit, self.flash_swaps).into())
    }

    /// The arbitrage call as an `eth_simulateV1` call object.
    pub fn arb_call_json(&self, opp: &Opportunity, min_profit: U256) -> serde_json::Value {
        serde_json::json!({
            "from": self.owner,
            "to": self.executor,
            "input": mev_bindings::arb_calldata(opp, min_profit, self.flash_swaps),
        })
    }

    /// `eth_call` the arbitrage; the executor returns its profit.
    async fn call_arb(&self, req: &TransactionRequest) -> std::result::Result<U256, String> {
        let raw = self
            .provider
            .call(req.clone())
            .await
            .map_err(|e| short(&e.to_string()))?;
        ArbExecutor::executeCall::abi_decode_returns(&raw).map_err(|e| short(&e.to_string()))
    }

    /// `eth_call` the executor as its owner; if it returns profit, estimate gas (on Arbitrum the
    /// estimate includes the L1 data cost).
    pub async fn simulate(&self, opp: &Opportunity, min_profit: U256) -> Result<SimOutcome> {
        let req = self.arb_request(opp, min_profit);
        match self.call_arb(&req).await {
            Ok(profit) => match self.provider.estimate_gas(req).await {
                Ok(gas) => Ok(SimOutcome::Profitable { profit, gas }),
                Err(e) => Ok(SimOutcome::Reverted(format!("estimate_gas: {e}"))),
            },
            Err(e) => Ok(SimOutcome::Reverted(e)),
        }
    }
}

/// Gas for an arbitrage with `hops` swaps (flash loan + swaps + repay). Calibrated on live
/// Arbitrum txs: 2 V3 hops used 300-333k gas. This prices the trade; the gas limit is set higher.
pub fn arb_gas_estimate(hops: usize) -> u64 {
    100_000 + 115_000 * hops as u64
}

/// The profit an `execute` return carries.
pub fn decode_arb_profit(raw: &[u8]) -> std::result::Result<U256, String> {
    ArbExecutor::executeCall::abi_decode_returns(raw).map_err(|e| short(&e.to_string()))
}

impl Simulator {
    /// Latency-critical path: a single eth_call, no estimate_gas round-trip.
    pub async fn simulate_fast(&self, opp: &Opportunity, min_profit: U256) -> Result<SimOutcome> {
        match self.call_arb(&self.arb_request(opp, min_profit)).await {
            Ok(profit) => Ok(SimOutcome::Profitable {
                profit,
                gas: arb_gas_estimate(opp.hops.len()),
            }),
            Err(e) => Ok(SimOutcome::Reverted(e)),
        }
    }
}

impl Simulator {
    /// `eth_call` the executor's generic `liquidate` as its owner, then estimate gas if it succeeds.
    pub async fn simulate_liquidation(
        &self,
        plan: &Plan,
        hops: &[Hop],
        min_profit: U256,
        aave_pool: Option<Address>,
    ) -> Result<SimOutcome> {
        let req = TransactionRequest::default()
            .from(self.owner)
            .to(self.executor)
            .input(mev_bindings::liquidation_calldata(plan, hops, min_profit, aave_pool).into());
        match self.call_arb(&req).await {
            Ok(profit) => match self.provider.estimate_gas(req).await {
                Ok(gas) => Ok(SimOutcome::Profitable { profit, gas }),
                Err(e) => Ok(SimOutcome::Reverted(format!("estimate_gas: {e}"))),
            },
            Err(e) => Ok(SimOutcome::Reverted(e)),
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arb_gas_estimate_covers_live_two_hop_trades() {
        // Live 2-hop V3 arbs used 299,763 and 302,351 gas.
        let g = arb_gas_estimate(2);
        assert!((300_000..=360_000).contains(&g));
        assert!(arb_gas_estimate(3) > g);
    }
}
