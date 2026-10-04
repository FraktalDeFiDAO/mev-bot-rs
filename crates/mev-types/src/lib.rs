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

/// Mask API keys inside URLs (e.g. `https://arb-mainnet.g.alchemy.com/v2/<key>`,
/// `.../chainstack.com/<key>`) before text reaches logs or event files: any URL path segment
/// or query value of 16+ key-like characters becomes `***`. Hosts and short paths stay readable.
pub fn redact_secrets(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("://") {
        return std::borrow::Cow::Borrowed(s);
    }
    let is_key_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (before, after) = rest.split_at(i + 3);
        out.push_str(before);
        let end = after
            .find(|c: char| c.is_whitespace() || "\"')<>,".contains(c))
            .unwrap_or(after.len());
        let (url, tail) = after.split_at(end);
        // Host stays; every later segment (split on / ? & =) that looks like a key is masked.
        let mut seg = String::new();
        let mut in_host = true;
        for c in url.chars() {
            if "/?&=#".contains(c) {
                push_segment(&mut out, &seg, in_host, is_key_char);
                seg.clear();
                in_host = false;
                out.push(c);
            } else {
                seg.push(c);
            }
        }
        push_segment(&mut out, &seg, in_host, is_key_char);
        rest = tail;
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

fn push_segment(out: &mut String, seg: &str, in_host: bool, is_key_char: impl Fn(char) -> bool) {
    if !in_host && seg.len() >= 16 && seg.chars().all(is_key_char) {
        out.push_str("***");
    } else {
        out.push_str(seg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_kinds_match_executor_constants() {
        assert_eq!(PoolKind::UniV2 { fee_bps: 30 }.hop_kind(), 0);
        assert_eq!(PoolKind::CamelotV2.hop_kind(), 1);
        // All V3-style pools share the executor's swap() path.
        assert_eq!(PoolKind::UniV3 { fee: 500 }.hop_kind(), 2);
        assert_eq!(PoolKind::PancakeV3 { fee: 500 }.hop_kind(), 2);
        assert_eq!(PoolKind::AlgebraV3.hop_kind(), 2);
    }

    #[test]
    fn pool_info_other_and_has() {
        let (a, b, c) = (
            Address::with_last_byte(1),
            Address::with_last_byte(2),
            Address::with_last_byte(3),
        );
        let p = PoolInfo {
            address: Address::ZERO,
            dex: "x".into(),
            kind: PoolKind::CamelotV2,
            token0: a,
            token1: b,
        };
        assert_eq!(p.other(a), b);
        assert_eq!(p.other(b), a);
        assert!(p.has(a) && p.has(b) && !p.has(c));
    }

    #[test]
    fn redacts_api_keys_in_urls_only() {
        // Fake key, assembled at runtime so the pre-commit secret scan doesn't see a literal.
        let key = ["AbCdEf01", "23456789", "xyzKEY"].concat();
        let msg = format!(
            "error sending request for url (https://arb-mainnet.g.alchemy.com/v2/{key}): timeout"
        );
        assert_eq!(
            redact_secrets(&msg),
            "error sending request for url (https://arb-mainnet.g.alchemy.com/v2/***): timeout"
        );
        let ws =
            "wss://arbitrum-mainnet.core.chainstack.com/0123456789abcdef0123456789abcdef closed";
        assert_eq!(
            redact_secrets(ws),
            "wss://arbitrum-mainnet.core.chainstack.com/*** closed"
        );
        let q = "https://x.example/rpc?apikey=ABCDEFGHIJKLMNOPQRST&chain=1";
        assert_eq!(
            redact_secrets(q),
            "https://x.example/rpc?apikey=***&chain=1"
        );
        // Public endpoints, short paths, hex outside URLs, and plain text are untouched.
        for keep in [
            "url=\"https://arb1.arbitrum.io/rpc\"",
            "wss://arb1.arbitrum.io/feed",
            "hash=0x17aef2272ea27e84b386f1cd130f41081f61e516c88b1ccc4840d3963ed2773a",
            "no urls here",
        ] {
            assert_eq!(redact_secrets(keep), keep);
        }
    }
}
