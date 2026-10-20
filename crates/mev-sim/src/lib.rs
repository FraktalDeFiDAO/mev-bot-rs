//! Exact simulation of an opportunity by calling the deployed executor against live state.
//!
//! Shadow mode on a chain with no deployed executor can simulate against a *virtual* one: the
//! executor built from this repo's source, created by an `eth_call` (which returns the runtime
//! code with the immutables set) and injected at a fixed address with a state override on every
//! call, together with the storage its owner would have set (the liquidation target allowlist and
//! the Uniswap V4 PoolManager). It is never used to send.

pub mod pga;

use alloy::{
    network::TransactionBuilder,
    primitives::{Address, B256, Bytes, U256, address, keccak256, map::B256HashMap},
    providers::{DynProvider, Provider},
    rpc::types::{
        TransactionRequest,
        state::{AccountOverride, StateOverride},
    },
    sol_types::{SolCall, SolValue},
};
use anyhow::{Context, Result, ensure};
use mev_bindings::ArbExecutor;
use mev_lending::Plan;
use mev_types::{Hop, Opportunity};

#[derive(Debug, Clone)]
pub enum SimOutcome {
    Profitable { profit: U256, gas: u64 },
    Reverted(String),
}

/// `ArbExecutor` creation code (`forge inspect ArbExecutor bytecode`); `scripts/check.sh` keeps it
/// equal to `contracts/src`.
pub const ARB_EXECUTOR_CREATION: &str = include_str!("arb_executor.creation.hex");
/// Where the virtual executor's code is injected, and the owner it is created by (no keys exist for
/// either; they only appear in `eth_call`s).
pub const VIRTUAL_EXECUTOR: Address = address!("0x00000000000000000000000000000000005ad0e0");
pub const VIRTUAL_OWNER: Address = address!("0x00000000000000000000000000000000005ad0e5");
/// `ArbExecutor` storage (`forge inspect ArbExecutor storageLayout`; `scripts/check.sh` keeps it so):
/// slot 0 `mapping(address => bool) allowedTarget`, slot 1 `address poolManager`.
const SLOT_ALLOWED_TARGET: u64 = 0;
const SLOT_POOL_MANAGER: u64 = 1;

/// Storage slot of `allowedTarget[target]`: keccak256(abi.encode(target, slot)).
fn allowed_target_slot(target: Address) -> B256 {
    keccak256((target, U256::from(SLOT_ALLOWED_TARGET)).abi_encode_params())
}

/// The storage an owner would set on a deployed executor: every `targets` allowlisted
/// (`setAllowedTarget`) and `pool_manager` set (`setPoolManager`).
fn owner_storage(targets: &[Address], pool_manager: Option<Address>) -> B256HashMap<B256> {
    let one = B256::from(U256::from(1));
    let mut m: B256HashMap<B256> = targets
        .iter()
        .map(|t| (allowed_target_slot(*t), one))
        .collect();
    if let Some(pm) = pool_manager {
        m.insert(B256::from(U256::from(SLOT_POOL_MANAGER)), pm.into_word());
    }
    m
}

pub struct Simulator {
    provider: DynProvider,
    executor: Address,
    owner: Address,
    /// The deployed executor has `executeFlashSwap` (must match the sender's setting).
    pub flash_swaps: bool,
    /// Virtual executor: the runtime code every call injects at `executor`.
    code: Option<StateOverride>,
}

impl Simulator {
    pub fn new(provider: DynProvider, executor: Address, owner: Address) -> Self {
        Self {
            provider,
            executor,
            owner,
            flash_swaps: false,
            code: None,
        }
    }

    /// A simulator for an executor that is not deployed (see the module docs). `vault` and `weth`
    /// are its constructor arguments; the profit receiver is the virtual owner. `targets` are the
    /// liquidation targets to allowlist and `pool_manager` the Uniswap V4 PoolManager, as an owner
    /// would set them after deploying.
    pub async fn virtual_executor(
        provider: DynProvider,
        vault: Address,
        weth: Address,
        targets: &[Address],
        pool_manager: Option<Address>,
    ) -> Result<Self> {
        let mut code: Vec<u8> = alloy::primitives::hex::decode(ARB_EXECUTOR_CREATION.trim())
            .context("ARB_EXECUTOR_CREATION is not hex")?;
        code.extend((vault, weth, VIRTUAL_OWNER).abi_encode_params());
        let create = TransactionRequest::default()
            .from(VIRTUAL_OWNER)
            .with_deploy_code(Bytes::from(code));
        let runtime = provider
            .call(create)
            .await
            .context("creating the virtual executor with eth_call")?;
        ensure!(
            !runtime.is_empty(),
            "virtual executor creation returned no code"
        );
        let mut overrides = StateOverride::default();
        overrides.insert(
            VIRTUAL_EXECUTOR,
            AccountOverride {
                code: Some(runtime),
                state_diff: Some(owner_storage(targets, pool_manager)),
                ..Default::default()
            },
        );
        let mut sim = Self::new(provider, VIRTUAL_EXECUTOR, VIRTUAL_OWNER);
        sim.code = Some(overrides);
        Ok(sim)
    }

    /// Whether calls run against injected code rather than a deployed executor.
    pub fn is_virtual(&self) -> bool {
        self.code.is_some()
    }

    async fn eth_call(&self, req: TransactionRequest) -> alloy::transports::TransportResult<Bytes> {
        match &self.code {
            Some(o) => self.provider.call(req).overrides(o.clone()).await,
            None => self.provider.call(req).await,
        }
    }

    async fn estimate(&self, req: TransactionRequest) -> alloy::transports::TransportResult<u64> {
        match &self.code {
            Some(o) => self.provider.estimate_gas(req).overrides(o.clone()).await,
            None => self.provider.estimate_gas(req).await,
        }
    }

    pub fn executor(&self) -> Address {
        self.executor
    }

    pub fn owner(&self) -> Address {
        self.owner
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
            .eth_call(req.clone())
            .await
            .map_err(|e| short(&e.to_string()))?;
        ArbExecutor::executeCall::abi_decode_returns(&raw).map_err(|e| short(&e.to_string()))
    }

    /// `eth_call` the executor as its owner; if it returns profit, estimate gas (on Arbitrum the
    /// estimate includes the L1 data cost).
    pub async fn simulate(&self, opp: &Opportunity, min_profit: U256) -> Result<SimOutcome> {
        let req = self.arb_request(opp, min_profit);
        match self.call_arb(&req).await {
            Ok(profit) => match self.estimate(req).await {
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
    /// Like `simulate_fast`, but against the state at `block` (diagnostics: where did an edge stop existing?).
    pub async fn simulate_fast_at(
        &self,
        opp: &Opportunity,
        min_profit: U256,
        block: alloy::eips::BlockId,
    ) -> Result<SimOutcome> {
        let req = self.arb_request(opp, min_profit);
        let call = self.provider.call(req).block(block);
        let call = match &self.code {
            Some(o) => call.overrides(o.clone()),
            None => call,
        };
        let raw = match call.await {
            Ok(raw) => raw,
            Err(e) => return Ok(SimOutcome::Reverted(short(&e.to_string()))),
        };
        Ok(match ArbExecutor::executeCall::abi_decode_returns(&raw) {
            Ok(profit) => SimOutcome::Profitable {
                profit,
                gas: arb_gas_estimate(opp.hops.len()),
            },
            Err(e) => SimOutcome::Reverted(short(&e.to_string())),
        })
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
            Ok(profit) => match self.estimate(req).await {
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
    fn virtual_executor_creation_code_decodes_and_takes_three_words() {
        let code = alloy::primitives::hex::decode(ARB_EXECUTOR_CREATION.trim()).unwrap();
        // Creation code (runtime 21,376 B on 2026-10-19) plus constructor(vault, weth, receiver).
        assert!(code.len() > 20_000, "{}", code.len());
        assert_eq!(
            (Address::ZERO, Address::ZERO, VIRTUAL_OWNER)
                .abi_encode_params()
                .len(),
            96
        );
        assert_ne!(VIRTUAL_EXECUTOR, VIRTUAL_OWNER);
    }

    #[test]
    fn owner_storage_allowlists_targets_and_sets_the_pool_manager() {
        let morpho = address!("0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb");
        let pm = address!("0x498581fF718922c3f8e6A244956aF099B2652b2b");
        let m = owner_storage(&[morpho], Some(pm));
        assert_eq!(m.len(), 2);
        // Solidity mapping slot: keccak256(pad32(key) ++ pad32(slot)).
        let mut pre = [0u8; 64];
        pre[12..32].copy_from_slice(morpho.as_slice());
        assert_eq!(m[&keccak256(pre)], B256::from(U256::from(1)));
        let mut slot1 = [0u8; 32];
        slot1[31] = 1;
        assert_eq!(m[&B256::from(slot1)].as_slice()[12..], pm.as_slice()[..]);
        assert!(owner_storage(&[], None).is_empty());
    }

    #[test]
    fn arb_gas_estimate_covers_live_two_hop_trades() {
        // Live 2-hop V3 arbs used 299,763 and 302,351 gas.
        let g = arb_gas_estimate(2);
        assert!((300_000..=360_000).contains(&g));
        assert!(arb_gas_estimate(3) > g);
    }
}
