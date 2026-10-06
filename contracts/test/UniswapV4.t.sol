// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

interface IERC20V4 {
    function balanceOf(address) external view returns (uint256);
    function transfer(address, uint256) external returns (bool);
}

interface IV3FactoryV4 {
    function getPool(address, address, uint24) external view returns (address);
}

/// Exposes the executor's internal swap so each V4 hop kind can be tested on its own.
contract V4Harness is ArbExecutor {
    constructor(address v, address w, address r) ArbExecutor(v, w, r) {}

    function swapForTest(Hop memory h, uint256 amountIn) external returns (uint256) {
        if (msg.sender != owner) revert NotOwner();
        return _swap(h, amountIn);
    }
}

/// Fork tests against the real Uniswap V4 PoolManager on Arbitrum (hookless pools only).
contract UniswapV4ForkTest is Test {
    address constant VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address constant USDT0 = 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9;
    address constant POOL_MANAGER = 0x360E68faCcca8cA495c1B759Fd9EEe466db9FB32;
    address constant UNI_V3_FACTORY = 0x1F98431c8aD98523631AE4a59f267346ea31F984;

    address owner = makeAddr("owner");
    address receiver = makeAddr("receiver");
    V4Harness h;

    /// Tick spacing travels in the hop's `pool` field.
    function _spacing(uint160 s) internal pure returns (address) {
        return address(s);
    }

    function setUp() public {
        vm.createSelectFork(vm.envOr("ARBITRUM_HTTP_URL", string("https://arb1.arbitrum.io/rpc")));
        vm.startPrank(owner);
        h = new V4Harness(VAULT, WETH, receiver);
        h.setPoolManager(POOL_MANAGER);
        vm.stopPrank();
    }

    function _swap(ArbExecutor.Hop memory hop, address tokenIn, uint256 amountIn) internal returns (uint256 out) {
        deal(tokenIn, address(h), amountIn);
        vm.prank(owner);
        out = h.swapForTest(hop, amountIn);
    }

    function test_erc20PoolStableSwap() public {
        // USDC/USDT0, fee 8 (0.0008%), spacing 1
        ArbExecutor.Hop memory hop = ArbExecutor.Hop(3, _spacing(1), USDC, USDT0, true, 8);
        uint256 out = _swap(hop, USDC, 1_000e6);
        assertGt(out, 995e6, "stable pair returns ~1:1");
        assertLt(out, 1_005e6);
        assertEq(IERC20V4(USDC).balanceOf(address(h)), 0, "input fully settled");
        assertEq(IERC20V4(USDT0).balanceOf(address(h)), out, "output taken");
    }

    function test_nativePoolBothDirectionsAndMatchesTheWethPool() public {
        // ETH/USDC fee 500 spacing 10 (native) vs WETH/USDC fee 500 spacing 10 (ERC20 WETH)
        ArbExecutor.Hop memory nat = ArbExecutor.Hop(4, _spacing(10), WETH, USDC, true, 500);
        ArbExecutor.Hop memory erc = ArbExecutor.Hop(3, _spacing(10), WETH, USDC, true, 500);
        uint256 viaNative = _swap(nat, WETH, 1 ether);
        assertEq(IERC20V4(WETH).balanceOf(address(h)), 0, "WETH unwrapped and spent");
        assertEq(address(h).balance, 0, "no ETH left behind");
        uint256 viaErc20 = _swap(erc, WETH, 1 ether);
        assertGt(viaNative, 1_000e6, "1 ETH is worth > $1000");
        // Two pools of the same pair stay arbitraged to within a couple of percent.
        uint256 hi = viaNative > viaErc20 ? viaNative : viaErc20;
        uint256 lo = viaNative > viaErc20 ? viaErc20 : viaNative;
        assertLt((hi - lo) * 100 / hi, 3, "native and WETH pools agree");

        // Back: USDC -> ETH -> wrapped as WETH.
        ArbExecutor.Hop memory back = ArbExecutor.Hop(4, _spacing(10), USDC, WETH, false, 500);
        uint256 w = _swap(back, USDC, 1_000e6);
        assertEq(IERC20V4(WETH).balanceOf(address(h)), w, "output wrapped to WETH");
        assertEq(address(h).balance, 0);
        assertGt(w, 0.1 ether);
    }

    function test_badV4HopsRevert() public {
        deal(USDC, address(h), 1_000e6);
        vm.startPrank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        h.swapForTest(ArbExecutor.Hop(3, _spacing(0), USDC, USDT0, true, 8), 1e6); // no spacing
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        h.swapForTest(ArbExecutor.Hop(3, _spacing(40000), USDC, USDT0, true, 8), 1e6); // spacing out of range
        vm.expectRevert(); // a pool that was never initialized
        h.swapForTest(ArbExecutor.Hop(3, _spacing(1), USDC, USDT0, true, 1234), 1e6);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        h.swapForTest(ArbExecutor.Hop(3, _spacing(1), USDC, USDC, true, 8), 1e6); // same currency
        vm.stopPrank();
        // Without a manager configured V4 hops are disabled.
        vm.startPrank(owner);
        h.setPoolManager(address(0));
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        h.swapForTest(ArbExecutor.Hop(3, _spacing(1), USDC, USDT0, true, 8), 1e6);
        vm.stopPrank();
    }

    function test_onlyOwnerSetsThePoolManagerAndTheCallbackIsGuarded() public {
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        h.setPoolManager(address(1));
        bytes memory data = abi.encode(uint8(3), USDC, USDT0, uint24(8), int24(1), uint256(1e6));
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        h.unlockCallback(data); // a stranger
        vm.prank(POOL_MANAGER);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        h.unlockCallback(data); // the manager, but no V4 hop of ours is running
    }

    /// End to end: dislocate the V4 WETH/USDC pool, then take the arb with a Balancer-funded
    /// executor.execute (buy WETH on V4, sell on Uniswap V3) and check the profit and that nothing stays.
    function test_arbBetweenV4AndV3PaysAndHoldsNothing() public {
        vm.startPrank(owner);
        ArbExecutor exec = new ArbExecutor(VAULT, WETH, receiver);
        exec.setPoolManager(POOL_MANAGER);
        vm.stopPrank();
        // Dump WETH into the V4 pool so WETH is cheap there.
        _swap(ArbExecutor.Hop(3, _spacing(10), WETH, USDC, true, 500), WETH, 40 ether);

        address v3 = IV3FactoryV4(UNI_V3_FACTORY).getPool(WETH, USDC, 500);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(3, _spacing(10), USDC, WETH, false, 500); // buy cheap WETH on V4
        hops[1] = ArbExecutor.Hop(2, v3, WETH, USDC, WETH < USDC, 0); // sell on V3
        uint256 before = IERC20V4(USDC).balanceOf(receiver);
        vm.prank(owner);
        uint256 profit = exec.execute(USDC, 8_000e6, 1e6, hops);
        assertGt(profit, 1e6, "the dislocation paid");
        assertEq(IERC20V4(USDC).balanceOf(receiver) - before, profit, "profit to the receiver");
        assertEq(IERC20V4(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20V4(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
        assertEq(address(exec).balance, 0, "executor holds no ETH");
        emit log_named_decimal_uint("V4/V3 arb profit USDC", profit, 6);
    }
}
