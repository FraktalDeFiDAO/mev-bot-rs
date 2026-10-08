//! Pool and lending events by topic0: what kind of event a log is (swap, liquidity change, liquidation, state), which
//! protocol it belongs to, and, for the pool events that carry one, the price it leaves behind.
//!
//! Chain logs are the source of truth for prices and for whether a transaction did anything: a feed transaction that
//! reverts emits none. This module is the single table of event shapes the bot recognises; adding a protocol means
//! adding its events here and a row to `kinds()`.

// `sol!` generates a function per event with one argument per field (Morpho's `Liquidate` has 8).
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{B256, U256},
    sol,
    sol_types::SolEvent,
};

sol! {
    interface UniswapV2 {
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Sync(uint112 reserve0, uint112 reserve1);
        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    }
    interface UniswapV3 {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }
    interface PancakeV3 {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint128 protocolFeesToken0, uint128 protocolFeesToken1);
    }
    interface UniswapV4 {
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
    }
    interface UniswapNftManager {
        event IncreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event DecreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
    }
    interface Balancer {
        event Swap(bytes32 indexed poolId, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut);
    }
    interface Aave {
        event LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken);
    }
    interface Morpho {
        event Liquidate(bytes32 indexed id, address indexed caller, address indexed borrower, uint256 repaidAssets, uint256 repaidShares, uint256 seizedAssets, uint256 badDebtAssets, uint256 badDebtShares);
    }
    interface Comet {
        event AbsorbCollateral(address indexed absorber, address indexed borrower, address indexed asset, uint256 collateralAbsorbed, uint256 usdValue);
        event AbsorbDebt(address indexed absorber, address indexed borrower, uint256 basePaidOut, uint256 usdValue);
    }
}

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
pub struct Kind {
    pub topic0: B256,
    /// Short stable name, e.g. `swap_v3`.
    pub name: &'static str,
    pub class: Class,
    /// Protocol family, e.g. `uniswap_v3`.
    pub protocol: &'static str,
}

/// Every event the bot recognises.
pub fn kinds() -> &'static [Kind] {
    static T: std::sync::OnceLock<Vec<Kind>> = std::sync::OnceLock::new();
    let k = |topic0, name, class, protocol| Kind {
        topic0,
        name,
        class,
        protocol,
    };
    T.get_or_init(|| {
        vec![
            k(
                UniswapV2::Swap::SIGNATURE_HASH,
                "swap_v2",
                Class::Swap,
                "uniswap_v2",
            ),
            k(
                UniswapV2::Sync::SIGNATURE_HASH,
                "sync_v2",
                Class::State,
                "uniswap_v2",
            ),
            k(
                UniswapV2::Mint::SIGNATURE_HASH,
                "mint_v2",
                Class::Liquidity,
                "uniswap_v2",
            ),
            k(
                UniswapV2::Burn::SIGNATURE_HASH,
                "burn_v2",
                Class::Liquidity,
                "uniswap_v2",
            ),
            k(
                UniswapV3::Swap::SIGNATURE_HASH,
                "swap_v3",
                Class::Swap,
                "uniswap_v3",
            ),
            k(
                PancakeV3::Swap::SIGNATURE_HASH,
                "swap_v3_pancake",
                Class::Swap,
                "pancake_v3",
            ),
            k(
                UniswapV3::Mint::SIGNATURE_HASH,
                "mint_v3",
                Class::Liquidity,
                "uniswap_v3",
            ),
            k(
                UniswapV3::Burn::SIGNATURE_HASH,
                "burn_v3",
                Class::Liquidity,
                "uniswap_v3",
            ),
            k(
                UniswapNftManager::IncreaseLiquidity::SIGNATURE_HASH,
                "increase_liquidity_nft",
                Class::Liquidity,
                "uniswap_v3",
            ),
            k(
                UniswapNftManager::DecreaseLiquidity::SIGNATURE_HASH,
                "decrease_liquidity_nft",
                Class::Liquidity,
                "uniswap_v3",
            ),
            k(
                UniswapV4::Swap::SIGNATURE_HASH,
                "swap_v4",
                Class::Swap,
                "uniswap_v4",
            ),
            k(
                UniswapV4::ModifyLiquidity::SIGNATURE_HASH,
                "modify_liquidity_v4",
                Class::Liquidity,
                "uniswap_v4",
            ),
            k(
                Balancer::Swap::SIGNATURE_HASH,
                "swap_balancer",
                Class::Swap,
                "balancer",
            ),
            k(
                Aave::LiquidationCall::SIGNATURE_HASH,
                "liquidation_aave",
                Class::Liquidation,
                "aave_v3",
            ),
            k(
                Morpho::Liquidate::SIGNATURE_HASH,
                "liquidation_morpho",
                Class::Liquidation,
                "morpho_blue",
            ),
            k(
                Comet::AbsorbCollateral::SIGNATURE_HASH,
                "absorb_collateral_compound",
                Class::Liquidation,
                "compound_v3",
            ),
            k(
                Comet::AbsorbDebt::SIGNATURE_HASH,
                "absorb_debt_compound",
                Class::Liquidation,
                "compound_v3",
            ),
        ]
    })
}

/// The recognised kind of an event with this `topic0`.
pub fn classify(topic0: &B256) -> Option<&'static Kind> {
    kinds().iter().find(|k| &k.topic0 == topic0)
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

fn word(data: &[u8], i: usize) -> Option<U256> {
    data.get(32 * i..32 * i + 32).map(U256::from_be_slice)
}

/// The price after the event, for the events that carry one (`Swap` on V3-style pools and V4, `Sync` on V2). A
/// reverted transaction emits no events, so a price taken from logs is always a price that happened.
pub fn price_update(kind: &Kind, data: &[u8]) -> Option<PriceUpdate> {
    match kind.name {
        "swap_v3" | "swap_v3_pancake" | "swap_v4" => {
            let tick_word = data.get(32 * 4..32 * 5)?;
            // int24 sign-extended into the 32-byte word.
            let tick =
                i32::from_be_bytes([tick_word[28], tick_word[29], tick_word[30], tick_word[31]]);
            Some(PriceUpdate::SqrtPrice {
                sqrt_price_x96: word(data, 2)?,
                liquidity: word(data, 3)?,
                tick,
            })
        }
        "sync_v2" => Some(PriceUpdate::Reserves {
            reserve0: word(data, 0)?,
            reserve1: word(data, 1)?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    #[test]
    fn well_known_event_hashes_match() {
        let known = [
            (
                "swap_v3",
                b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"),
            ),
            (
                "swap_v2",
                b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822"),
            ),
            (
                "sync_v2",
                b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"),
            ),
            (
                "liquidation_aave",
                b256!("e413a321e8681d831f4dbccbca790d2952b56f977908e45be37335533e005286"),
            ),
        ];
        for (name, hash) in known {
            assert_eq!(
                kinds().iter().find(|k| k.name == name).map(|k| k.topic0),
                Some(hash),
                "{name}"
            );
        }
    }

    #[test]
    fn every_kind_has_a_distinct_topic_and_name() {
        let mut topics = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for k in kinds() {
            assert!(topics.insert(k.topic0), "duplicate topic for {}", k.name);
            assert!(names.insert(k.name), "duplicate name {}", k.name);
        }
        assert!(kinds().len() >= 17);
    }

    #[test]
    fn the_definitions_agree_with_the_bot_bindings() {
        // The bot's own bindings define the same events; if they ever drift apart this catches it.
        assert_eq!(
            UniswapV3::Swap::SIGNATURE_HASH,
            mev_bindings::IUniswapV3Pool::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            UniswapV3::Mint::SIGNATURE_HASH,
            mev_bindings::IUniswapV3Pool::Mint::SIGNATURE_HASH
        );
        assert_eq!(
            UniswapV3::Burn::SIGNATURE_HASH,
            mev_bindings::IUniswapV3Pool::Burn::SIGNATURE_HASH
        );
        assert_eq!(
            PancakeV3::Swap::SIGNATURE_HASH,
            mev_bindings::IPancakeV3Pool::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            UniswapV2::Sync::SIGNATURE_HASH,
            mev_bindings::IUniswapV2Pair::Sync::SIGNATURE_HASH
        );
        assert_eq!(
            UniswapV4::Swap::SIGNATURE_HASH,
            mev_bindings::IPoolManager::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            UniswapV4::ModifyLiquidity::SIGNATURE_HASH,
            mev_bindings::IPoolManager::ModifyLiquidity::SIGNATURE_HASH
        );
    }

    #[test]
    fn a_v3_swap_gives_its_price_and_a_sync_gives_reserves() {
        let swap = classify(&UniswapV3::Swap::SIGNATURE_HASH).unwrap();
        let mut data = vec![0u8; 32 * 5];
        data[32 * 2 + 31] = 7; // sqrtPriceX96 = 7
        data[32 * 3 + 31] = 9; // liquidity = 9
        data[32 * 4..32 * 5].fill(0xff); // tick = -1 (sign-extended)
        assert_eq!(
            price_update(swap, &data),
            Some(PriceUpdate::SqrtPrice {
                sqrt_price_x96: U256::from(7),
                liquidity: U256::from(9),
                tick: -1
            })
        );
        let sync = classify(&UniswapV2::Sync::SIGNATURE_HASH).unwrap();
        let mut d = vec![0u8; 64];
        d[31] = 3;
        d[63] = 4;
        assert_eq!(
            price_update(sync, &d),
            Some(PriceUpdate::Reserves {
                reserve0: U256::from(3),
                reserve1: U256::from(4)
            })
        );
        assert_eq!(price_update(swap, &data[..64]), None, "truncated data");
        let burn = classify(&UniswapV3::Burn::SIGNATURE_HASH).unwrap();
        assert_eq!(price_update(burn, &data), None, "no price in a burn");
    }
}
