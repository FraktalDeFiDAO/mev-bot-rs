// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Vm} from "forge-std/Vm.sol";
import {FastFeedTicketVault} from "../src/FastFeedTicketVault.sol";
import {UsdgConverter, IERC20Min, Owned} from "../src/UsdgConverter.sol";
import {VaultFork} from "./FastFeedTicketVault.t.sol";

/// A token that can call back into the vault while being transferred (an ERC-777-style hook).
contract MockToken {
    mapping(address => uint256) public balanceOf;
    address public target;
    bytes public data;

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
    }

    function arm(address target_, bytes calldata data_) external {
        target = target_;
        data = data_;
    }

    function transfer(address to, uint256 amt) external returns (bool) {
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        if (target != address(0)) {
            (bool ok, bytes memory r) = target.call(data);
            if (!ok) {
                assembly {
                    revert(add(r, 32), mload(r))
                }
            }
        }
        return true;
    }
}

interface IV3Callback {
    function uniswapV3SwapCallback(int256 a0, int256 a1, bytes calldata d) external;
}

/// A 1:1 "pool" with the same callback protocol as Uniswap V3 (pays out only after being paid).
contract MockV3Pool {
    address public token0;
    address public token1;

    constructor(address a, address b) {
        (token0, token1) = a < b ? (a, b) : (b, a);
    }

    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160, bytes calldata d)
        external
        returns (int256 a0, int256 a1)
    {
        (address tin, address tout) = zeroForOne ? (token0, token1) : (token1, token0);
        uint256 amt = uint256(amountSpecified);
        (a0, a1) = zeroForOne ? (int256(amt), -int256(amt)) : (-int256(amt), int256(amt));
        uint256 before_ = IERC20Min(tin).balanceOf(address(this));
        IV3Callback(msg.sender).uniswapV3SwapCallback(a0, a1, d);
        require(IERC20Min(tin).balanceOf(address(this)) >= before_ + amt, "not paid");
        IERC20Min(tout).transfer(recipient, amt);
    }
}

contract UsdgConverterForkTest is VaultFork {
    address attacker = makeAddr("attacker");

    function setUp() public override {
        super.setUp();
        vm.etch(attacker, "");
        vm.startPrank(owner);
        vault.setRoute(USDC, _usdcRoute(10, 100_000e6));
        vault.setRoute(address(0), _ethRoute(100, 50 ether));
        vm.stopPrank();
    }

    function _convert(address token, uint256 amt) internal returns (uint256) {
        vm.prank(keeper);
        return vault.convert(token, amt);
    }

    // ------------------------------------------------------------ real conversions

    function test_usdc_converts_to_usdg_through_the_hooked_v4_pool() public {
        deal(USDC, address(vault), 1_000e6);
        vm.recordLogs();
        uint256 out = _convert(USDC, 1_000e6);
        assertGe(out, 999e6, "about 1.4 bps of cost observed on the real pool");
        assertEq(USDG.balanceOf(address(vault)), out);
        assertEq(IERC20Min(USDC).balanceOf(address(vault)), 0, "all of it was swapped");
        assertEq(IERC20Min(USDC).allowance(address(vault), POOL_MANAGER), 0, "no standing approvals");
        Vm.Log[] memory logs = vm.getRecordedLogs();
        bool seen;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].topics[0] == UsdgConverter.Converted.selector) seen = true;
        }
        assertTrue(seen, "a Converted event was emitted");
    }

    function test_eth_converts_to_usdg_through_two_chained_swaps() public {
        vm.deal(address(vault), 1 ether);
        (uint256 expected,) = vault.quote(address(0), 1 ether);
        uint256 out = _convert(address(0), 1 ether);
        assertGe(out, expected * 99 / 100, "within the 100 bps bound of the oracle value");
        assertLe(out, expected * 101 / 100, "and not suspiciously above it");
        assertEq(address(vault).balance, 0);
        assertEq(IERC20Min(WETH).balanceOf(address(vault)), 0, "no WETH stranded");
        assertEq(IERC20Min(USDC).balanceOf(address(vault)), 0, "no USDC stranded between the hops");
        assertEq(USDG.balanceOf(address(vault)), out);
    }

    function test_sizes_from_one_dollar_to_a_hundred_thousand_never_pay_less_than_the_bound() public {
        uint256[4] memory sizes = [uint256(1e6), 100e6, 10_000e6, 100_000e6];
        for (uint256 i; i < sizes.length; ++i) {
            deal(USDC, address(vault), sizes[i]);
            (, uint256 minOut) = vault.quote(USDC, sizes[i]);
            uint256 before_ = USDG.balanceOf(address(vault));
            vm.prank(keeper);
            try vault.convert(USDC, sizes[i]) returns (uint256 out) {
                emit log_named_uint(string.concat("USDC in ", vm.toString(sizes[i] / 1e6), " -> USDG out (6 dec)"), out);
                assertGe(out, minOut, "success means the bound held");
                assertEq(USDG.balanceOf(address(vault)) - before_, out);
            } catch {
                emit log_named_string("refused", string.concat(vm.toString(sizes[i] / 1e6), " USDC"));
                assertEq(USDG.balanceOf(address(vault)), before_, "a refused conversion changes nothing");
                assertEq(IERC20Min(USDC).balanceOf(address(vault)), sizes[i]);
            }
            deal(USDC, address(vault), 0);
        }
        // The small and medium sizes must work on the real pool.
        deal(USDC, address(vault), 10_000e6);
        assertGe(_convert(USDC, 10_000e6), 9_990e6);
    }

    // ------------------------------------------------------------ the oracle bound

    function _mockEthPrice(int256 answer, uint256 updatedAt) internal {
        vm.mockCall(
            ETH_USD,
            abi.encodeWithSignature("latestRoundData()"),
            abi.encode(uint80(1), answer, uint256(1), updatedAt, uint80(1))
        );
    }

    function _liveEthAnswer() internal view returns (int256 a) {
        (bool ok, bytes memory r) = ETH_USD.staticcall(abi.encodeWithSignature("latestRoundData()"));
        require(ok);
        (, a,,,) = abi.decode(r, (uint80, int256, uint256, uint256, uint80));
    }

    function test_an_oracle_far_above_the_pool_makes_the_conversion_revert() public {
        vm.deal(address(vault), 1 ether);
        _mockEthPrice(_liveEthAnswer() * 110 / 100, block.timestamp); // oracle 10% above what the pool pays
        vm.prank(keeper);
        vm.expectPartialRevert(UsdgConverter.Slippage.selector);
        vault.convert(address(0), 1 ether);
        assertEq(address(vault).balance, 1 ether, "nothing was lost");
    }

    function test_an_oracle_below_the_pool_is_fine_because_we_receive_more_than_required() public {
        vm.deal(address(vault), 1 ether);
        _mockEthPrice(_liveEthAnswer() * 90 / 100, block.timestamp);
        uint256 out = _convert(address(0), 1 ether);
        (uint256 expected,) = vault.quote(address(0), 1 ether);
        assertGt(out, expected, "better than the oracle value");
    }

    function test_a_stale_oracle_is_refused() public {
        vm.deal(address(vault), 1 ether);
        _mockEthPrice(_liveEthAnswer(), block.timestamp - 3 days);
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.StaleOracle.selector);
        vault.convert(address(0), 1 ether);
        _mockEthPrice(0, block.timestamp);
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.StaleOracle.selector);
        vault.convert(address(0), 1 ether);
    }

    function test_a_down_or_just_restarted_sequencer_is_refused() public {
        vm.deal(address(vault), 1 ether);
        vm.mockCall(
            SEQUENCER_FEED,
            abi.encodeWithSignature("latestRoundData()"),
            abi.encode(uint80(1), int256(1), block.timestamp - 1 days, block.timestamp, uint80(1))
        );
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.SequencerDown.selector);
        vault.convert(address(0), 1 ether);
        // Up again, but only just: inside the one-hour grace period.
        vm.mockCall(
            SEQUENCER_FEED,
            abi.encodeWithSignature("latestRoundData()"),
            abi.encode(uint80(1), int256(0), block.timestamp - 60, block.timestamp, uint80(1))
        );
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.SequencerDown.selector);
        vault.convert(address(0), 1 ether);
    }

    function test_a_stablecoin_route_needs_no_oracle_and_ignores_the_sequencer_feed() public {
        vm.mockCall(
            SEQUENCER_FEED,
            abi.encodeWithSignature("latestRoundData()"),
            abi.encode(uint80(1), int256(1), block.timestamp, block.timestamp, uint80(1))
        );
        deal(USDC, address(vault), 100e6);
        assertGe(_convert(USDC, 100e6), 99e6);
    }

    // ------------------------------------------------------------ who may do what

    function test_only_whitelisted_tokens_amounts_and_balances() public {
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.NoRoute.selector);
        vault.convert(address(0xBEEF), 1);
        deal(USDC, address(vault), 1_000e6);
        vm.startPrank(keeper);
        vm.expectRevert(UsdgConverter.BadAmount.selector);
        vault.convert(USDC, 0);
        vm.expectRevert(UsdgConverter.BadAmount.selector);
        vault.convert(USDC, 100_001e6); // above maxPerCall
        vm.expectRevert(UsdgConverter.Insufficient.selector);
        vault.convert(USDC, 2_000e6); // more than the vault holds
        vm.stopPrank();
    }

    function test_swap_callbacks_are_refused_to_everyone_but_the_pool_during_a_conversion() public {
        vm.startPrank(attacker);
        vm.expectRevert(UsdgConverter.Unauthorized.selector);
        vault.uniswapV3SwapCallback(1e6, 0, "");
        vm.expectRevert(UsdgConverter.Unauthorized.selector);
        vault.unlockCallback("");
        vm.stopPrank();
        // Even the real pool manager is refused outside a conversion.
        vm.prank(POOL_MANAGER);
        vm.expectRevert(UsdgConverter.Unauthorized.selector);
        vault.unlockCallback("");
        vm.prank(V3_WETH_USDC);
        vm.expectRevert(UsdgConverter.Unauthorized.selector);
        vault.uniswapV3SwapCallback(1e6, 0, "");
    }

    function test_route_validation_rejects_broken_routes() public {
        UsdgConverter.Route memory r = _usdcRoute(10, 1e12);
        vm.startPrank(owner);
        // A route must end in USDG.
        r.steps[0].tokenOut = WETH;
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(USDC, r);
        // Steps must chain.
        r = _ethRoute(100, 1 ether);
        r.steps[1].tokenIn = WETH;
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(address(0), r);
        // A V3 step must name a pool that really trades that pair.
        r = _ethRoute(100, 1 ether);
        r.steps[0].tokenOut = address(USDG);
        r.steps[0].pool = V3_WETH_USDC; // WETH/USDC pool, not WETH/USDG
        r.steps[1] = r.steps[0];
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(address(0), r);
        // Slippage above 10% is refused; so is a zero cap, an unknown kind and too many steps.
        r = _usdcRoute(1_001, 1e12);
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(USDC, r);
        r = _usdcRoute(10, 0);
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(USDC, r);
        r = _usdcRoute(10, 1e12);
        r.steps[0].kind = 7;
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(USDC, r);
        r = _usdcRoute(10, 1e12);
        r.steps = new UsdgConverter.Step[](4);
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(USDC, r);
        // A route whose first step does not start at the token being whitelisted.
        r = _usdcRoute(10, 1e12);
        vm.expectRevert(UsdgConverter.BadRoute.selector);
        vault.setRoute(WETH, r);
        vm.stopPrank();
    }

    function test_routes_can_be_listed_replaced_and_removed() public {
        (address[] memory tokens, uint256[] memory bal) = vault.holdings();
        assertEq(tokens.length, 2);
        assertEq(tokens[0], USDC);
        assertEq(tokens[1], address(0));
        deal(USDC, address(vault), 7e6);
        vm.deal(address(vault), 2 ether);
        (, bal) = vault.holdings();
        assertEq(bal[0], 7e6);
        assertEq(bal[1], 2 ether);
        vm.startPrank(owner);
        vault.setRoute(USDC, _usdcRoute(20, 5_000e6)); // replace, not duplicate
        assertEq(vault.routeTokenCount(), 2);
        assertEq(vault.routeOf(USDC).maxSlippageBps, 20);
        vault.removeRoute(USDC);
        vm.expectRevert(UsdgConverter.NoRoute.selector);
        vault.removeRoute(USDC);
        vm.stopPrank();
        assertEq(vault.routeTokenCount(), 1);
        vm.prank(keeper);
        vm.expectRevert(UsdgConverter.NoRoute.selector);
        vault.convert(USDC, 1e6);
    }

    // ------------------------------------------------------------ hostile and broken pieces

    function test_a_token_that_calls_back_into_the_vault_cannot_re_enter_convert() public {
        MockToken evil = new MockToken();
        MockV3Pool pool = new MockV3Pool(address(evil), address(USDG));
        deal(address(USDG), address(pool), 1_000e6);
        UsdgConverter.Route memory r;
        r.steps = new UsdgConverter.Step[](1);
        r.steps[0] = UsdgConverter.Step(0, address(evil), address(USDG), address(pool), 0, 0, address(0));
        r.maxSlippageBps = 10;
        r.maxPerCall = 1_000e6;
        r.tokenDecimals = 6;
        vm.prank(owner);
        vault.setRoute(address(evil), r);
        evil.mint(address(vault), 100e6);

        // Control: with no hook armed the mock route converts 1:1, so the harness itself works.
        uint256 out = _convert(address(evil), 40e6);
        assertEq(out, 40e6);

        // Armed: the token re-enters `convert` while it is being paid to the pool.
        evil.arm(address(vault), abi.encodeCall(vault.convert, (address(evil), 1)));
        vm.prank(keeper);
        vm.expectRevert(Owned.TransferFailed.selector);
        vault.convert(address(evil), 40e6);
        assertEq(evil.balanceOf(address(vault)), 60e6, "the refused conversion moved nothing");
    }

    function test_a_pool_that_does_not_deliver_cannot_pass_the_bound() public {
        // A hostile pool that takes the input and pays out less than the bound requires.
        MockToken tok = new MockToken();
        StingyPool pool = new StingyPool(address(tok), address(USDG));
        deal(address(USDG), address(pool), 1_000e6);
        UsdgConverter.Route memory r;
        r.steps = new UsdgConverter.Step[](1);
        r.steps[0] = UsdgConverter.Step(0, address(tok), address(USDG), address(pool), 0, 0, address(0));
        r.maxSlippageBps = 10;
        r.maxPerCall = 1_000e6;
        r.tokenDecimals = 6;
        vm.prank(owner);
        vault.setRoute(address(tok), r);
        tok.mint(address(vault), 100e6);
        vm.prank(keeper);
        vm.expectPartialRevert(UsdgConverter.Slippage.selector);
        vault.convert(address(tok), 100e6);
        assertEq(tok.balanceOf(address(vault)), 100e6, "the whole conversion was undone");
    }

    function test_a_broken_conversion_route_never_blocks_renewals_and_can_be_repaired() public {
        // Point the USDC route at a hookless pool with no liquidity.
        UsdgConverter.Route memory broken = _usdcRoute(10, 100_000e6);
        broken.steps[0].fee = 9;
        broken.steps[0].hooks = address(0);
        vm.prank(owner);
        vault.setRoute(USDC, broken);
        deal(USDC, address(vault), 100e6);
        vm.prank(keeper);
        vm.expectRevert();
        vault.convert(USDC, 100e6);
        assertEq(IERC20Min(USDC).balanceOf(address(vault)), 100e6);

        // Renewal does not depend on conversion: it spends the USDG already held.
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 3, 1_000e6, 0);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1);

        // Repair the route (owner reconfigures the pool key): the conversion works again.
        vm.prank(owner);
        vault.setRoute(USDC, _usdcRoute(10, 100_000e6));
        assertGe(_convert(USDC, 100e6), 99e6);
    }

    // ------------------------------------------------------------ together

    function test_a_keeper_converts_deposits_then_renews() public {
        // The vault holds only USDC and ETH; the keeper turns them into USDG and buys the ticket.
        deal(USDC, address(vault), 30e6);
        vm.deal(address(vault), 0.02 ether);
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 3, 1_000e6, 0);
        vm.stopPrank();
        _intoSecondHalf();
        (,,,, uint256 usdgHeld,,,, bool can) = vault.status();
        assertEq(usdgHeld, 0);
        assertFalse(can, "nothing to pay with yet");
        _convert(USDC, 30e6);
        _convert(address(0), 0.02 ether);
        (,,,, usdgHeld,,,, can) = vault.status();
        assertGt(usdgHeld, 17e6, "enough for a ticket");
        assertTrue(can);
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1);
    }

    // ------------------------------------------------------------ properties

    /// forge-config: default.fuzz.runs = 24
    function testFuzz_a_successful_usdc_conversion_always_meets_the_bound(uint256 amt) public {
        amt = bound(amt, 1e6, 60_000e6);
        deal(USDC, address(vault), amt);
        (, uint256 minOut) = vault.quote(USDC, amt);
        vm.prank(keeper);
        try vault.convert(USDC, amt) returns (uint256 out) {
            assertGe(out, minOut);
            assertGe(out, amt * 9_990 / 10_000, "never worse than 10 bps below 1:1");
            assertEq(USDG.balanceOf(address(vault)), out);
        } catch {
            assertEq(USDG.balanceOf(address(vault)), 0);
            assertEq(IERC20Min(USDC).balanceOf(address(vault)), amt);
        }
    }

    /// forge-config: default.fuzz.runs = 24
    /// Whatever a keeper converts or renews, the owner's claim (USDG + tickets inside the Tickets contract + other
    /// holdings at oracle value) falls only by the ticket price and the slippage bound.
    function testFuzz_a_keeper_cannot_reduce_the_owners_claim_beyond_bound_and_price(uint256 usdcAmt, uint256 ethAmt)
        public
    {
        usdcAmt = bound(usdcAmt, 1e6, 20_000e6);
        ethAmt = bound(ethAmt, 0.001 ether, 5 ether);
        deal(USDC, address(vault), usdcAmt);
        vm.deal(address(vault), ethAmt);
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 3, 1_000e6, 0);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        _intoSecondHalf();

        (uint256 ethValue,) = vault.quote(address(0), ethAmt);
        uint256 claimBefore = 100e6 + usdcAmt + ethValue;
        uint256 price = TICKETS.currentPrice();

        vm.startPrank(keeper);
        try vault.convert(USDC, usdcAmt) {} catch {}
        try vault.convert(address(0), ethAmt) {} catch {}
        vault.renew();
        vm.stopPrank();

        (uint256 ethLeft,) =
            address(vault).balance == 0 ? (uint256(0), uint256(0)) : vault.quote(address(0), address(vault).balance);
        uint256 claimAfter = USDG.balanceOf(address(vault)) + TICKETS.tokenBalance(address(vault))
            + IERC20Min(USDC).balanceOf(address(vault)) + ethLeft;
        uint256 slack = usdcAmt * 10 / 10_000 + ethValue * 100 / 10_000;
        assertGe(claimAfter + price + slack, claimBefore, "only the ticket price and the slippage bound can be lost");
    }
}

/// Takes the input and pays only half: the bound must catch it.
contract StingyPool {
    address public token0;
    address public token1;

    constructor(address a, address b) {
        (token0, token1) = a < b ? (a, b) : (b, a);
    }

    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160, bytes calldata d)
        external
        returns (int256 a0, int256 a1)
    {
        (address tin, address tout) = zeroForOne ? (token0, token1) : (token1, token0);
        uint256 amt = uint256(amountSpecified);
        (a0, a1) = zeroForOne ? (int256(amt), -int256(amt)) : (-int256(amt), int256(amt));
        uint256 before_ = IERC20Min(tin).balanceOf(address(this));
        IV3Callback(msg.sender).uniswapV3SwapCallback(a0, a1, d);
        require(IERC20Min(tin).balanceOf(address(this)) >= before_ + amt, "not paid");
        IERC20Min(tout).transfer(recipient, amt / 2);
    }
}
