//! Exact simulation of an opportunity by calling the deployed executor against live state.

use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
};
use anyhow::Result;
use mev_bindings::{ArbExecutor, to_sol_call, to_sol_hop};
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
}

impl Simulator {
    pub fn new(provider: DynProvider, executor: Address, owner: Address) -> Self {
        Self {
            provider,
            executor,
            owner,
        }
    }

    /// `eth_call` the executor as its owner; if it returns profit, estimate gas (on Arbitrum the
    /// estimate includes the L1 data cost).
    pub async fn simulate(&self, opp: &Opportunity, min_profit: U256) -> Result<SimOutcome> {
        let exec = ArbExecutor::new(self.executor, &self.provider);
        let hops: Vec<_> = opp.hops.iter().map(to_sol_hop).collect();
        let builder = exec
            .execute(opp.loan_token, opp.amount_in, min_profit, hops)
            .from(self.owner);
        match builder.call().await {
            Ok(profit) => match builder.estimate_gas().await {
                Ok(gas) => Ok(SimOutcome::Profitable { profit, gas }),
                Err(e) => Ok(SimOutcome::Reverted(format!("estimate_gas: {e}"))),
            },
            Err(e) => Ok(SimOutcome::Reverted(short(&e.to_string()))),
        }
    }
}

/// Gas for an arbitrage with `hops` swaps (flash loan + swaps + repay). Calibrated on live
/// Arbitrum txs: 2 V3 hops used 300-333k gas. This prices the trade; the gas limit is set higher.
pub fn arb_gas_estimate(hops: usize) -> u64 {
    100_000 + 115_000 * hops as u64
}

impl Simulator {
    /// Latency-critical path: a single eth_call, no estimate_gas round-trip.
    pub async fn simulate_fast(&self, opp: &Opportunity, min_profit: U256) -> Result<SimOutcome> {
        let exec = ArbExecutor::new(self.executor, &self.provider);
        let hops: Vec<_> = opp.hops.iter().map(to_sol_hop).collect();
        let builder = exec
            .execute(opp.loan_token, opp.amount_in, min_profit, hops)
            .from(self.owner);
        match builder.call().await {
            Ok(profit) => Ok(SimOutcome::Profitable {
                profit,
                gas: arb_gas_estimate(opp.hops.len()),
            }),
            Err(e) => Ok(SimOutcome::Reverted(short(&e.to_string()))),
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
    ) -> Result<SimOutcome> {
        let exec = ArbExecutor::new(self.executor, &self.provider);
        let builder = exec
            .liquidate(
                plan.debt_asset,
                plan.collateral_asset,
                plan.flash_amount,
                min_profit,
                plan.calls.iter().map(to_sol_call).collect(),
                hops.iter().map(to_sol_hop).collect(),
            )
            .from(self.owner);
        match builder.call().await {
            Ok(profit) => match builder.estimate_gas().await {
                Ok(gas) => Ok(SimOutcome::Profitable { profit, gas }),
                Err(e) => Ok(SimOutcome::Reverted(format!("estimate_gas: {e}"))),
            },
            Err(e) => Ok(SimOutcome::Reverted(short(&e.to_string()))),
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
