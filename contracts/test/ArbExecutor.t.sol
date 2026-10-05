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

interface IAavePoolFull {
    function supply(address asset, uint256 amount, address onBehalfOf, uint16 referralCode) external;
    function borrow(address asset, uint256 amount, uint256 interestRateMode, uint16 referralCode, address onBehalfOf)
        external;
    function getUserAccountData(address user)
        external
        view
        returns (uint256, uint256, uint256, uint256, uint256, uint256 healthFactor);
}

interface IAaveOracle {
    function getAssetPrice(address asset) external view returns (uint256);
}

interface IERC20Approve {
    function approve(address, uint256) external returns (bool);
    function allowance(address, address) external view returns (uint256);
}

interface ICometFull {
    struct AssetInfo {
        uint8 offset;
        address asset;
        address priceFeed;
        uint64 scale;
        uint64 borrowCollateralFactor;
        uint64 liquidateCollateralFactor;
        uint64 liquidationFactor;
        uint128 supplyCap;
    }

    function supply(address asset, uint256 amount) external;
    function withdraw(address asset, uint256 amount) external;
    function getAssetInfoByAddress(address asset) external view returns (AssetInfo memory);
    function getPrice(address priceFeed) external view returns (uint256);
    function isLiquidatable(address account) external view returns (bool);
    function storeFrontPriceFactor() external view returns (uint256);
    function absorb(address absorber, address[] calldata accounts) external;
    function buyCollateral(address asset, uint256 minAmount, uint256 baseAmount, address recipient) external;
}

interface IMorphoFull {
    struct MarketParams {
        address loanToken;
        address collateralToken;
        address oracle;
        address irm;
        uint256 lltv;
    }

    function idToMarketParams(bytes32 id) external view returns (MarketParams memory);
    function supplyCollateral(MarketParams memory mp, uint256 assets, address onBehalf, bytes memory data) external;
    function borrow(MarketParams memory mp, uint256 assets, uint256 shares, address onBehalf, address receiver)
        external
        returns (uint256, uint256);
    function liquidate(
        MarketParams memory mp,
        address borrower,
        uint256 seizedAssets,
        uint256 repaidShares,
        bytes memory data
    ) external returns (uint256, uint256);
    function position(bytes32 id, address user) external view returns (uint256, uint128, uint128);
}

interface IMorphoOracle {
    function price() external view returns (uint256);
}

interface IFluidVaultT1 {
    function operate(uint256 nftId, int256 newCol, int256 newDebt, address to)
        external
        payable
        returns (uint256, int256, int256);
    function liquidate(uint256 debtAmt, uint256 colPerUnitDebt, address to, bool absorb)
        external
        payable
        returns (uint256 actualDebtAmt, uint256 actualColAmt);
}

interface IFluidOracle {
    function getExchangeRateOperate() external view returns (uint256);
    function getExchangeRateLiquidate() external view returns (uint256);
}

interface IFluidVaultResolver {
    struct LiquidationStruct {
        address vault;
        address tokenIn;
        address tokenOut;
        uint256 tokenInAmtOne;
        uint256 tokenOutAmtOne;
        uint256 tokenInAmtTwo;
        uint256 tokenOutAmtTwo;
        bool absorbAvailable;
    }

    function getVaultLiquidation(address vault, uint256 tokenInAmt) external returns (LiquidationStruct memory);
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
    address constant AAVE_POOL = 0x794a61358D6845594F94dc1DB02A252b5b4814aD;
    address constant AAVE_ORACLE = 0xb56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7;

    address owner = makeAddr("owner");
    address receiver = makeAddr("receiver");
    address attacker = makeAddr("attacker");

    ArbExecutor exec;
    address v3Pool;
    address sushiPair;

    function setUp() public {
        vm.createSelectFork(vm.envOr("ARBITRUM_HTTP_URL", string("https://arb1.arbitrum.io/rpc")));
        vm.prank(owner);
        exec = new ArbExecutor(VAULT, WETH, receiver);
        vm.prank(owner);
        exec.setTarget(AAVE_POOL, true);
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

    /// Flash-swap route (profit in WETH): sell WETH on Uniswap V3 (the pool fronts the USDC), buy
    /// the cheap WETH back on Sushi after `_createArb`, repay the V3 pool in WETH.
    function _flashRoute() internal view returns (ArbExecutor.Hop[] memory hops) {
        bool usdcIsToken0OnSushi = IV2Pair(sushiPair).token0() == USDC;
        hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(2, v3Pool, WETH, USDC, WETH < USDC, 0);
        hops[1] = ArbExecutor.Hop(0, sushiPair, USDC, WETH, usdcIsToken0OnSushi, 30);
    }

    function test_flashSwapCapturesRealArbAndHoldsNothing() public {
        _createArb(30 ether);
        uint256 before = IERC20Full(WETH).balanceOf(receiver);
        ArbExecutor.Hop[] memory hops = _flashRoute();
        vm.prank(owner);
        uint256 profit = exec.executeFlashSwap(2 ether, 1e12, hops);
        assertGt(profit, 1e12, "expected profit");
        assertEq(IERC20Full(WETH).balanceOf(receiver) - before, profit, "profit to receiver");
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
        emit log_named_decimal_uint("flash-swap profit WETH", profit, 18);
    }

    function test_flashSwapOnlyOwnerAndRouteChecks() public {
        ArbExecutor.Hop[] memory hops = _flashRoute();
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.executeFlashSwap(1 ether, 0, hops);

        // First hop must be a V3-style pool.
        ArbExecutor.Hop[] memory v2First = _route();
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.executeFlashSwap(1_000e6, 0, v2First);

        // Route must return to its starting token.
        ArbExecutor.Hop[] memory open = new ArbExecutor.Hop[](2);
        open[0] = hops[0];
        open[1] = hops[0];
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.executeFlashSwap(1 ether, 0, open);
    }

    function test_flashSwapUnprofitableAndMinProfitRevert() public {
        ArbExecutor.Hop[] memory hops = _flashRoute();
        vm.prank(owner);
        vm.expectRevert(); // no gap: the round trip loses fees
        exec.executeFlashSwap(1 ether, 0, hops);

        _createArb(30 ether);
        vm.prank(owner);
        vm.expectRevert(); // absurd minimum
        exec.executeFlashSwap(2 ether, 1_000 ether, hops);
    }

    function test_flashCallbackRejectsCallsOutsideAFlashSwap() public {
        bytes memory data = abi.encode(uint256(0), _flashRoute());
        vm.prank(v3Pool); // the real pool, but no flash swap of ours is running
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.uniswapV3SwapCallback(0, -1, data);
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.algebraSwapCallback(0, -1, data);
    }

    /// Round trip through a V3-style pool and back via Sushi V2 after creating an arb, so the new
    /// callback is exercised for real. `v3Like` = Pancake V3 or Algebra pool for WETH/USDC.
    /// `dump` = WETH dumped into Sushi to create the gap. Kept small for concentrated-liquidity
    /// pools: a large gap makes the second leg cross many ticks, and every tick is a remote storage
    /// read for the fork backend (public RPCs throttle -> FatalExternalError, not a contract bug).
    function _arbVia(address v3Like) internal returns (uint256 profit) {
        _createArb(3 ether);
        bool usdcIsToken0OnSushi = IV2Pair(sushiPair).token0() == USDC;
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](2);
        hops[0] = ArbExecutor.Hop(0, sushiPair, USDC, WETH, usdcIsToken0OnSushi, 30);
        hops[1] = ArbExecutor.Hop(2, v3Like, WETH, USDC, WETH < USDC, 0);
        vm.prank(owner);
        profit = exec.execute(USDC, 500e6, 1, hops);
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

    // ---------------------------------------------------------------- Aave v3 liquidations

    address borrower = makeAddr("borrower");

    /// Open a real Aave position (1 WETH collateral, USDC debt near the limit), then mock the Aave
    /// oracle WETH price down `dropBps` so the position becomes liquidatable.
    function _liquidatablePosition(uint256 dropBps) internal returns (uint256 debtUsdc) {
        deal(WETH, borrower, 1 ether);
        vm.startPrank(borrower);
        IERC20Approve(WETH).approve(AAVE_POOL, 1 ether);
        IAavePoolFull(AAVE_POOL).supply(WETH, 1 ether, borrower, 0);
        (,, uint256 availableBase,,,) = IAavePoolFull(AAVE_POOL).getUserAccountData(borrower);
        debtUsdc = (availableBase * 99) / 100 / 100; // base is 8 decimals USD; USDC has 6
        IAavePoolFull(AAVE_POOL).borrow(USDC, debtUsdc, 2, 0, borrower);
        vm.stopPrank();

        uint256 px = IAaveOracle(AAVE_ORACLE).getAssetPrice(WETH);
        vm.mockCall(
            AAVE_ORACLE,
            abi.encodeWithSelector(IAaveOracle.getAssetPrice.selector, WETH),
            abi.encode(px * (10_000 - dropBps) / 10_000)
        );
        (,,,,, uint256 hf) = IAavePoolFull(AAVE_POOL).getUserAccountData(borrower);
        assertLt(hf, 1e18, "position should be liquidatable");
    }

    function _wethToUsdcHops() internal view returns (ArbExecutor.Hop[] memory hops) {
        hops = new ArbExecutor.Hop[](1);
        hops[0] = ArbExecutor.Hop(2, v3Pool, WETH, USDC, WETH < USDC, 0);
    }

    /// Aave liquidationCall(WETH collateral, USDC debt, user, max, false), approving exactly `flash` USDC.
    function _aaveCall(address user, uint256 flash) internal pure returns (ArbExecutor.ProtocolCall[] memory calls) {
        calls = new ArbExecutor.ProtocolCall[](1);
        calls[0] = ArbExecutor.ProtocolCall(
            AAVE_POOL,
            abi.encodeWithSignature(
                "liquidationCall(address,address,address,uint256,bool)", WETH, USDC, user, type(uint256).max, false
            ),
            AAVE_POOL,
            flash
        );
    }

    function test_liquidatesAavePositionForProfit() public {
        uint256 debt = _liquidatablePosition(1_500);
        // Flash a 0.1% buffer over the debt: Aave rounds variable debt up, and the excess is returned.
        uint256 flash = debt * 1001 / 1000;
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        ArbExecutor.ProtocolCall[] memory calls = _aaveCall(borrower, flash);
        uint256 before = IERC20Full(USDC).balanceOf(receiver);

        vm.prank(owner);
        uint256 profit = exec.liquidate(USDC, WETH, flash, 1e6, calls, hops);

        assertGt(profit, 1e6, "expected >1 USDC profit");
        assertEq(IERC20Full(USDC).balanceOf(receiver) - before, profit, "profit to receiver");
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
        assertEq(IERC20Approve(USDC).allowance(address(exec), AAVE_POOL), 0, "approval reset");
        emit log_named_decimal_uint("liquidation profit USDC", profit, 6);
    }

    function test_liquidateHealthyPositionReverts() public {
        deal(WETH, borrower, 1 ether);
        vm.startPrank(borrower);
        IERC20Approve(WETH).approve(AAVE_POOL, 1 ether);
        IAavePoolFull(AAVE_POOL).supply(WETH, 1 ether, borrower, 0);
        IAavePoolFull(AAVE_POOL).borrow(USDC, 100e6, 2, 0, borrower);
        vm.stopPrank();
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        ArbExecutor.ProtocolCall[] memory calls = _aaveCall(borrower, 101e6);
        vm.prank(owner);
        vm.expectRevert(); // Aave: health factor not below threshold (bubbled up)
        exec.liquidate(USDC, WETH, 101e6, 0, calls, hops);
    }

    function test_liquidateOnlyOwnerAndRouteChecks() public {
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        ArbExecutor.ProtocolCall[] memory calls = _aaveCall(borrower, 1e6);
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.liquidate(USDC, WETH, 1e6, 0, calls, hops);

        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.liquidate(USDC, WETH, 1e6, 0, calls, noHops); // different tokens need a swap route

        ArbExecutor.ProtocolCall[] memory noCalls = new ArbExecutor.ProtocolCall[](0);
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.liquidate(USDC, WETH, 1e6, 0, noCalls, hops);
    }

    function test_liquidationMinProfitEnforced() public {
        uint256 debt = _liquidatablePosition(1_500);
        uint256 flash = debt * 1001 / 1000;
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        ArbExecutor.ProtocolCall[] memory calls = _aaveCall(borrower, flash);
        vm.prank(owner);
        vm.expectRevert(); // NotProfitable
        exec.liquidate(USDC, WETH, flash, 1_000_000e6, calls, hops);
    }

    function test_nonAllowlistedTargetReverts() public {
        uint256 debt = _liquidatablePosition(1_500);
        uint256 flash = debt * 1001 / 1000;
        vm.prank(owner);
        exec.setTarget(AAVE_POOL, false);
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        ArbExecutor.ProtocolCall[] memory calls = _aaveCall(borrower, flash);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.TargetNotAllowed.selector, AAVE_POOL));
        exec.liquidate(USDC, WETH, flash, 0, calls, hops);
    }

    function test_cannotCallTokenToDrainFlashFunds() public {
        // Even if the owner key were misused, calling a token is blocked unless allowlisted.
        ArbExecutor.ProtocolCall[] memory calls = new ArbExecutor.ProtocolCall[](1);
        calls[0] = ArbExecutor.ProtocolCall(
            USDC, abi.encodeWithSignature("transfer(address,uint256)", attacker, 1e6), address(0), 0
        );
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.TargetNotAllowed.selector, USDC));
        exec.liquidate(USDC, USDC, 1e6, 0, calls, noHops);
    }

    function test_setTargetOnlyOwner() public {
        vm.prank(attacker);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.setTarget(attacker, true);
    }

    // ---------------------------------------------------------------- Compound III (Comet)

    address constant COMET_USDC = 0x9c4ec768c28520B50860ea7a15bd7213a9fF58bf;

    /// Real Comet position (1 WETH collateral, USDC borrowed near the limit), then the WETH price
    /// feed is mocked down 15% so the account becomes absorbable. Returns the mocked price (8 dec).
    function _cometPosition() internal returns (uint256 mockedPrice, ICometFull.AssetInfo memory info) {
        ICometFull comet = ICometFull(COMET_USDC);
        info = comet.getAssetInfoByAddress(WETH);
        uint256 px = comet.getPrice(info.priceFeed);
        deal(WETH, borrower, 1 ether);
        vm.startPrank(borrower);
        IERC20Approve(WETH).approve(COMET_USDC, 1 ether);
        comet.supply(WETH, 1 ether);
        uint256 borrowUsdc = px * uint256(info.borrowCollateralFactor) / 1e18 * 98 / 100 / 100; // 8dec -> 6dec
        comet.withdraw(USDC, borrowUsdc);
        vm.stopPrank();

        mockedPrice = px * 85 / 100;
        vm.mockCall(
            info.priceFeed,
            abi.encodeWithSignature("latestRoundData()"),
            abi.encode(uint80(1), int256(mockedPrice), uint256(0), block.timestamp, uint80(1))
        );
        assertTrue(comet.isLiquidatable(borrower), "position should be absorbable");
    }

    function test_compoundAbsorbAndBuyCollateralForProfit() public {
        (uint256 mockedPrice, ICometFull.AssetInfo memory info) = _cometPosition();
        uint256 discount = ICometFull(COMET_USDC).storeFrontPriceFactor() * (1e18 - info.liquidationFactor) / 1e18;
        // Base to pay for 1 WETH at the storefront price, with the adapter's 0.1% haircut.
        uint256 baseAmount = mockedPrice * (1e18 - discount) / 1e18 / 100 * 999 / 1000;

        ArbExecutor.ProtocolCall[] memory calls = new ArbExecutor.ProtocolCall[](2);
        address[] memory accounts = new address[](1);
        accounts[0] = borrower;
        calls[0] = ArbExecutor.ProtocolCall(
            COMET_USDC, abi.encodeCall(ICometFull.absorb, (address(exec), accounts)), address(0), 0
        );
        calls[1] = ArbExecutor.ProtocolCall(
            COMET_USDC,
            abi.encodeCall(ICometFull.buyCollateral, (WETH, 0.99 ether, baseAmount, address(exec))),
            COMET_USDC,
            baseAmount
        );
        ArbExecutor.Hop[] memory hops = _wethToUsdcHops();
        vm.prank(owner);
        exec.setTarget(COMET_USDC, true);

        vm.prank(owner);
        uint256 profit = exec.liquidate(USDC, WETH, baseAmount, 1e6, calls, hops);

        assertGt(profit, 1e6, "expected >1 USDC profit");
        assertFalse(ICometFull(COMET_USDC).isLiquidatable(borrower), "borrower absorbed");
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "executor holds no WETH");
        assertEq(IERC20Approve(USDC).allowance(address(exec), COMET_USDC), 0, "approval reset");
        emit log_named_decimal_uint("comet absorb+buy profit USDC", profit, 6);
    }

    // ---------------------------------------------------------------- Morpho Blue

    address constant MORPHO = 0x6c247b1F6182318877311737BaC0844bAa518F5e;
    address constant WBTC = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f;
    bytes32 constant MORPHO_WBTC_USDC = 0xe6392ff19d10454b099d692b58c361ef93e31af34ed1ef78232e07c78fe99169;

    function test_morphoLiquidationForProfit() public {
        IMorphoFull morpho = IMorphoFull(MORPHO);
        IMorphoFull.MarketParams memory mp = morpho.idToMarketParams(MORPHO_WBTC_USDC);
        assertEq(mp.collateralToken, WBTC);
        assertEq(mp.loanToken, USDC);

        // Real position: 0.1 WBTC collateral, borrow 98% of the LLTV limit.
        uint256 price = IMorphoOracle(mp.oracle).price();
        deal(WBTC, borrower, 0.1e8);
        vm.startPrank(borrower);
        IERC20Approve(WBTC).approve(MORPHO, 0.1e8);
        morpho.supplyCollateral(mp, 0.1e8, borrower, "");
        uint256 borrowAssets = 0.1e8 * price / 1e36 * mp.lltv / 1e18 * 98 / 100;
        morpho.borrow(mp, borrowAssets, 0, borrower, borrower);
        vm.stopPrank();

        // Oracle drops 15%: unhealthy.
        uint256 mocked = price * 85 / 100;
        vm.mockCall(mp.oracle, abi.encodeWithSelector(IMorphoOracle.price.selector), abi.encode(mocked));

        // Seize all collateral the debt allows (mirrors mev-lending-morpho::math::size).
        uint256 lif = 1e36 / (1e18 - 3e17 * (1e18 - mp.lltv) / 1e18); // 1e18-scaled incentive factor
        if (lif > 1.15e18) lif = 1.15e18;
        uint256 seizeByDebt = borrowAssets * lif / 1e18 * 1e36 / mocked;
        uint256 seized = (seizeByDebt < 0.1e8 ? seizeByDebt : 0.1e8) * 999 / 1000;
        uint256 flash = seized * mocked / 1e36 * 1e18 / lif * 101 / 100 + 1;

        ArbExecutor.ProtocolCall[] memory calls = new ArbExecutor.ProtocolCall[](1);
        calls[0] = ArbExecutor.ProtocolCall(
            MORPHO, abi.encodeCall(IMorphoFull.liquidate, (mp, borrower, seized, 0, "")), MORPHO, flash
        );
        address wbtcUsdc = IV3Factory(UNI_V3_FACTORY).getPool(WBTC, USDC, 500);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](1);
        hops[0] = ArbExecutor.Hop(2, wbtcUsdc, WBTC, USDC, WBTC < USDC, 0);

        vm.prank(owner);
        exec.setTarget(MORPHO, true);
        vm.prank(owner);
        uint256 profit = exec.liquidate(USDC, WBTC, flash, 1e6, calls, hops);

        assertGt(profit, 1e6, "expected >1 USDC profit");
        (, uint128 sharesAfter, uint128 collAfter) = morpho.position(MORPHO_WBTC_USDC, borrower);
        assertLt(collAfter, 0.1e8, "collateral seized");
        assertTrue(sharesAfter < type(uint128).max, "position updated");
        assertEq(IERC20Full(USDC).balanceOf(address(exec)), 0, "executor holds no USDC");
        assertEq(IERC20Full(WBTC).balanceOf(address(exec)), 0, "executor holds no WBTC");
        assertEq(IERC20Approve(USDC).allowance(address(exec), MORPHO), 0, "approval reset");
        emit log_named_decimal_uint("morpho liquidation profit USDC", profit, 6);
    }

    // ---------------------------------------------------------------- Fluid (native ETH collateral)

    address constant FLUID_ETH_ARB = 0xf55B8e9F0c51Ace009f4b41d03321675d4C643b3; // collateral ETH, debt ARB
    address constant FLUID_ETH_ARB_ORACLE = 0x916f49Bc155A1A2E47e44afA588A6DF3890bB9eB;
    address constant FLUID_VAULT_RESOLVER = 0xA5C3E16523eeeDDcC34706b0E6bE88b4c6EA95cC;
    address constant ARB = 0x912CE59144191C1204E64559FE8253a0e49E6548;

    function test_fluidLiquidationWithNativeEthCollateral() public {
        // Real position: 1 ETH collateral, borrow ARB at ~70% of the oracle value.
        uint256 rate = IFluidOracle(FLUID_ETH_ARB_ORACLE).getExchangeRateLiquidate(); // 1e27-scaled ARB per ETH
        uint256 borrowArb = 1 ether * rate / 1e27 * 70 / 100;
        vm.deal(borrower, 1 ether);
        vm.prank(borrower);
        IFluidVaultT1(FLUID_ETH_ARB).operate{value: 1 ether}(0, int256(1 ether), int256(borrowArb), borrower);

        // ETH crashes 45% against ARB: the position falls past the liquidation threshold.
        uint256 mocked = rate * 55 / 100;
        vm.mockCall(
            FLUID_ETH_ARB_ORACLE,
            abi.encodeWithSelector(IFluidOracle.getExchangeRateLiquidate.selector),
            abi.encode(mocked)
        );
        vm.mockCall(
            FLUID_ETH_ARB_ORACLE,
            abi.encodeWithSelector(IFluidOracle.getExchangeRateOperate.selector),
            abi.encode(mocked)
        );

        // Offer up to 3,000 ARB of debt; the vault takes only what is liquidatable (the rest of the
        // flash loan is returned). The resolver can't be used here: it re-simulates internally,
        // which conflicts with the mocked oracle. Live, the adapter reads it via eth_call.
        uint256 debtAmt = 3_000 ether;
        uint256 colPerUnitDebt = 0;

        ArbExecutor.ProtocolCall[] memory calls = new ArbExecutor.ProtocolCall[](1);
        calls[0] = ArbExecutor.ProtocolCall(
            FLUID_ETH_ARB,
            abi.encodeCall(IFluidVaultT1.liquidate, (debtAmt, colPerUnitDebt, address(exec), false)),
            FLUID_ETH_ARB,
            debtAmt
        );
        // ETH arrives natively, gets wrapped, then WETH -> ARB on Uniswap V3.
        address wethArb = IV3Factory(UNI_V3_FACTORY).getPool(WETH, ARB, 500);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](1);
        hops[0] = ArbExecutor.Hop(2, wethArb, WETH, ARB, WETH < ARB, 0);

        vm.prank(owner);
        exec.setTarget(FLUID_ETH_ARB, true);
        vm.prank(owner);
        uint256 profit = exec.liquidate(ARB, WETH, debtAmt, 1e18, calls, hops);

        assertGt(profit, 1e18, "expected >1 ARB profit");
        assertEq(address(exec).balance, 0, "no ETH left");
        assertEq(IERC20Full(WETH).balanceOf(address(exec)), 0, "no WETH left");
        assertEq(IERC20Full(ARB).balanceOf(address(exec)), 0, "no ARB left");
        assertEq(IERC20Approve(ARB).allowance(address(exec), FLUID_ETH_ARB), 0, "approval reset");
        emit log_named_decimal_uint("fluid liquidation profit ARB", profit, 18);
    }

    function test_rejectsEthOutsideLiquidation() public {
        vm.deal(attacker, 1 ether);
        vm.prank(attacker);
        (bool ok,) = address(exec).call{value: 1 ether}("");
        assertFalse(ok, "plain ETH transfers must be rejected");
    }
}
