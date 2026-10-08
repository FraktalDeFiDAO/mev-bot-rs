//! Core types for per-protocol decoding crates.
//!
//! A protocol crate (`mev-protocol-<name>`) implements [`Protocol`] for one protocol: the event shapes it emits, what
//! its calldata means, which destinations are its contracts. It depends on nothing but this crate and `alloy`, so
//! it can be used on its own. `mev-decode` collects them in a registry.

use alloy::primitives::{Address, B256, U256};

/// What an event means for the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A trade.
    Swap,
    /// Liquidity added to or removed from a pool.
    Liquidity,
    /// A lending position liquidated or absorbed.
    Liquidation,
    /// The pool's state restated (V2 `Sync`).
    State,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Swap => "swap",
            Class::Liquidity => "liquidity",
            Class::Liquidation => "liquidation",
            Class::State => "state",
        }
    }
}

/// One recognised event shape.
#[derive(Debug, Clone, Copy)]
pub struct EventKind {
    pub topic0: B256,
    /// Short stable name, e.g. `swap_v3`.
    pub name: &'static str,
    pub class: Class,
    /// Protocol family, e.g. `uniswap_v3`.
    pub protocol: &'static str,
}

impl EventKind {
    pub fn new(topic0: B256, name: &'static str, class: Class, protocol: &'static str) -> Self {
        Self {
            topic0,
            name,
            class,
            protocol,
        }
    }
}

/// The price a pool event leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceUpdate {
    /// Concentrated liquidity (V3, Pancake, Algebra, V4): `sqrtPriceX96`, active liquidity and tick after the swap.
    SqrtPrice {
        sqrt_price_x96: U256,
        liquidity: U256,
        tick: i32,
    },
    /// Constant product (V2 `Sync`): the reserves.
    Reserves { reserve0: U256, reserve1: U256 },
}

/// Which pool a swap is aimed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolRef {
    Address(Address),
    /// A Uniswap-V3-style pool named by its tokens and fee tier.
    Pair {
        token_a: Address,
        token_b: Address,
        fee: u32,
    },
    /// A V2-style pair named by its tokens (several factories may have one; which is meant is not in the calldata).
    PairV2 {
        token_a: Address,
        token_b: Address,
    },
}

/// One exact-input swap on one pool. For a multi-hop route this is the first hop (`hops` says how many there are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapIntent {
    pub pool: PoolRef,
    pub token_in: Address,
    pub amount_in: U256,
    /// `sqrtPriceLimitX96`: the swap stops here with input left over. `None` when the call has none (or it is zero).
    pub sqrt_limit_x96: Option<U256>,
    pub hops: u8,
}

/// A liquidation the call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiquidationIntent {
    pub protocol: &'static str,
    /// The positions it targets (Aave/Morpho: one; Compound `absorb`: a list).
    pub borrowers: Vec<Address>,
    pub collateral: Option<Address>,
    pub debt: Option<Address>,
    /// Debt to cover / assets to seize, where the call states it (units depend on the protocol).
    pub amount: Option<U256>,
    /// Morpho market id.
    pub market: Option<B256>,
}

/// What a call's calldata means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Swap(SwapIntent),
    Liquidation(LiquidationIntent),
}

/// A destination a protocol crate has identified.
#[derive(Debug, Clone, Copy)]
pub struct Contract {
    pub address: Address,
    pub name: &'static str,
    /// Whether the protocol's `decode_call` reads calls to it.
    pub decoded: bool,
}

/// One protocol: its events, its calls, its contracts.
pub trait Protocol: Sync {
    /// Stable id, e.g. `uniswap_v3`.
    fn id(&self) -> &'static str;
    /// Event shapes this protocol emits.
    fn events(&self) -> Vec<EventKind>;
    /// The price after one of this protocol's events, where the event carries one.
    fn price_update(&self, _kind: &EventKind, _data: &[u8]) -> Option<PriceUpdate> {
        None
    }
    /// The intent of a call to `to`, if it is one of this protocol's contracts and a function it decodes.
    fn decode_call(&self, _to: Address, _input: &[u8]) -> Option<Decoded> {
        None
    }
    /// Destinations identified for this protocol, decoded or not.
    fn contracts(&self) -> Vec<Contract> {
        Vec::new()
    }
}

/// ABI helpers over 32-byte words (arguments after the 4-byte selector).
pub mod abi {
    use alloy::primitives::{Address, U256};

    pub fn word(d: &[u8], i: usize) -> Option<&[u8]> {
        d.get(32 * i..32 * i + 32)
    }

    /// An address word: the high 12 bytes must be zero.
    pub fn addr(d: &[u8], i: usize) -> Option<Address> {
        let w = word(d, i)?;
        w[..12]
            .iter()
            .all(|b| *b == 0)
            .then(|| Address::from_slice(&w[12..]))
    }

    pub fn uint(d: &[u8], i: usize) -> Option<U256> {
        word(d, i).map(U256::from_be_slice)
    }

    /// `(selector, arguments)` of calldata.
    pub fn split(input: &[u8]) -> Option<([u8; 4], &[u8])> {
        let sel: [u8; 4] = input.get(..4)?.try_into().ok()?;
        Some((sel, &input[4..]))
    }
}

/// The price update carried by a concentrated-liquidity `Swap` log's data (`amount0, amount1, sqrtPriceX96,
/// liquidity, tick`, each a 32-byte word). Shared by V3, Pancake, Algebra and V4.
pub fn sqrt_price_update(data: &[u8]) -> Option<PriceUpdate> {
    let tick_word = data.get(32 * 4..32 * 5)?;
    // int24 sign-extended into the 32-byte word.
    let tick = i32::from_be_bytes([tick_word[28], tick_word[29], tick_word[30], tick_word[31]]);
    Some(PriceUpdate::SqrtPrice {
        sqrt_price_x96: abi::uint(data, 2)?,
        liquidity: abi::uint(data, 3)?,
        tick,
    })
}

/// The reserves in a V2 `Sync` log's data.
pub fn reserves_update(data: &[u8]) -> Option<PriceUpdate> {
    Some(PriceUpdate::Reserves {
        reserve0: abi::uint(data, 0)?,
        reserve1: abi::uint(data, 1)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_words_with_dirty_high_bytes_are_rejected() {
        let mut d = vec![0u8; 32];
        d[31] = 7;
        assert_eq!(abi::addr(&d, 0), Some(Address::with_last_byte(7)));
        d[0] = 1;
        assert_eq!(abi::addr(&d, 0), None);
        assert_eq!(abi::addr(&d, 1), None, "out of range");
    }

    #[test]
    fn sqrt_price_and_reserves_are_read_from_log_data() {
        let mut data = vec![0u8; 32 * 5];
        data[32 * 2 + 31] = 7;
        data[32 * 3 + 31] = 9;
        data[32 * 4..].fill(0xff); // tick -1
        assert_eq!(
            sqrt_price_update(&data),
            Some(PriceUpdate::SqrtPrice {
                sqrt_price_x96: U256::from(7),
                liquidity: U256::from(9),
                tick: -1
            })
        );
        assert_eq!(sqrt_price_update(&data[..64]), None);
        let mut d = vec![0u8; 64];
        d[31] = 3;
        d[63] = 4;
        assert_eq!(
            reserves_update(&d),
            Some(PriceUpdate::Reserves {
                reserve0: U256::from(3),
                reserve1: U256::from(4)
            })
        );
    }

    #[test]
    fn split_needs_a_selector() {
        assert_eq!(abi::split(&[1, 2, 3]), None);
        assert_eq!(
            abi::split(&[1, 2, 3, 4, 5]),
            Some(([1, 2, 3, 4], &[5u8][..]))
        );
    }
}
