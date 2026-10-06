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
    }

    #[sol(rpc)]
    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        event Sync(uint112 reserve0, uint112 reserve1);
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
    }

    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        function liquidity() external view returns (uint128);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
    }

    #[sol(rpc)]
    interface IPancakeV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint32 feeProtocol, bool unlocked);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint128 protocolFeesToken0, uint128 protocolFeesToken1);
    }

    #[sol(rpc)]
    interface IAlgebraFactory {
        function poolByPair(address tokenA, address tokenB) external view returns (address pool);
    }

    #[sol(rpc)]
    interface IAlgebraPool {
        /// Camelot's Algebra 1.9: directional fees in hundredths of a bip.
        function globalState() external view returns (uint160 price, int24 tick, uint16 feeZto, uint16 feeOtz, uint16 timepointIndex, uint8 communityFeeToken0, uint8 communityFeeToken1, bool unlocked);
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
        function setTarget(address target, bool allowed) external;
        function allowedTarget(address target) external view returns (bool);
        error TargetNotAllowed(address target);
        function owner() external view returns (address);
        function profitReceiver() external view returns (address);
        error NotOwner();
        error Unauthorized();
        error NotProfitable(uint256 balance, uint256 required);
        error BadRoute();
        error TransferFailed();
    }
}

pub const MULTICALL3: alloy::primitives::Address =
    alloy::primitives::address!("cA11bde05977b3631167028862bE2a173976CA11");

pub fn to_sol_hop(h: &mev_types::Hop) -> ArbExecutor::Hop {
    ArbExecutor::Hop {
        kind: h.kind,
        pool: h.pool,
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

/// Executor calldata for a liquidation: funded by Balancer (`liquidate`), or by an Aave V3 flash
/// loan from `aave_pool` (`liquidateViaAave`) when the loan exceeds what Balancer holds. Both
/// return the profit as `uint256`.
pub fn liquidation_calldata(
    plan: &mev_lending::Plan,
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

pub fn to_sol_call(c: &mev_lending::ProtocolCall) -> ArbExecutor::ProtocolCall {
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
        let c = mev_lending::ProtocolCall {
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
}
