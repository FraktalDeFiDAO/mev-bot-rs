//! Shared domain types used across the bot.

pub use alloy::primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

/// How a pool prices swaps. Mirrors the executor contract's hop kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolKind {
    /// Uniswap V2 / Sushi V2 style constant product with a fixed fee.
    UniV2 { fee_bps: u16 },
    /// Camelot V2 volatile pair: directional fees, priced on-chain via `getAmountOut`.
    CamelotV2,
    /// Uniswap V3 / Sushi V3 concentrated liquidity. `fee` in hundredths of a bip (500 = 0.05%).
    UniV3 { fee: u32 },
    /// PancakeSwap V3: Uniswap V3 math, different slot0 layout and Swap event.
    PancakeV3 { fee: u32 },
    /// Algebra (Camelot V3): one pool per pair, dynamic directional fees from `globalState()`.
    AlgebraV3,
}

impl PoolKind {
    /// Hop kind id understood by `ArbExecutor`.
    pub fn hop_kind(&self) -> u8 {
        match self {
            PoolKind::UniV2 { .. } => 0,
            PoolKind::CamelotV2 => 1,
            // All V3-style pools share the executor's swap() path; only the callback name differs.
            PoolKind::UniV3 { .. } | PoolKind::PancakeV3 { .. } | PoolKind::AlgebraV3 => 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolInfo {
    pub address: Address,
    pub dex: String,
    pub kind: PoolKind,
    pub token0: Address,
    pub token1: Address,
}

impl PoolInfo {
    pub fn other(&self, token: Address) -> Address {
        if token == self.token0 {
            self.token1
        } else {
            self.token0
        }
    }

    pub fn has(&self, token: Address) -> bool {
        token == self.token0 || token == self.token1
    }
}

/// One swap in a route, in the shape the executor contract expects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hop {
    pub kind: u8,
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub zero_for_one: bool,
    pub fee_bps: u16,
}

/// A candidate arbitrage found by the search, before simulation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Opportunity {
    pub loan_token: Address,
    pub amount_in: U256,
    pub hops: Vec<Hop>,
    /// Estimated profit in loan-token raw units (approximate model).
    pub est_profit: f64,
    /// Estimated profit in USD (approximate model).
    pub est_profit_usd: f64,
    pub buy_pool: Address,
    pub sell_pool: Address,
}
