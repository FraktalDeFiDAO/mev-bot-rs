//! Uniswap V4: every pool lives inside the singleton PoolManager; events name the pool by id.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::{SolCall, SolEvent, SolType, sol_data},
};
use mev_protocol::{
    Class, Contract, Decoded, EventKind, LiquidityAction, LiquidityIntent, PoolRef, PriceUpdate,
    Protocol, abi, sqrt_price_update, v4,
};

/// The V4 `PositionManager` on Arbitrum One (an ERC-721 of positions; it calls the PoolManager to change liquidity).
pub const POSITION_MANAGER: Address = address!("d88f38f930b7952f2db2432cb002e7abbf3dd869");

/// `Actions` of v4-periphery that change a position's liquidity.
const INCREASE_LIQUIDITY: u8 = 0x00;
const DECREASE_LIQUIDITY: u8 = 0x01;
const MINT_POSITION: u8 = 0x02;
const BURN_POSITION: u8 = 0x03;
/// Nested `multicall`s are followed this deep.
const MAX_DEPTH: usize = 2;

sol! {
    struct PoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    interface PositionManager {
        function modifyLiquidities(bytes unlockData, uint256 deadline) external payable;
        function multicall(bytes[] data) external payable returns (bytes[] results);
        function increase(uint256 tokenId, uint256 liquidity, uint128 amount0Max, uint128 amount1Max, bytes hookData) external;
        function decrease(uint256 tokenId, uint256 liquidity, uint128 amount0Min, uint128 amount1Min, bytes hookData) external;
        function mint(PoolKey poolKey, int24 tickLower, int24 tickUpper, uint256 liquidity, uint128 amount0Max, uint128 amount1Max, address owner, bytes hookData) external;
        function burn(uint256 tokenId, uint128 amount0Min, uint128 amount1Min, bytes hookData) external;
    }
}

fn intent(action: LiquidityAction) -> LiquidityIntent {
    LiquidityIntent {
        protocol: "uniswap_v4",
        action,
        pool: None,
        token_id: None,
        tick_lower: None,
        tick_upper: None,
        amount0: None,
        amount1: None,
        liquidity: None,
    }
}

/// Every liquidity change in a call to the position manager: the actions of `modifyLiquidities` (also inside
/// `multicall`). Actions that only move tokens (`SETTLE_PAIR`, `TAKE_PAIR`, `CLOSE_CURRENCY`, ...) change no liquidity.
fn position_manager(input: &[u8], depth: usize, out: &mut Vec<LiquidityIntent>) {
    let Some((sel, args)) = abi::split(input) else {
        return;
    };
    if sel == PositionManager::multicallCall::SELECTOR && depth < MAX_DEPTH {
        if let Ok(c) = PositionManager::multicallCall::abi_decode_raw(args) {
            for inner in &c.data {
                position_manager(inner, depth + 1, out);
            }
        }
        return;
    }
    if sel != PositionManager::modifyLiquiditiesCall::SELECTOR {
        return;
    }
    let Ok(call) = PositionManager::modifyLiquiditiesCall::abi_decode_raw(args) else {
        return;
    };
    type Unlock = (sol_data::Bytes, sol_data::Array<sol_data::Bytes>);
    let Ok((actions, params)) = <Unlock as SolType>::abi_decode_params(&call.unlockData) else {
        return;
    };
    for (action, p) in actions.iter().zip(params.iter()) {
        let one = match *action {
            INCREASE_LIQUIDITY => PositionManager::increaseCall::abi_decode_raw(p)
                .ok()
                .map(|c| LiquidityIntent {
                    token_id: Some(c.tokenId),
                    liquidity: Some(c.liquidity),
                    amount0: Some(U256::from(c.amount0Max)),
                    amount1: Some(U256::from(c.amount1Max)),
                    ..intent(LiquidityAction::Add)
                }),
            DECREASE_LIQUIDITY => PositionManager::decreaseCall::abi_decode_raw(p)
                .ok()
                .map(|c| LiquidityIntent {
                    token_id: Some(c.tokenId),
                    liquidity: Some(c.liquidity),
                    amount0: Some(U256::from(c.amount0Min)),
                    amount1: Some(U256::from(c.amount1Min)),
                    ..intent(LiquidityAction::Remove)
                }),
            MINT_POSITION => PositionManager::mintCall::abi_decode_raw(p).ok().map(|c| {
                let k = &c.poolKey;
                let id = v4::pool_id(
                    k.currency0,
                    k.currency1,
                    k.fee.to::<u32>(),
                    k.tickSpacing.as_i32(),
                    k.hooks,
                );
                LiquidityIntent {
                    pool: Some(PoolRef::Address(v4::stand_in(id))),
                    tick_lower: Some(c.tickLower.as_i32()),
                    tick_upper: Some(c.tickUpper.as_i32()),
                    liquidity: Some(c.liquidity),
                    amount0: Some(U256::from(c.amount0Max)),
                    amount1: Some(U256::from(c.amount1Max)),
                    ..intent(LiquidityAction::Add)
                }
            }),
            // Burning removes whatever liquidity the position still has: the amount is not in the call.
            BURN_POSITION => {
                PositionManager::burnCall::abi_decode_raw(p)
                    .ok()
                    .map(|c| LiquidityIntent {
                        token_id: Some(c.tokenId),
                        amount0: Some(U256::from(c.amount0Min)),
                        amount1: Some(U256::from(c.amount1Min)),
                        ..intent(LiquidityAction::Remove)
                    })
            }
            _ => None,
        };
        out.extend(one);
    }
}

sol! {
    interface PoolManager {
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, uint24 fee, int24 tickSpacing, address hooks, uint160 sqrtPriceX96, int24 tick);
        event Donate(bytes32 indexed id, address indexed sender, uint256 amount0, uint256 amount1);
    }
}

pub struct UniswapV4;

impl Protocol for UniswapV4 {
    fn id(&self) -> &'static str {
        "uniswap_v4"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(PoolManager::Swap::SIGNATURE_HASH, "swap_v4", Class::Swap, p),
            EventKind::new(
                PoolManager::ModifyLiquidity::SIGNATURE_HASH,
                "modify_liquidity_v4",
                Class::Liquidity,
                p,
            ),
            // A new pool, and a donation to a pool's LPs: no swap or liquidity change, but part of the full record.
            EventKind::new(
                PoolManager::Initialize::SIGNATURE_HASH,
                "initialize_v4",
                Class::State,
                p,
            ),
            EventKind::new(
                PoolManager::Donate::SIGNATURE_HASH,
                "donate_v4",
                Class::State,
                p,
            ),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "swap_v4")
            .then(|| sqrt_price_update(data))
            .flatten()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        self.decode_calls(to, input).into_iter().next()
    }

    fn decode_calls(&self, to: Address, input: &[u8]) -> Vec<Decoded> {
        let mut out = Vec::new();
        if to == POSITION_MANAGER {
            position_manager(input, 0, &mut out);
        }
        out.into_iter().map(Decoded::Liquidity).collect()
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: POSITION_MANAGER,
            name: "Uniswap V4 PositionManager (modifyLiquidities: mint, increase, decrease, burn)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn position_manager_calls_decode_to_the_liquidity_the_chain_confirmed() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/pm_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (input, events) = l.split_once(',').unwrap();
            let input = alloy::hex::decode(input.trim_start_matches("0x")).unwrap();
            let got = UniswapV4.decode_calls(POSITION_MANAGER, &input);
            let events: Vec<Vec<&str>> =
                events.split(';').map(|e| e.split('/').collect()).collect();
            // A burn removes the rest of a position (no amount in the call); every other intent lines up with
            // the transaction's ModifyLiquidity events, in order.
            let sized: Vec<&LiquidityIntent> = got
                .iter()
                .map(|d| match d {
                    Decoded::Liquidity(i) => i,
                    other => panic!("{other:?}"),
                })
                .filter(|i| i.liquidity.is_some())
                .collect();
            let burns = got.len() - sized.len();
            assert!(
                events.len() >= sized.len() && events.len() <= sized.len() + burns,
                "{l}"
            );
            for (i, e) in sized.iter().zip(&events) {
                let delta: i128 = e[3].parse().unwrap();
                let want = U256::from(delta.unsigned_abs());
                assert_eq!(i.liquidity, Some(want), "{l}");
                assert_eq!(i.action == LiquidityAction::Add, delta > 0, "{l}");
                if let Some(lo) = i.tick_lower {
                    assert_eq!(
                        (lo, i.tick_upper.unwrap()),
                        (e[1].parse().unwrap(), e[2].parse().unwrap()),
                        "{l}"
                    );
                }
                if let Some(PoolRef::Address(a)) = &i.pool {
                    let id: alloy::primitives::B256 = e[0].parse().unwrap();
                    assert_eq!(*a, v4::stand_in(id), "{l}");
                }
            }
            n += 1;
        }
        assert!(n >= 30, "{n}");
    }

    #[test]
    fn other_contracts_and_garbage_decode_to_nothing() {
        assert!(
            UniswapV4
                .decode_calls(Address::with_last_byte(1), &[0xdd, 0x46, 0x50, 0x8f])
                .is_empty()
        );
        assert!(
            UniswapV4
                .decode_calls(POSITION_MANAGER, &[0xdd, 0x46, 0x50, 0x8f, 1, 2])
                .is_empty()
        );
        assert!(UniswapV4.decode_call(POSITION_MANAGER, b"").is_none());
    }

    #[test]
    fn the_four_events_are_distinct_and_only_swap_has_a_price() {
        let ev = UniswapV4.events();
        assert_eq!(ev.len(), 4);
        let topics: std::collections::HashSet<_> = ev.iter().map(|k| k.topic0).collect();
        assert_eq!(topics.len(), 4);
        let data = vec![0u8; 32 * 6];
        for k in &ev {
            assert_eq!(
                UniswapV4.price_update(k, &data).is_some(),
                k.name == "swap_v4",
                "{}",
                k.name
            );
        }
        // Only a swap or a liquidity change is of those classes; a new pool and a donation are state.
        let class = |n: &str| ev.iter().find(|k| k.name == n).unwrap().class;
        assert_eq!(class("swap_v4"), Class::Swap);
        assert_eq!(class("modify_liquidity_v4"), Class::Liquidity);
        assert_eq!(
            (class("initialize_v4"), class("donate_v4")),
            (Class::State, Class::State)
        );
    }
}
