//! Uniswap V3-style pools (also Sushi V3; Camelot V3/Algebra emits the same `Swap` shape and so shares the topic).
//! Events only: the routers that call these pools are in `mev-protocol-uniswap-router`.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, U256, address, b256},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{
    Class, Contract, Decoded, EventKind, LiquidityAction, LiquidityIntent, PoolRef, PriceUpdate,
    Protocol, abi, sqrt_price_update,
};

sol! {
    interface Pool {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }
    interface NftManager {
        event IncreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event DecreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
    }
}

sol! {
    interface NpmCalls {
        struct MintParams { address token0; address token1; uint24 fee; int24 tickLower; int24 tickUpper; uint256 amount0Desired; uint256 amount1Desired; uint256 amount0Min; uint256 amount1Min; address recipient; uint256 deadline; }
        struct IncreaseLiquidityParams { uint256 tokenId; uint256 amount0Desired; uint256 amount1Desired; uint256 amount0Min; uint256 amount1Min; uint256 deadline; }
        struct DecreaseLiquidityParams { uint256 tokenId; uint128 liquidity; uint256 amount0Min; uint256 amount1Min; uint256 deadline; }
        function mint(MintParams params) external payable;
        function increaseLiquidity(IncreaseLiquidityParams params) external payable;
        function decreaseLiquidity(DecreaseLiquidityParams params) external payable;
        function multicall(bytes[] data) external payable;
    }
}

/// Uniswap V3 `NonfungiblePositionManager` on Arbitrum One.
pub const NFT_MANAGER: Address = address!("C36442b4a4522E871399CD717aBDD847Ab11FE88");
/// Nested `multicall`s are followed this deep.
const MAX_DEPTH: usize = 2;

fn liquidity(action: LiquidityAction) -> LiquidityIntent {
    LiquidityIntent {
        protocol: "uniswap_v3",
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

/// The first liquidity change in a call to the position manager (`mint`, `increaseLiquidity`, `decreaseLiquidity`,
/// or one of those inside `multicall`).
fn npm(input: &[u8], depth: usize) -> Option<LiquidityIntent> {
    let (sel, args) = abi::split(input)?;
    if sel == NpmCalls::mintCall::SELECTOR {
        let p = NpmCalls::mintCall::abi_decode_raw(args).ok()?.params;
        return Some(LiquidityIntent {
            pool: Some(PoolRef::Pair {
                token_a: p.token0,
                token_b: p.token1,
                fee: p.fee.to::<u32>(),
            }),
            tick_lower: Some(p.tickLower.as_i32()),
            tick_upper: Some(p.tickUpper.as_i32()),
            amount0: Some(p.amount0Desired),
            amount1: Some(p.amount1Desired),
            ..liquidity(LiquidityAction::Add)
        });
    }
    if sel == NpmCalls::increaseLiquidityCall::SELECTOR {
        let p = NpmCalls::increaseLiquidityCall::abi_decode_raw(args)
            .ok()?
            .params;
        return Some(LiquidityIntent {
            token_id: Some(p.tokenId),
            amount0: Some(p.amount0Desired),
            amount1: Some(p.amount1Desired),
            ..liquidity(LiquidityAction::Add)
        });
    }
    if sel == NpmCalls::decreaseLiquidityCall::SELECTOR {
        let p = NpmCalls::decreaseLiquidityCall::abi_decode_raw(args)
            .ok()?
            .params;
        return Some(LiquidityIntent {
            token_id: Some(p.tokenId),
            liquidity: Some(U256::from(p.liquidity)),
            amount0: Some(p.amount0Min),
            amount1: Some(p.amount1Min),
            ..liquidity(LiquidityAction::Remove)
        });
    }
    if sel == NpmCalls::multicallCall::SELECTOR && depth < MAX_DEPTH {
        let calls = NpmCalls::multicallCall::abi_decode_raw(args).ok()?.data;
        return calls.iter().find_map(|c| npm(c, depth + 1));
    }
    None
}

pub struct UniswapV3;

impl Protocol for UniswapV3 {
    fn id(&self) -> &'static str {
        "uniswap_v3"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(Pool::Swap::SIGNATURE_HASH, "swap_v3", Class::Swap, p),
            EventKind::new(Pool::Mint::SIGNATURE_HASH, "mint_v3", Class::Liquidity, p),
            EventKind::new(Pool::Burn::SIGNATURE_HASH, "burn_v3", Class::Liquidity, p),
            EventKind::new(
                NftManager::IncreaseLiquidity::SIGNATURE_HASH,
                "increase_liquidity_nft",
                Class::Liquidity,
                p,
            ),
            EventKind::new(
                NftManager::DecreaseLiquidity::SIGNATURE_HASH,
                "decrease_liquidity_nft",
                Class::Liquidity,
                p,
            ),
            // Camelot V3 (Algebra) emits this just before each `Swap`: no indexed arguments, two values (seen as 250 and
            // 250, i.e. 0.025%), most likely the fee in effect for the swap. Signature not known, so the topic is
            // given directly. Recognised (state class) so it stops showing up as an unknown event; its data is not
            // used yet.
            EventKind::new(
                b256!("8a89de70856bccec096661388f305b9a75f5f65cb0d8a0e1e803c39dabedb57f"),
                "fee_algebra_observed",
                Class::State,
                p,
            ),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "swap_v3")
            .then(|| sqrt_price_update(data))
            .flatten()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        (to == NFT_MANAGER)
            .then(|| npm(input, 0))
            .flatten()
            .map(Decoded::Liquidity)
    }

    fn contracts(&self) -> Vec<Contract> {
        vec![Contract {
            address: NFT_MANAGER,
            name: "Uniswap V3 NonfungiblePositionManager (mint, increase, decrease, multicall decoded)",
            decoded: true,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    #[test]
    fn the_swap_topic_matches_the_known_mainnet_value() {
        let swap = UniswapV3
            .events()
            .into_iter()
            .find(|k| k.name == "swap_v3")
            .unwrap();
        assert_eq!(
            swap.topic0,
            b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67")
        );
    }

    #[test]
    fn a_swap_carries_a_price_and_a_burn_does_not() {
        let ev = UniswapV3.events();
        let swap = ev.iter().find(|k| k.name == "swap_v3").unwrap();
        let burn = ev.iter().find(|k| k.name == "burn_v3").unwrap();
        let mut data = vec![0u8; 32 * 5];
        data[32 * 2 + 31] = 7;
        assert!(matches!(
            UniswapV3.price_update(swap, &data),
            Some(PriceUpdate::SqrtPrice { .. })
        ));
        assert_eq!(UniswapV3.price_update(burn, &data), None);
    }

    fn lq(d: Option<Decoded>) -> LiquidityIntent {
        match d {
            Some(Decoded::Liquidity(l)) => l,
            other => panic!("expected a liquidity change, got {other:?}"),
        }
    }

    #[test]
    fn mint_names_the_pool_ticks_and_amounts() {
        use alloy::primitives::aliases::{I24, U24};
        let c = NpmCalls::mintCall {
            params: NpmCalls::MintParams {
                token0: Address::with_last_byte(1),
                token1: Address::with_last_byte(2),
                fee: U24::from(500u32),
                tickLower: I24::try_from(-887_270i32).unwrap(),
                tickUpper: I24::try_from(60i32).unwrap(),
                amount0Desired: U256::from(1_000u64),
                amount1Desired: U256::from(2_000u64),
                amount0Min: U256::ZERO,
                amount1Min: U256::ZERO,
                recipient: Address::with_last_byte(9),
                deadline: U256::from(1u8),
            },
        };
        assert_eq!(
            &c.abi_encode()[..4],
            [0x88, 0x31, 0x64, 0x56],
            "mint selector"
        );
        let l = lq(UniswapV3.decode_call(NFT_MANAGER, &c.abi_encode()));
        assert_eq!(l.action, LiquidityAction::Add);
        assert_eq!(
            l.pool,
            Some(PoolRef::Pair {
                token_a: Address::with_last_byte(1),
                token_b: Address::with_last_byte(2),
                fee: 500
            })
        );
        assert_eq!((l.tick_lower, l.tick_upper), (Some(-887_270), Some(60)));
        assert_eq!(
            (l.amount0, l.amount1),
            (Some(U256::from(1_000u64)), Some(U256::from(2_000u64)))
        );
        assert!(
            UniswapV3
                .decode_call(Address::with_last_byte(5), &c.abi_encode())
                .is_none(),
            "another contract"
        );
    }

    #[test]
    fn increase_and_decrease_carry_the_position_id() {
        let inc = NpmCalls::increaseLiquidityCall {
            params: NpmCalls::IncreaseLiquidityParams {
                tokenId: U256::from(5_717_406u64),
                amount0Desired: U256::from(7u8),
                amount1Desired: U256::from(8u8),
                amount0Min: U256::ZERO,
                amount1Min: U256::ZERO,
                deadline: U256::from(1u8),
            },
        };
        assert_eq!(&inc.abi_encode()[..4], [0x21, 0x9f, 0x5d, 0x17]);
        let l = lq(UniswapV3.decode_call(NFT_MANAGER, &inc.abi_encode()));
        assert_eq!(
            (l.action, l.token_id, l.pool),
            (LiquidityAction::Add, Some(U256::from(5_717_406u64)), None)
        );
        let dec = NpmCalls::decreaseLiquidityCall {
            params: NpmCalls::DecreaseLiquidityParams {
                tokenId: U256::from(5_719_107u64),
                liquidity: 123_456_789_012u128,
                amount0Min: U256::ZERO,
                amount1Min: U256::ZERO,
                deadline: U256::from(1u8),
            },
        };
        assert_eq!(&dec.abi_encode()[..4], [0x0c, 0x49, 0xcc, 0xbe]);
        let l = lq(UniswapV3.decode_call(NFT_MANAGER, &dec.abi_encode()));
        assert_eq!(
            (l.action, l.liquidity),
            (
                LiquidityAction::Remove,
                Some(U256::from(123_456_789_012u128))
            )
        );
    }

    #[test]
    fn multicall_is_unwrapped_to_the_first_liquidity_change_and_skips_other_calls() {
        use alloy::primitives::Bytes;
        let dec = NpmCalls::decreaseLiquidityCall {
            params: NpmCalls::DecreaseLiquidityParams {
                tokenId: U256::from(42u8),
                liquidity: 10,
                amount0Min: U256::ZERO,
                amount1Min: U256::ZERO,
                deadline: U256::from(1u8),
            },
        }
        .abi_encode();
        let collect = vec![0xfc, 0x6f, 0x78, 0x65, 0, 0, 0]; // collect(...): not a liquidity change
        let m = NpmCalls::multicallCall {
            data: vec![Bytes::from(collect), Bytes::from(dec)],
        };
        assert_eq!(&m.abi_encode()[..4], [0xac, 0x96, 0x50, 0xd8]);
        let l = lq(UniswapV3.decode_call(NFT_MANAGER, &m.abi_encode()));
        assert_eq!(
            (l.action, l.token_id),
            (LiquidityAction::Remove, Some(U256::from(42u8)))
        );
        let only = NpmCalls::multicallCall {
            data: vec![Bytes::from(vec![0xfc, 0x6f, 0x78, 0x65])],
        };
        assert!(
            UniswapV3
                .decode_call(NFT_MANAGER, &only.abi_encode())
                .is_none()
        );
    }
}
