// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

interface IERC20Full {
    function balanceOf(address) external view returns (uint256);
    function transfer(address, uint256) external returns (bool);
}

interface IV2Factory {
    function getPair(address, address) external view returns (address);
}

interface IV3Factory {
    function getPool(address, address, uint24) external view returns (address);
}

interface IAlgebraFactory {
    function poolByPair(address, address) external view returns (address);
}

interface IV2Pair {
    function token0() external view returns (address);
    function getReserves() external view returns (uint112, uint112, uint32);
    function swap(uint256, uint256, address, bytes calldata) external;
}

contract ArbExecutorForkTest is Test {
    address constant VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address constant UNI_V3_FACTORY = 0x1F98431c8aD98523631AE4a59f267346ea31F984;
    address constant SUSHI_V2_FACTORY = 0xc35DADB65012eC5796536bD9864eD8773aBc74C4;
    address constant PANCAKE_V3_FACTORY = 0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865;
    address constant CAMELOT_V3_FACTORY = 0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B;

    address owner = makeAddr("owner");
    address receiver = makeAddr("receiver");
    address attacker = makeAddr("attacker");

    ArbExecutor exec;
    address v3Pool;
    address sushiPair;

    function setUp() public {
        vm.createSelectFork(vm.envOr("ARBITRUM_HTTP_URL", string("https://arb1.arbitrum.io/rpc")));
        vm.prank(owner);
        exec = new ArbExecutor(VAULT, receiver);
        v3Pool = IV3Factory(UNI_V3_FACTORY).getPool(WETH, USDC, 500);
        sushiPair = IV2Factory(SUSHI_V2_FACTORY).getPair(WETH, USDC);
        assertTrue(v3Pool != address(0) && sushiPair != address(0), "pools not found");
    }

    function _route() internal view returns (ArbExecutor.Hop[] memory hops) {
        // Borrow USDC -> buy WETH on Sushi V2 -> sell WETH on Uniswap V3 0.05%
        bool usdcIsToken0OnSushi = IV2Pair(sushiPair).token0() == USDC;
        bool wethIsToken0OnV3 = WETH < USDC;
        hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(0, sushiPair, USDC, WETH, usdcIsToken0OnSushi, 30);
        hops[1] = ArbExecutor.Hop(2, v3Pool, WETH, USDC, wethIsToken0OnV3, 0);
    }

    /// Dump WETH into the Sushi pair so WETH becomes cheap there — creates a real arb vs Uniswap V3.
    function _createArb(uint256 wethIn) internal {
        deal(WETH, address(this), wethIn);
        bool wethIs0 = IV2Pair(sushiPair).token0() == WETH;
        (uint112 r0, uint112 r1,) = IV2Pair(sushiPair).getReserves();
        (uint256 rIn, uint256 rOut) = wethIs0 ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
        uint256 out = (wethIn * 997 * rOut) / (rIn * 1000 + wethIn * 997);
        IERC20Full(WETH).transfer(sushiPair, wethIn);
        IV2Pair(sushiPair).swap(wethIs0 ? 0 : out, wethIs0 ? out : 0, address(this), "");
    }

    function test_onlyOwnerCanExecute() public {
        ArbExecutor.Hop[] memory hops = _route();
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.execute(USDC, 1e6, 0, hops);
    }

    function test_flashCallbackRejectsDirectCalls() public {
        address[] memory t = new address[](1);
        uint256[] memory a = new uint256[](1);
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.receiveFlashLoan(t, a, a, "");

        // Even the vault can't call it unless a trade we started is in progress.
        vm.prank(VAULT);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.receiveFlashLoan(t, a, a, "");
    }

    function test_v3CallbackRejectsUnexpectedCaller() public {
        vm.prank(v3Pool); // a real pool, but not during our swap
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.uniswapV3SwapCallback(1, 0, "");
    }

    function test_unprofitableRouteReverts() public {
        ArbExecutor.Hop[] memory hops = _route();
        vm.prank(owner);
        vm.expectRevert(); // NotProfitable: round trip with no price gap loses fees
        exec.execute(USDC, 1_000e6, 0, hops);
    }

    function test_badRouteReverts() public {
        ArbExecutor.Hop[] memory hops = _route();
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.execute(WETH, 1e18, 0, hops); // route starts with USDC, loan is WETH
    }

    function test_capturesRealArbAndHoldsNothing() public {
        _createArb(30 ether);
        uint256 before = IERC20Full(USDC).balanceOf(receiver);
        ArbExecutor.Hop[] memory hops = _route();

        vm.prank(owner);
        uint256 profit = exec.execute(USDC, 5_000e6, 1e6, hops);

        assertGt(profit, 1e6, "expected >1 USDC profit");
        assertEq(IERC20Full(USDC).balanceOf(receiver) - before, profit, "profit to receiver");
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
        emit log_named_decimal_uint("profit USDC", profit, 6);
    }

    function test_minProfitEnforced() public {
        _createArb(30 ether);
        ArbExecutor.Hop[] memory hops = _route();
        vm.prank(owner);
        vm.expectRevert(); // demands an absurd profit
        exec.execute(USDC, 5_000e6, 1_000_000e6, hops);
    }

    /// Round trip through a V3-style pool and back via Sushi V2 after creating an arb, so the new
    /// callback is exercised for real. `v3Like` = Pancake V3 or Algebra pool for WETH/USDC.
    function _arbVia(address v3Like) internal returns (uint256 profit) {
        _createArb(30 ether);
        bool usdcIsToken0OnSushi = IV2Pair(sushiPair).token0() == USDC;
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(0, sushiPair, USDC, WETH, usdcIsToken0OnSushi, 30);
        hops[1] = ArbExecutor.Hop(2, v3Like, WETH, USDC, WETH < USDC, 0);
        vm.prank(owner);
        profit = exec.execute(USDC, 2_000e6, 1, hops);
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
    }

    function test_pancakeV3Callback() public {
        address pool = IV3Factory(PANCAKE_V3_FACTORY).getPool(WETH, USDC, 500);
        assertTrue(pool != address(0), "pancake pool");
        assertGt(_arbVia(pool), 0);
    }

    function test_algebraCallback() public {
        address pool = IAlgebraFactory(CAMELOT_V3_FACTORY).poolByPair(WETH, USDC);
        assertTrue(pool != address(0), "camelot v3 pool");
        assertGt(_arbVia(pool), 0);
    }

    function test_newCallbacksRejectUnexpectedCaller() public {
        vm.startPrank(attacker);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.pancakeV3SwapCallback(1, 0, "");
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.algebraSwapCallback(1, 0, "");
        vm.stopPrank();
    }
}
