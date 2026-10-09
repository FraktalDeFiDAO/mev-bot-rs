//! Every call inside a transaction, through the contracts that only forward calls to others.
//!
//! A feed transaction is often not a swap but a wrapper around several: a router `multicall`, Multicall3
//! `aggregate3`, a smart account's `executeBatch`, an ERC-4337 `handleOps` bundle. [`calls_in`] unwraps those
//! recursively and returns every leaf call; [`decode_all`] decodes each leaf with the protocol crates, so a bundle with
//! three swaps and a liquidity change gives four actions, not one.
//!
//! Wrappers read: Multicall3 (`aggregate`, `tryAggregate`, `tryBlockAndAggregate`, `blockAndAggregate`, `aggregate3`,
//! `aggregate3Value`); the self-multicalls of the Uniswap routers and position managers (`multicall(bytes[])`,
//! `multicall(uint256,bytes[])`, `multicall(bytes32,bytes[])`, which call the same contract); smart accounts'
//! `execute(address,uint256,bytes)` and `executeBatch` (both shapes); ERC-4337 EntryPoint v0.7 `handleOps`, whose
//! operations' `callData` is unwrapped the same way. Anything else is a leaf.
//!
//! Not read: the ETH `value` of a call (so ETH-in swaps through a wrapper keep the router's own accounting), EntryPoint
//! v0.6 `handleOps`, and wrappers that use delegatecall to a chosen implementation.

use alloy::{
    primitives::{Address, Bytes, address},
    sol,
    sol_types::SolCall,
};

use crate::{Decoded, decode_calls};

sol! {
    interface Multicall3 {
        struct Call { address target; bytes callData; }
        struct Call3 { address target; bool allowFailure; bytes callData; }
        struct Call3Value { address target; bool allowFailure; uint256 value; bytes callData; }
        function aggregate(Call[] calls) external payable returns (uint256 blockNumber, bytes[] returnData);
        function blockAndAggregate(Call[] calls) external payable returns (uint256 blockNumber, bytes32 blockHash, bytes[] returnData);
        function tryAggregate(bool requireSuccess, Call[] calls) external payable returns (bytes[] returnData);
        function tryBlockAndAggregate(bool requireSuccess, Call[] calls) external payable returns (uint256 blockNumber, bytes32 blockHash, bytes[] returnData);
        function aggregate3(Call3[] calls) external payable returns (bytes[] returnData);
        function aggregate3Value(Call3Value[] calls) external payable returns (bytes[] returnData);
    }
    interface SelfMulticall {
        function multicall(bytes[] data) external payable returns (bytes[] results);
        function multicallDeadline(uint256 deadline, bytes[] data) external payable returns (bytes[] results);
        function multicallBlockhash(bytes32 previousBlockhash, bytes[] data) external payable returns (bytes[] results);
    }
    interface SmartAccount {
        function execute(address dest, uint256 value, bytes func) external;
        function executeBatch(address[] dest, bytes[] func) external;
        function executeBatchValue(address[] dest, uint256[] value, bytes[] func) external;
    }
    interface EntryPointV7 {
        struct PackedUserOperation {
            address sender; uint256 nonce; bytes initCode; bytes callData; bytes32 accountGasLimits;
            uint256 preVerificationGas; bytes32 gasFees; bytes paymasterAndData; bytes signature;
        }
        function handleOps(PackedUserOperation[] ops, address beneficiary) external;
    }
}

/// How deep wrappers are followed, and how many leaves one transaction may yield (a hostile bundle must not blow up).
const MAX_DEPTH: usize = 6;
const MAX_LEAVES: usize = 512;

/// The Multicall3 deployment, the same address on every chain.
pub const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");
/// ERC-4337 EntryPoint v0.7.
pub const ENTRYPOINT_V7: Address = address!("0000000071727De22E5E9d8BAf0edAc6f37da032");

/// One call found in a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub to: Address,
    pub input: Bytes,
    /// The wrappers it was found through, outermost first (`"multicall3"`, `"self_multicall"`, `"account_execute"`,
    /// `"entrypoint_v7"`). Empty for a call made directly by the transaction.
    pub via: Vec<&'static str>,
}

impl Leaf {
    pub fn selector(&self) -> Option<[u8; 4]> {
        self.input.get(..4).map(|s| [s[0], s[1], s[2], s[3]])
    }
}

/// A leaf and what the protocol crates make of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub leaf: Leaf,
    pub decoded: Option<Decoded>,
}

/// Every call in the transaction `to` with `input`, wrappers unwrapped, in execution order.
pub fn calls_in(to: Address, input: &[u8]) -> Vec<Leaf> {
    let mut out = Vec::new();
    walk(to, input, &mut Vec::new(), &mut out);
    out
}

/// Every call in the transaction and its decoding (or `None` where no protocol crate reads it).
pub fn actions_in(to: Address, input: &[u8]) -> Vec<Action> {
    calls_in(to, input)
        .into_iter()
        .flat_map(|leaf| {
            // One action per intent in the leaf (a Universal Router `execute` can hold several swaps); a leaf nothing
            // reads is a single action with no decoding.
            let intents = decode_calls(leaf.to, &leaf.input);
            if intents.is_empty() {
                vec![Action {
                    leaf,
                    decoded: None,
                }]
            } else {
                intents
                    .into_iter()
                    .map(|d| Action {
                        leaf: leaf.clone(),
                        decoded: Some(d),
                    })
                    .collect()
            }
        })
        .collect()
}

/// Every swap, liquidity change and liquidation the transaction intends, including those inside wrappers.
pub fn decode_all(to: Address, input: &[u8]) -> Vec<Decoded> {
    actions_in(to, input)
        .into_iter()
        .filter_map(|a| a.decoded)
        .collect()
}

fn walk(to: Address, input: &[u8], via: &mut Vec<&'static str>, out: &mut Vec<Leaf>) {
    if out.len() >= MAX_LEAVES {
        return;
    }
    if via.len() < MAX_DEPTH
        && let Some((kind, subs)) = unwrap(to, input)
    {
        via.push(kind);
        for (t, data) in subs {
            walk(t, &data, via, out);
        }
        via.pop();
        return;
    }
    out.push(Leaf {
        to,
        input: Bytes::copy_from_slice(input),
        via: via.clone(),
    });
}

/// A call a wrapper makes: its target and calldata.
type Sub = (Address, Vec<u8>);

/// The calls a wrapper makes, if `input` is a call to one. `None` for a plain call or a malformed wrapper (a
/// malformed wrapper is treated as a leaf, so nothing is invented).
fn unwrap(to: Address, input: &[u8]) -> Option<(&'static str, Vec<Sub>)> {
    let sel: [u8; 4] = input.get(..4)?.try_into().ok()?;
    let args = &input[4..];
    let pair = |t: Address, d: Bytes| (t, d.to_vec());
    macro_rules! dec {
        ($c:path) => {
            <$c as SolCall>::abi_decode_raw(args).ok()?
        };
    }
    if to == MULTICALL3 {
        let calls = match sel {
            Multicall3::aggregateCall::SELECTOR => dec!(Multicall3::aggregateCall)
                .calls
                .into_iter()
                .map(|c| pair(c.target, c.callData))
                .collect(),
            Multicall3::blockAndAggregateCall::SELECTOR => dec!(Multicall3::blockAndAggregateCall)
                .calls
                .into_iter()
                .map(|c| pair(c.target, c.callData))
                .collect(),
            Multicall3::tryAggregateCall::SELECTOR => dec!(Multicall3::tryAggregateCall)
                .calls
                .into_iter()
                .map(|c| pair(c.target, c.callData))
                .collect(),
            Multicall3::tryBlockAndAggregateCall::SELECTOR => {
                dec!(Multicall3::tryBlockAndAggregateCall)
                    .calls
                    .into_iter()
                    .map(|c| pair(c.target, c.callData))
                    .collect()
            }
            Multicall3::aggregate3Call::SELECTOR => dec!(Multicall3::aggregate3Call)
                .calls
                .into_iter()
                .map(|c| pair(c.target, c.callData))
                .collect(),
            Multicall3::aggregate3ValueCall::SELECTOR => dec!(Multicall3::aggregate3ValueCall)
                .calls
                .into_iter()
                .map(|c| pair(c.target, c.callData))
                .collect(),
            _ => return None,
        };
        return Some(("multicall3", calls));
    }
    if to == ENTRYPOINT_V7 && sel == EntryPointV7::handleOpsCall::SELECTOR {
        let ops = dec!(EntryPointV7::handleOpsCall).ops;
        // Each operation is a call from its sender (the smart account) with `callData`.
        return Some((
            "entrypoint_v7",
            ops.into_iter()
                .map(|o| pair(o.sender, o.callData))
                .collect(),
        ));
    }
    // Self-multicalls: the router or position manager calls itself with each element.
    let batch = match sel {
        SelfMulticall::multicallCall::SELECTOR => Some(dec!(SelfMulticall::multicallCall).data),
        SelfMulticall::multicallDeadlineCall::SELECTOR => {
            Some(dec!(SelfMulticall::multicallDeadlineCall).data)
        }
        SelfMulticall::multicallBlockhashCall::SELECTOR => {
            Some(dec!(SelfMulticall::multicallBlockhashCall).data)
        }
        _ => None,
    };
    if let Some(data) = batch {
        return Some((
            "self_multicall",
            data.into_iter().map(|d| pair(to, d)).collect(),
        ));
    }
    // Smart accounts: each element is a call to another contract.
    let account: Option<Vec<Sub>> = match sel {
        SmartAccount::executeCall::SELECTOR => {
            let c = dec!(SmartAccount::executeCall);
            Some(vec![pair(c.dest, c.func)])
        }
        SmartAccount::executeBatchCall::SELECTOR => {
            let c = dec!(SmartAccount::executeBatchCall);
            (c.dest.len() == c.func.len()).then(|| {
                c.dest
                    .into_iter()
                    .zip(c.func)
                    .map(|(t, d)| pair(t, d))
                    .collect()
            })
        }
        SmartAccount::executeBatchValueCall::SELECTOR => {
            let c = dec!(SmartAccount::executeBatchValueCall);
            (c.dest.len() == c.func.len()).then(|| {
                c.dest
                    .into_iter()
                    .zip(c.func)
                    .map(|(t, d)| pair(t, d))
                    .collect()
            })
        }
        _ => None,
    };
    account.map(|a| ("account_execute", a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn target(n: u8) -> Address {
        Address::with_last_byte(n)
    }

    fn aggregate3(calls: Vec<(Address, Vec<u8>)>) -> Vec<u8> {
        Multicall3::aggregate3Call {
            calls: calls
                .into_iter()
                .map(|(t, d)| Multicall3::Call3 {
                    target: t,
                    allowFailure: true,
                    callData: d.into(),
                })
                .collect(),
        }
        .abi_encode()
    }

    fn self_multicall(data: Vec<Vec<u8>>) -> Vec<u8> {
        SelfMulticall::multicallCall {
            data: data.into_iter().map(Bytes::from).collect(),
        }
        .abi_encode()
    }

    #[test]
    fn a_plain_call_is_its_own_single_leaf() {
        let l = calls_in(target(1), &[0xde, 0xad, 0xbe, 0xef, 1, 2, 3]);
        assert_eq!(l.len(), 1);
        assert!(l[0].via.is_empty());
        assert_eq!(l[0].selector(), Some([0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn multicall3_aggregate3_yields_every_inner_call_in_order() {
        let input = aggregate3(vec![
            (target(1), vec![1, 1, 1, 1]),
            (target(2), vec![2, 2, 2, 2, 9]),
        ]);
        let l = calls_in(MULTICALL3, &input);
        assert_eq!(
            l.iter().map(|x| x.to).collect::<Vec<_>>(),
            vec![target(1), target(2)]
        );
        assert_eq!(l[1].input.as_ref(), &[2, 2, 2, 2, 9]);
        assert!(l.iter().all(|x| x.via == vec!["multicall3"]));
    }

    #[test]
    fn a_router_multicall_calls_the_same_contract_with_each_element() {
        let input = self_multicall(vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]]);
        let l = calls_in(target(7), &input);
        assert_eq!(l.len(), 2);
        assert!(
            l.iter()
                .all(|x| x.to == target(7) && x.via == vec!["self_multicall"])
        );
    }

    #[test]
    fn wrappers_nest_multicall3_around_a_router_multicall_around_an_account_execute() {
        let inner = SmartAccount::executeCall {
            dest: target(9),
            value: U256::ZERO,
            func: vec![9, 9, 9, 9].into(),
        }
        .abi_encode();
        let router = self_multicall(vec![vec![1, 1, 1, 1], inner]);
        let outer = aggregate3(vec![(target(7), router)]);
        let l = calls_in(MULTICALL3, &outer);
        assert_eq!(l.len(), 2);
        assert_eq!(
            (l[0].to, l[0].via.clone()),
            (target(7), vec!["multicall3", "self_multicall"])
        );
        assert_eq!(
            (l[1].to, l[1].via.clone(), l[1].input.to_vec()),
            (
                target(9),
                vec!["multicall3", "self_multicall", "account_execute"],
                vec![9, 9, 9, 9]
            )
        );
    }

    #[test]
    fn an_entrypoint_bundle_yields_each_operations_calls_from_its_account() {
        let batch = SmartAccount::executeBatchCall {
            dest: vec![target(5), target(6)],
            func: vec![vec![5, 5, 5, 5].into(), vec![6, 6, 6, 6].into()],
        }
        .abi_encode();
        let op = |sender: u8, call: Vec<u8>| EntryPointV7::PackedUserOperation {
            sender: target(sender),
            nonce: U256::ZERO,
            initCode: Bytes::new(),
            callData: call.into(),
            accountGasLimits: Default::default(),
            preVerificationGas: U256::ZERO,
            gasFees: Default::default(),
            paymasterAndData: Bytes::new(),
            signature: Bytes::new(),
        };
        let bundle = EntryPointV7::handleOpsCall {
            ops: vec![op(0xa1, batch), op(0xa2, vec![3, 3, 3, 3])],
            beneficiary: target(0xbe),
        }
        .abi_encode();
        let l = calls_in(ENTRYPOINT_V7, &bundle);
        assert_eq!(
            l.iter().map(|x| x.to).collect::<Vec<_>>(),
            vec![target(5), target(6), target(0xa2)]
        );
        assert_eq!(l[0].via, vec!["entrypoint_v7", "account_execute"]);
        assert_eq!(
            l[2].via,
            vec!["entrypoint_v7"],
            "an op whose callData is not a known account call is a leaf at the account"
        );
    }

    #[test]
    fn a_malformed_wrapper_is_a_leaf_and_nothing_is_invented() {
        let mut bad = Multicall3::aggregate3Call::SELECTOR.to_vec();
        bad.extend([0xff; 40]);
        let l = calls_in(MULTICALL3, &bad);
        assert_eq!(l.len(), 1);
        assert!(l[0].via.is_empty());
        // Multicall3 selectors only count at the Multicall3 address.
        let ok = aggregate3(vec![(target(1), vec![1, 2, 3, 4])]);
        assert_eq!(calls_in(target(3), &ok).len(), 1);
    }

    #[test]
    fn recursion_and_size_are_bounded() {
        // A multicall containing itself, 10 levels deep, stops at the depth limit instead of recursing forever.
        let mut data = vec![1, 2, 3, 4];
        for _ in 0..10 {
            data = self_multicall(vec![data]);
        }
        let l = calls_in(target(7), &data);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].via.len(), MAX_DEPTH);
        // A wide one is cut at MAX_LEAVES.
        let wide = self_multicall((0..MAX_LEAVES + 100).map(|_| vec![1, 2, 3, 4]).collect());
        assert_eq!(calls_in(target(7), &wide).len(), MAX_LEAVES);
    }

    #[test]
    fn decode_all_finds_every_swap_inside_nested_wrappers_and_ignores_the_rest() {
        use mev_protocol_uniswap_router::SWAP_ROUTER_02;
        alloy::sol! {
            struct Params { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
            function exactInputSingle(Params params) external payable returns (uint256 amountOut);
        }
        let swap = |amount: u64| {
            exactInputSingleCall {
                params: Params {
                    tokenIn: target(0xa1),
                    tokenOut: target(0xb2),
                    fee: alloy::primitives::Uint::<24, 1>::from(500u32),
                    recipient: target(0xee),
                    amountIn: U256::from(amount),
                    amountOutMinimum: U256::ZERO,
                    sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
                },
            }
            .abi_encode()
        };
        // One Multicall3 call to SwapRouter02's own multicall, holding two swaps and a call nothing reads.
        let router = self_multicall(vec![swap(1_000), vec![0, 0, 0, 0], swap(2_000)]);
        let outer = aggregate3(vec![
            (SWAP_ROUTER_02, router),
            (target(0x13), vec![9, 9, 9, 9]),
        ]);
        let acts = actions_in(MULTICALL3, &outer);
        assert_eq!(
            acts.len(),
            4,
            "three inside the router multicall, one beside it"
        );
        let amounts: Vec<_> = decode_all(MULTICALL3, &outer)
            .into_iter()
            .map(|d| match d {
                Decoded::Swap(s) => s.amount_in,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            amounts,
            vec![U256::from(1_000u64), U256::from(2_000u64)],
            "both swaps, in order, nothing invented"
        );
    }
}
