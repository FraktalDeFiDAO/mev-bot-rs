//! Uniswap V2-style constant-product pairs (also Sushi V2 and Camelot V2): their events, and the calldata of the V2-style
//! routers seen in the feed (`swapExactTokensForTokens` and its fee-on-transfer variant). A router call names the
//! swap's tokens, not the pair, so the pair is `PoolRef::PairV2`; multi-hop paths report the first hop and a hop count.

// `sol!` generates one function per event with an argument per field.
#![allow(clippy::too_many_arguments)]

use alloy::{
    primitives::{Address, U256, address},
    sol,
    sol_types::{SolCall, SolEvent},
};
use mev_protocol::{
    Class, Contract, Decoded, EventKind, PoolRef, PriceUpdate, Protocol, SwapIntent, abi,
    reserves_update,
};

sol! {
    interface Pair {
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Sync(uint112 reserve0, uint112 reserve1);
        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    }
}

sol! {
    interface V2Router {
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
        function swapExactTokensForETH(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactTokensForETHSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
    }
}

/// V2-style routers seen in the feed: `(address, factory it answers to)`. The first two answer to factories we do not
/// hold (identified by `factory()` on chain on 2026-10-08, swap events of the V2 shape only); the last two are the Sushi
/// V2 and Camelot V2 routers, whose pairs we do hold (their calls name tokens, so they resolve to `PoolRef::PairV2`).
pub const ROUTERS: [(Address, Address); 4] = [
    (
        address!("5eb6543d20015e68c4674fba2ce2273d16ad51b7"),
        address!("9b43e504d84adcb7321dc2551a86bff1084ae2e5"),
    ),
    (
        address!("82dfd2b94222bdb603aa6b34a8d37311ab3db800"),
        address!("c8b66e63976670d25feeaad28fe317212766d823"),
    ),
    (
        address!("1b02da8cb0d097eb8d57a175b88c7d8b47997506"),
        address!("c35dadb65012ec5796536bd9864ed8773abc74c4"),
    ),
    (
        address!("4752ba5dbc23f44d87826276bf6fd6b1c372ad24"),
        address!("6eccab422d763ac031210895c81787e87b43a652"),
    ),
];

fn router_swap(input: &[u8]) -> Option<SwapIntent> {
    let (sel, args) = abi::split(input)?;
    let (amount_in, path) = if sel == V2Router::swapExactTokensForTokensCall::SELECTOR {
        let c = V2Router::swapExactTokensForTokensCall::abi_decode_raw(args).ok()?;
        (c.amountIn, c.path)
    } else if sel == V2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::SELECTOR {
        let c =
            V2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_raw(
                args,
            )
            .ok()?;
        (c.amountIn, c.path)
    } else if sel == V2Router::swapExactTokensForETHCall::SELECTOR {
        let c = V2Router::swapExactTokensForETHCall::abi_decode_raw(args).ok()?;
        (c.amountIn, c.path)
    } else if sel == V2Router::swapExactTokensForETHSupportingFeeOnTransferTokensCall::SELECTOR {
        let c =
            V2Router::swapExactTokensForETHSupportingFeeOnTransferTokensCall::abi_decode_raw(args)
                .ok()?;
        (c.amountIn, c.path)
    } else {
        return None;
    };
    let (a, b) = (*path.first()?, *path.get(1)?);
    if amount_in == U256::ZERO || a == b {
        return None;
    }
    Some(SwapIntent {
        pool: PoolRef::PairV2 {
            token_a: a,
            token_b: b,
        },
        token_in: a,
        amount_in,
        sqrt_limit_x96: None,
        hops: u8::try_from(path.len() - 1).ok()?,
    })
}

pub struct UniswapV2;

impl Protocol for UniswapV2 {
    fn id(&self) -> &'static str {
        "uniswap_v2"
    }

    fn events(&self) -> Vec<EventKind> {
        let p = self.id();
        vec![
            EventKind::new(Pair::Swap::SIGNATURE_HASH, "swap_v2", Class::Swap, p),
            EventKind::new(Pair::Sync::SIGNATURE_HASH, "sync_v2", Class::State, p),
            EventKind::new(Pair::Mint::SIGNATURE_HASH, "mint_v2", Class::Liquidity, p),
            EventKind::new(Pair::Burn::SIGNATURE_HASH, "burn_v2", Class::Liquidity, p),
        ]
    }

    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> {
        (kind.name == "sync_v2")
            .then(|| reserves_update(data))
            .flatten()
    }

    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> {
        ROUTERS
            .iter()
            .any(|(r, _)| *r == to)
            .then(|| router_swap(input))
            .flatten()
            .map(Decoded::Swap)
    }

    fn contracts(&self) -> Vec<Contract> {
        ROUTERS
            .iter()
            .map(|(address, _)| Contract {
                address: *address,
                name: "V2-style router (swapExactTokensFor{Tokens,ETH} and fee-on-transfer variants decoded; ETH-in variants not)",
                decoded: true,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    #[test]
    fn landed_router_calls_decode_to_their_first_hop_token_and_amount() {
        let mut n = 0;
        for l in include_str!("../tests/fixtures/router_rows.csv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<&str> = l.split(',').collect();
            let input = alloy::hex::decode(f[1].trim_start_matches("0x")).unwrap();
            let Some(Decoded::Swap(s)) = UniswapV2.decode_call(f[0].parse().unwrap(), &input)
            else {
                panic!("not decoded: {l}")
            };
            assert_eq!(s.token_in, f[2].parse::<Address>().unwrap(), "{l}");
            assert_eq!(
                s.pool,
                PoolRef::PairV2 {
                    token_a: f[2].parse().unwrap(),
                    token_b: f[3].parse().unwrap()
                },
                "{l}"
            );
            assert_eq!(s.hops, f[4].parse::<u8>().unwrap(), "{l}");
            assert_eq!(s.amount_in, f[5].parse::<U256>().unwrap(), "{l}");
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }

    #[test]
    fn topics_match_the_known_mainnet_values() {
        let by = |n: &str| {
            UniswapV2
                .events()
                .into_iter()
                .find(|k| k.name == n)
                .unwrap()
                .topic0
        };
        assert_eq!(
            by("swap_v2"),
            b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822")
        );
        assert_eq!(
            by("sync_v2"),
            b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1")
        );
    }

    #[test]
    fn only_sync_carries_a_price() {
        let ev = UniswapV2.events();
        let sync = ev.iter().find(|k| k.name == "sync_v2").unwrap();
        let swap = ev.iter().find(|k| k.name == "swap_v2").unwrap();
        let mut d = vec![0u8; 64];
        d[31] = 3;
        d[63] = 4;
        assert!(matches!(
            UniswapV2.price_update(sync, &d),
            Some(PriceUpdate::Reserves { .. })
        ));
        assert_eq!(UniswapV2.price_update(swap, &d), None);
    }

    fn swap(d: Option<Decoded>) -> SwapIntent {
        match d {
            Some(Decoded::Swap(s)) => s,
            other => panic!("expected a swap, got {other:?}"),
        }
    }

    #[test]
    fn router_swaps_name_the_pair_the_first_hop_and_the_hop_count() {
        let (router, _) = ROUTERS[0];
        let (a, b, c) = (
            Address::with_last_byte(1),
            Address::with_last_byte(2),
            Address::with_last_byte(3),
        );
        let call = V2Router::swapExactTokensForTokensCall {
            amountIn: U256::from(33_653_572u64),
            amountOutMin: U256::from(1u8),
            path: vec![a, b, c],
            to: Address::with_last_byte(9),
            deadline: U256::from(2_000_000_000u64),
        };
        assert_eq!(
            &call.abi_encode()[..4],
            [0x38, 0xed, 0x17, 0x39],
            "selector seen on chain"
        );
        let s = swap(UniswapV2.decode_call(router, &call.abi_encode()));
        assert_eq!(
            s.pool,
            PoolRef::PairV2 {
                token_a: a,
                token_b: b
            }
        );
        assert_eq!(
            (s.token_in, s.amount_in, s.hops),
            (a, U256::from(33_653_572u64), 2)
        );
        let fee = V2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
            amountIn: U256::from(5u8),
            amountOutMin: U256::ZERO,
            path: vec![b, a],
            to: Address::with_last_byte(9),
            deadline: U256::from(1u8),
        };
        assert_eq!(
            &fee.abi_encode()[..4],
            [0x5c, 0x11, 0xd7, 0x95],
            "selector seen on chain"
        );
        let s = swap(UniswapV2.decode_call(ROUTERS[1].0, &fee.abi_encode()));
        assert_eq!(
            (s.pool, s.token_in, s.hops),
            (
                PoolRef::PairV2 {
                    token_a: b,
                    token_b: a
                },
                b,
                1
            )
        );
    }

    #[test]
    fn other_contracts_zero_amounts_and_short_paths_are_not_decoded() {
        let (router, _) = ROUTERS[0];
        let ok = V2Router::swapExactTokensForTokensCall {
            amountIn: U256::from(5u8),
            amountOutMin: U256::ZERO,
            path: vec![Address::with_last_byte(1), Address::with_last_byte(2)],
            to: Address::with_last_byte(9),
            deadline: U256::from(1u8),
        };
        assert!(UniswapV2.decode_call(router, &ok.abi_encode()).is_some());
        assert!(
            UniswapV2
                .decode_call(Address::with_last_byte(5), &ok.abi_encode())
                .is_none(),
            "not one of the routers"
        );
        let mut zero = ok.clone();
        zero.amountIn = U256::ZERO;
        assert!(UniswapV2.decode_call(router, &zero.abi_encode()).is_none());
        let mut short = ok.clone();
        short.path = vec![Address::with_last_byte(1)];
        assert!(UniswapV2.decode_call(router, &short.abi_encode()).is_none());
        let mut same = ok;
        same.path = vec![Address::with_last_byte(1), Address::with_last_byte(1)];
        assert!(UniswapV2.decode_call(router, &same.abi_encode()).is_none());
        assert!(
            UniswapV2
                .decode_call(router, &[0x38, 0xed, 0x17, 0x39])
                .is_none(),
            "no arguments"
        );
    }
}
