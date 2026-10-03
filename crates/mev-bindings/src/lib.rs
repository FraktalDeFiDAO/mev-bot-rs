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
        // MorphoChainlinkOracleV2
        function BASE_FEED_1() external view returns (address);
        function BASE_FEED_2() external view returns (address);
        function QUOTE_FEED_1() external view returns (address);
        function QUOTE_FEED_2() external view returns (address);
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
        function liquidate(address debt, address collateral, uint256 flashAmount, uint256 minProfit, ProtocolCall[] calldata calls, Hop[] calldata hops) external returns (uint256 profit);
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

pub fn to_sol_call(c: &mev_lending::ProtocolCall) -> ArbExecutor::ProtocolCall {
    ArbExecutor::ProtocolCall {
        target: c.target,
        data: c.data.clone(),
        approveSpender: c.approve_spender,
        approveAmount: c.approve_amount,
    }
}
