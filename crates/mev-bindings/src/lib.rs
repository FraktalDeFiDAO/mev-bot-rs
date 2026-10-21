//! Solidity bindings for our executor and the external contracts we read.
#![allow(clippy::too_many_arguments)] // sol!-generated constructors for wide events

use alloy::sol;

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256);
        function decimals() external view returns (uint8);
    }

    #[sol(rpc)]
    interface IUniswapV2Factory {
        function getPair(address tokenA, address tokenB) external view returns (address pair);
        /// Also emitted by Camelot V2 and Sushi V2 (same signature).
        event PairCreated(address indexed token0, address indexed token1, address pair, uint256 index);
    }

    #[sol(rpc)]
    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        event Sync(uint112 reserve0, uint112 reserve1);
    }

    /// Solidly-style factory (Aerodrome V2 `PoolFactory`). `getFee` is the pool's effective fee in basis points.
    #[sol(rpc)]
    interface ISolidlyFactory {
        function getPool(address tokenA, address tokenB, bool stable) external view returns (address pool);
        function getFee(address pool, bool stable) external view returns (uint256);
        event PoolCreated(address indexed token0, address indexed token1, bool indexed stable, address pool, uint256 index);
    }

    /// Solidly-style pool (Aerodrome V2): uint256 reserves, so its `Sync` and `Swap` topics differ from V2's.
    #[sol(rpc)]
    interface ISolidlyPool {
        function getReserves() external view returns (uint256 reserve0, uint256 reserve1, uint256 blockTimestampLast);
        event Sync(uint256 reserve0, uint256 reserve1);
        event Swap(address indexed sender, address indexed to, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out);
    }

    #[sol(rpc)]
    interface ICamelotPair {
        function stableSwap() external view returns (bool);
        function token0FeePercent() external view returns (uint16);
        function token1FeePercent() external view returns (uint16);
    }

    #[sol(rpc)]
    interface IUniswapV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
        /// Uniswap V3, Sushi V3 and PancakeSwap V3 factories.
        event PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickSpacing, address pool);
    }

    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        function liquidity() external view returns (uint128);
        function tickBitmap(int16 wordPosition) external view returns (uint256);
        function ticks(int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }

    #[sol(rpc)]
    interface IPancakeV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint32 feeProtocol, bool unlocked);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint128 protocolFeesToken0, uint128 protocolFeesToken1);
    }

    #[sol(rpc)]
    interface IAlgebraFactory {
        function poolByPair(address tokenA, address tokenB) external view returns (address pool);
        event Pool(address indexed token0, address indexed token1, address pool);
    }

    #[sol(rpc)]
    interface IAlgebraPool {
        /// Camelot's Algebra 1.9: directional fees in hundredths of a bip.
        function globalState() external view returns (uint160 price, int24 tick, uint16 feeZto, uint16 feeOtz, uint16 timepointIndex, uint8 communityFeeToken0, uint8 communityFeeToken1, bool unlocked);
        function dataStorageOperator() external view returns (address);
        /// 256-bit row of initialized ticks: bit `tick & 0xff` of row `tick >> 8` (no spacing compression).
        function tickTable(int16 row) external view returns (uint256);
        function ticks(int24 tick) external view returns (uint128 liquidityTotal, int128 liquidityDelta, uint256 outerFeeGrowth0Token, uint256 outerFeeGrowth1Token, int56 outerTickCumulative, uint160 outerSecondsPerLiquidity, uint32 outerSecondsSpent, bool initialized);
    }

    /// The pool's `DataStorageOperator`: per-direction adaptive-fee configuration (Camelot's verified source).
    #[sol(rpc)]
    interface IAlgebraOperator {
        function feeConfigZto() external view returns (uint16 alpha1, uint16 alpha2, uint32 beta1, uint32 beta2, uint16 gamma1, uint16 gamma2, uint32 volumeBeta, uint16 volumeGamma, uint16 baseFee);
        function feeConfigOtz() external view returns (uint16 alpha1, uint16 alpha2, uint32 beta1, uint32 beta2, uint16 gamma1, uint16 gamma2, uint32 volumeBeta, uint16 volumeGamma, uint16 baseFee);
    }

    /// Metadata every V2/V3-style pool exposes; used to verify auto-discovered pools.
    #[sol(rpc)]
    interface IPoolMeta {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function factory() external view returns (address);
    }

    #[sol(rpc)]
    interface IAavePool {
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor);
        function getReservesList() external view returns (address[] memory);
        /// `UserConfigurationMap`: bit 2·id = borrowing reserve `id`, bit 2·id+1 = its collateral.
        function getUserConfiguration(address user) external view returns (uint256);
        function getReserveAddressById(uint16 id) external view returns (address);
        event Borrow(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint8 interestRateMode, uint256 borrowRate, uint16 indexed referralCode);
    }

    #[sol(rpc)]
    interface IAaveDataProvider {
        function getUserReserveData(address asset, address user) external view returns (uint256 currentATokenBalance, uint256 currentStableDebt, uint256 currentVariableDebt, uint256 principalStableDebt, uint256 scaledVariableDebt, uint256 stableBorrowRate, uint256 liquidityRate, uint40 stableRateLastUpdated, bool usageAsCollateralEnabled);
        function getReserveConfigurationData(address asset) external view returns (uint256 decimals, uint256 ltv, uint256 liquidationThreshold, uint256 liquidationBonus, uint256 reserveFactor, bool usageAsCollateralEnabled, bool borrowingEnabled, bool stableBorrowRateEnabled, bool isActive, bool isFrozen);
        function getLiquidationProtocolFee(address asset) external view returns (uint256);
    }

    #[sol(rpc)]
    interface IAaveOracle {
        function getAssetPrice(address asset) external view returns (uint256);
        function getSourceOfAsset(address asset) external view returns (address);
    }

    /// Price sources seen on Arbitrum: Chainlink proxies (`aggregator()`), Aave CAPO adapters for
    /// stables (`ASSET_TO_USD_AGGREGATOR()`) and ratio assets (`BASE_TO_USD_AGGREGATOR()`).
    #[sol(rpc)]
    interface IPriceSource {
        function aggregator() external view returns (address);
        function ASSET_TO_USD_AGGREGATOR() external view returns (address);
        function BASE_TO_USD_AGGREGATOR() external view returns (address);
        // Aave price-cap adapters for liquid-staking tokens: the exchange-rate feed proxy.
        function RATIO_PROVIDER() external view returns (address);
        // MorphoChainlinkOracleV2
        function BASE_FEED_1() external view returns (address);
        function BASE_FEED_2() external view returns (address);
        function QUOTE_FEED_1() external view returns (address);
        function QUOTE_FEED_2() external view returns (address);
        // Any Chainlink-style feed: traced (access list) when no getter above resolves it.
        function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80);
        function latestAnswer() external view returns (int256);
    }

    /// Uniswap V4 PoolManager: storage reads and the events that move pool state.
    #[sol(rpc)]
    interface IPoolManager {
        function extsload(bytes32 slot) external view returns (bytes32 value);
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, uint24 fee, int24 tickSpacing, address hooks, uint160 sqrtPriceX96, int24 tick);
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
    }

    /// Uniswap V4 PositionManager: `poolKeys` maps the first 25 bytes of a pool id to its key (set
    /// when a position is first minted through it), so active pools' keys need no event history.
    #[sol(rpc)]
    interface IV4PositionManager {
        function poolKeys(bytes25 poolId) external view returns (address currency0, address currency1, uint24 fee, int24 tickSpacing, address hooks);
    }

    #[sol(rpc)]
    interface IMulticall3 {
        struct Call3 { address target; bool allowFailure; bytes callData; }
        struct Result { bool success; bytes returnData; }
        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
    }

    #[sol(rpc)]
    interface ArbExecutor {
        struct Hop { uint8 kind; address pool; address tokenIn; address tokenOut; bool zeroForOne; uint16 feeBps; }
        struct ProtocolCall { address target; bytes data; address approveSpender; uint256 approveAmount; }
        function execute(address token, uint256 amount, uint256 minProfit, Hop[] calldata hops) external returns (uint256 profit);
        function executeFlashSwap(uint256 amount, uint256 minProfit, Hop[] calldata hops) external returns (uint256 profit);
        function liquidate(address debt, address collateral, uint256 flashAmount, uint256 minProfit, ProtocolCall[] calldata calls, Hop[] calldata hops) external returns (uint256 profit);
        function liquidateViaAave(address aavePool, address debt, address collateral, uint256 flashAmount, uint256 minProfit, ProtocolCall[] calldata calls, Hop[] calldata hops) external returns (uint256 profit);
        function batch(bytes[] calldata calls) external returns (bool[] memory ok);
        event BatchItem(uint256 indexed index, bytes4 indexed selector, bool ok);
        event Profit(address indexed token, uint256 amount);
        function setTarget(address target, bool allowed) external;
        function allowedTarget(address target) external view returns (bool);
        error TargetNotAllowed(address target);
        function owner() external view returns (address);
        function profitReceiver() external view returns (address);
        function poolManager() external view returns (address);
        error NotOwner();
        error Unauthorized();
        error NotProfitable(uint256 balance, uint256 required);
        error BadRoute();
        error TransferFailed();
        error NothingExecuted();
        error BatchTooLarge();
    }
}

pub const MULTICALL3: alloy::primitives::Address =
    alloy::primitives::address!("cA11bde05977b3631167028862bE2a173976CA11");

pub fn to_sol_hop(h: &mev_types::Hop) -> ArbExecutor::Hop {
    // A V4 hop names its pool by tick spacing (the executor rebuilds the key); others by address.
    let pool = if matches!(h.kind, 3 | 4) {
        alloy::primitives::Address::from(alloy::primitives::aliases::U160::from(h.spacing))
    } else {
        h.pool
    };
    ArbExecutor::Hop {
        kind: h.kind,
        pool,
        tokenIn: h.token_in,
        tokenOut: h.token_out,
        zeroForOne: h.zero_for_one,
        feeBps: h.fee_bps,
    }
}

/// Executor hop kind shared by Uniswap V3 / Sushi V3 / Pancake V3 / Algebra pools.
pub const V3_HOP: u8 = 2;

/// Can this arbitrage run as a flash swap (`executeFlashSwap`): the first hop is a V3-style pool
/// starting from the loan token, and no later hop reuses that pool (it is locked during its own
/// callback). Such routes need no lender: no fee and no lender-balance cap.
pub fn flash_swappable(opp: &mev_types::Opportunity) -> bool {
    let Some(first) = opp.hops.first() else {
        return false;
    };
    opp.hops.len() >= 2
        && first.kind == V3_HOP
        && first.token_in == opp.loan_token
        && opp.hops[1..].iter().all(|h| h.pool != first.pool)
}

/// Executor calldata for an arbitrage: a flash swap when `flash_swaps` (the deployed executor
/// supports it) and the route allows, otherwise a Balancer flash loan (`execute`). Both return the
/// profit as `uint256`.
pub fn arb_calldata(
    opp: &mev_types::Opportunity,
    min_profit: alloy::primitives::U256,
    flash_swaps: bool,
) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    let hops = opp.hops.iter().map(to_sol_hop).collect();
    if flash_swaps && flash_swappable(opp) {
        ArbExecutor::executeFlashSwapCall {
            amount: opp.amount_in,
            minProfit: min_profit,
            hops,
        }
        .abi_encode()
    } else {
        ArbExecutor::executeCall {
            token: opp.loan_token,
            amount: opp.amount_in,
            minProfit: min_profit,
            hops,
        }
        .abi_encode()
    }
}

/// Storage slot of a V4 pool's `Pool.State` (`_pools` is the manager's storage slot 6).
pub fn v4_state_slot(id: alloy::primitives::B256) -> alloy::primitives::B256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(id.as_slice());
    buf[63] = 6;
    alloy::primitives::keccak256(buf)
}

/// `Pool.State` slot `base + n` (slot0 = 0, liquidity = 3).
pub fn v4_slot_plus(base: alloy::primitives::B256, n: u64) -> alloy::primitives::B256 {
    alloy::primitives::B256::from(
        alloy::primitives::U256::from_be_bytes(base.0) + alloy::primitives::U256::from(n),
    )
}

/// A V4 pool's `slot0` word unpacked: (sqrtPriceX96, tick, protocolFee, lpFee). `protocolFee` packs two
/// 12-bit fees: zeroForOne in the low bits, oneForZero in the high bits.
pub fn decode_v4_slot0(word: alloy::primitives::B256) -> (alloy::primitives::U256, i32, u32, u32) {
    let w = alloy::primitives::U256::from_be_bytes(word.0);
    let mask24 = alloy::primitives::U256::from(0xff_ffffu32);
    let sqrt =
        w & ((alloy::primitives::U256::from(1u8) << 160) - alloy::primitives::U256::from(1u8));
    let tick_raw = u32::try_from((w >> 160) & mask24).unwrap_or(0);
    let tick = ((tick_raw << 8) as i32) >> 8; // sign-extend 24 bits
    let protocol = u32::try_from((w >> 184) & mask24).unwrap_or(0);
    let lp = u32::try_from((w >> 208) & mask24).unwrap_or(0);
    (sqrt, tick, protocol, lp)
}

/// Total swap fee (hundredths of a bip) for one direction: protocol fee and LP fee compounded,
/// `p + l - p*l/1e6` (the manager's `calculateSwapFee`).
pub fn v4_swap_fee(protocol_fee: u32, lp_fee: u32) -> u32 {
    protocol_fee + lp_fee - ((protocol_fee as u64 * lp_fee as u64) / 1_000_000) as u32
}

/// Most trades one executor `batch` call carries (`ArbExecutor.MAX_BATCH`).
pub const MAX_BATCH: usize = 8;

/// Executor calldata running `items` (each ready-made `execute` / `executeFlashSwap` /
/// `liquidate` / `liquidateViaAave` calldata) in one transaction via `batch`.
pub fn batch_calldata(items: Vec<Vec<u8>>) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    ArbExecutor::batchCall {
        calls: items.into_iter().map(Into::into).collect(),
    }
    .abi_encode()
}

/// Incremental executor proceeds, emitted only after repayment and starting-balance preservation.
pub fn profits(
    logs: &[alloy::rpc::types::Log],
    executor: alloy::primitives::Address,
) -> Vec<(alloy::primitives::Address, alloy::primitives::U256)> {
    use alloy::sol_types::SolEvent;
    logs.iter()
        .filter(|l| l.address() == executor)
        .filter_map(|l| ArbExecutor::Profit::decode_log(&l.inner).ok())
        .map(|e| (e.token, e.amount))
        .collect()
}

/// Per-item results of a `batch` transaction, in item order, from the executor's `BatchItem`
/// events. Empty when the receipt has none (not a batch).
pub fn batch_outcomes(
    logs: &[alloy::rpc::types::Log],
    executor: alloy::primitives::Address,
) -> Vec<bool> {
    use alloy::sol_types::SolEvent;
    let mut out: Vec<(usize, bool)> = logs
        .iter()
        .filter(|l| l.address() == executor)
        .filter_map(|l| ArbExecutor::BatchItem::decode_log(&l.inner).ok())
        .map(|e| (e.index.try_into().unwrap_or(usize::MAX), e.ok))
        .collect();
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, ok)| ok).collect()
}

/// The exact liquidation calldata together with its keccak hash. Built ONCE with the final
/// (floored) `min_profit` so the bytes that are simulated and the bytes that are signed are the
/// same object, and the hash can be compared (JEV-LIQ-015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalCalldata {
    pub bytes: Vec<u8>,
    pub hash: alloy::primitives::B256,
}

pub fn final_liquidation_calldata(
    plan: &mev_types::Plan,
    hops: &[mev_types::Hop],
    min_profit: alloy::primitives::U256,
    aave_pool: Option<alloy::primitives::Address>,
) -> FinalCalldata {
    let bytes = liquidation_calldata(plan, hops, min_profit, aave_pool);
    let hash = alloy::primitives::keccak256(&bytes);
    FinalCalldata { bytes, hash }
}

/// Executor calldata for a liquidation: funded by Balancer (`liquidate`), or by an Aave V3 flash
/// loan from `aave_pool` (`liquidateViaAave`) when the loan exceeds what Balancer holds. Both
/// return the profit as `uint256`.
pub fn liquidation_calldata(
    plan: &mev_types::Plan,
    hops: &[mev_types::Hop],
    min_profit: alloy::primitives::U256,
    aave_pool: Option<alloy::primitives::Address>,
) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    let calls = plan.calls.iter().map(to_sol_call).collect();
    let hops = hops.iter().map(to_sol_hop).collect();
    match aave_pool {
        Some(pool) => ArbExecutor::liquidateViaAaveCall {
            aavePool: pool,
            debt: plan.debt_asset,
            collateral: plan.collateral_asset,
            flashAmount: plan.flash_amount,
            minProfit: min_profit,
            calls,
            hops,
        }
        .abi_encode(),
        None => ArbExecutor::liquidateCall {
            debt: plan.debt_asset,
            collateral: plan.collateral_asset,
            flashAmount: plan.flash_amount,
            minProfit: min_profit,
            calls,
            hops,
        }
        .abi_encode(),
    }
}

pub fn to_sol_call(c: &mev_types::ProtocolCall) -> ArbExecutor::ProtocolCall {
    ArbExecutor::ProtocolCall {
        target: c.target,
        data: c.data.clone(),
        approveSpender: c.approve_spender,
        approveAmount: c.approve_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{Address, U256},
        sol_types::SolCall,
    };

    #[test]
    fn hop_and_call_conversions_preserve_fields() {
        let h = mev_types::Hop {
            kind: 2,
            pool: Address::with_last_byte(1),
            token_in: Address::with_last_byte(2),
            token_out: Address::with_last_byte(3),
            zero_for_one: true,
            fee_bps: 30,
            spacing: 0,
        };
        let s = to_sol_hop(&h);
        assert_eq!(
            (
                s.kind,
                s.pool,
                s.tokenIn,
                s.tokenOut,
                s.zeroForOne,
                s.feeBps
            ),
            (2, h.pool, h.token_in, h.token_out, true, 30)
        );
        let c = mev_types::ProtocolCall {
            target: Address::with_last_byte(9),
            data: vec![1, 2, 3].into(),
            approve_spender: Address::with_last_byte(8),
            approve_amount: U256::from(77),
        };
        let sc = to_sol_call(&c);
        assert_eq!(
            (
                sc.target,
                sc.data.to_vec(),
                sc.approveSpender,
                sc.approveAmount
            ),
            (c.target, vec![1, 2, 3], c.approve_spender, c.approve_amount)
        );
    }

    #[test]
    fn liquidate_calldata_round_trips_through_the_abi() {
        let call = ArbExecutor::liquidateCall {
            debt: Address::with_last_byte(1),
            collateral: Address::with_last_byte(2),
            flashAmount: U256::from(1_000),
            minProfit: U256::from(5),
            calls: vec![],
            hops: vec![],
        };
        let decoded = ArbExecutor::liquidateCall::abi_decode(&call.abi_encode()).unwrap();
        assert_eq!(decoded.debt, call.debt);
        assert_eq!(decoded.flashAmount, call.flashAmount);
    }

    /// A 32-byte word from hex given in chunks (public on-chain test vectors).
    fn word(parts: &[&str]) -> alloy::primitives::B256 {
        format!("0x{}", parts.concat()).parse().unwrap()
    }

    #[test]
    fn v4_slot0_decodes_the_live_eth_usdc_pool_word() {
        // Read from the Arbitrum PoolManager for the ETH/USDC 0.05 % pool (2026-10-06).
        let slot0 = word(&[
            "0000000001f407d0",
            "7dfcfd2a00000000",
            "00000000000366c3",
            "dc93f45fa0004f31",
        ]);
        let (sqrt, tick, protocol, lp) = decode_v4_slot0(slot0);
        assert_eq!(sqrt.to_string(), "4112071849507166162538289");
        assert_eq!(tick, -197334);
        assert_eq!((protocol & 0xfff, protocol >> 12, lp), (125, 125, 500));
        // Protocol fee compounds with the LP fee: 125 + 500 - 125*500/1e6 (rounds down to 0).
        assert_eq!(v4_swap_fee(125, 500), 625);
        assert_eq!(v4_swap_fee(1000, 3000), 3997);
    }

    #[test]
    fn v4_state_slot_matches_the_manager_layout() {
        let id = word(&[
            "864abca0a6202dba",
            "5b8868772308da95",
            "3ff125b0f95015ad",
            "bf89aaf579e903a8",
        ]);
        // keccak(id ++ 6): the slot read on-chain for this pool (ETH/USDC 0.05 %) above.
        let slot = v4_state_slot(id);
        let expected = |last: &'static str| {
            word(&[
                "07987b1980b45d4b",
                "fabc174b7412cf7d",
                "817b6209bcd40ae6",
                last,
            ])
        };
        assert_eq!(slot, expected("0ee33776fe7c1e3a"));
        assert_eq!(v4_slot_plus(slot, 3), expected("0ee33776fe7c1e3d"));
    }

    #[test]
    fn a_v4_hop_names_its_pool_by_tick_spacing() {
        let h = mev_types::Hop {
            kind: 4,
            pool: Address::with_last_byte(0x99), // stand-in id address: never sent
            token_in: Address::with_last_byte(2),
            token_out: Address::with_last_byte(3),
            zero_for_one: true,
            fee_bps: 500,
            spacing: 10,
        };
        assert_eq!(to_sol_hop(&h).pool, Address::with_last_byte(10));
        let v3 = mev_types::Hop {
            kind: 2,
            spacing: 0,
            ..h
        };
        assert_eq!(to_sol_hop(&v3).pool, Address::with_last_byte(0x99));
    }

    #[test]
    fn batch_calldata_carries_each_item_unchanged_and_in_order() {
        let a = ArbExecutor::liquidateCall {
            debt: Address::with_last_byte(1),
            collateral: Address::with_last_byte(2),
            flashAmount: U256::from(1_000),
            minProfit: U256::from(5),
            calls: vec![],
            hops: vec![],
        }
        .abi_encode();
        let b = ArbExecutor::executeCall {
            token: Address::with_last_byte(3),
            amount: U256::from(9),
            minProfit: U256::ZERO,
            hops: vec![],
        }
        .abi_encode();
        let encoded = batch_calldata(vec![a.clone(), b.clone()]);
        let decoded = ArbExecutor::batchCall::abi_decode(&encoded).unwrap();
        let items: Vec<Vec<u8>> = decoded.calls.iter().map(|c| c.to_vec()).collect();
        assert_eq!(items, vec![a, b]);
        assert_eq!(&encoded[..4], ArbExecutor::batchCall::SELECTOR);
    }

    #[test]
    fn batch_outcomes_are_read_from_the_executors_events_in_item_order() {
        use alloy::{
            primitives::{B256, FixedBytes, Log as PrimLog, LogData},
            rpc::types::Log,
            sol_types::SolEvent,
        };
        let exec = Address::with_last_byte(0xee);
        let mk = |addr: Address, idx: u64, ok: bool| {
            let ev = ArbExecutor::BatchItem {
                index: U256::from(idx),
                selector: FixedBytes::from([1, 2, 3, 4]),
                ok,
            };
            Log {
                inner: PrimLog {
                    address: addr,
                    data: ev.encode_log_data(),
                },
                ..Default::default()
            }
        };
        // Out of order, plus a lookalike event from another contract and an unrelated log.
        let other = Log {
            inner: PrimLog {
                address: exec,
                data: LogData::new_unchecked(vec![B256::ZERO], Default::default()),
            },
            ..Default::default()
        };
        let logs = vec![
            mk(exec, 1, false),
            mk(Address::with_last_byte(1), 0, false),
            other,
            mk(exec, 0, true),
        ];
        assert_eq!(batch_outcomes(&logs, exec), vec![true, false]);
        assert!(batch_outcomes(&[], exec).is_empty());
    }
}
