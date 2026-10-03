//! Solidity bindings for our executor and the external contracts we read.

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
    interface IMulticall3 {
        struct Call3 { address target; bool allowFailure; bytes callData; }
        struct Result { bool success; bytes returnData; }
        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
    }

    #[sol(rpc)]
    interface ArbExecutor {
        struct Hop { uint8 kind; address pool; address tokenIn; address tokenOut; bool zeroForOne; uint16 feeBps; }
        function execute(address token, uint256 amount, uint256 minProfit, Hop[] calldata hops) external returns (uint256 profit);
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
