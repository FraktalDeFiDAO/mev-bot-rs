// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {StdInvariant} from "forge-std/StdInvariant.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

/// Network-free tests: access control, accounting, and invariants against mocks. These complement
/// the fork tests (real protocols) and run anywhere in seconds.

contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

/// Balancer-style vault: lends, calls back, requires repayment.
contract MockVault {
    function flashLoan(address recipient, address[] calldata tokens, uint256[] calldata amounts, bytes calldata data)
        external
    {
        MockERC20 t = MockERC20(tokens[0]);
        uint256 before = t.balanceOf(address(this));
        t.transfer(recipient, amounts[0]);
        uint256[] memory fees = new uint256[](1);
        (bool ok, bytes memory ret) = recipient.call(
            abi.encodeWithSignature(
                "receiveFlashLoan(address[],uint256[],uint256[],bytes)", tokens, amounts, fees, data
            )
        );
        if (!ok) {
            assembly {
                revert(add(ret, 32), mload(ret))
            }
        }
        require(t.balanceOf(address(this)) >= before, "not repaid");
    }
}

/// Liquidation protocol stand-in: pulls `repay` of debt from the caller, pays `reward` of debt
/// (same-token liquidation, so no swap route is needed).
contract MockProtocol {
    function liquidate(MockERC20 debt, uint256 repay, uint256 reward) external {
        debt.transferFrom(msg.sender, address(this), repay);
        debt.mint(msg.sender, reward);
    }
}

contract MockWETH {
    function deposit() external payable {}
}

contract ArbExecutorUnitTest is Test {
    address owner = makeAddr("owner");
    address receiver = makeAddr("receiver");
    MockERC20 token;
    MockVault vault;
    MockProtocol protocol;
    ArbExecutor exec;

    function setUp() public {
        token = new MockERC20();
        vault = new MockVault();
        protocol = new MockProtocol();
        token.mint(address(vault), 1e30);
        address weth = address(new MockWETH()); // create before prank: `new` would consume it
        vm.prank(owner);
        exec = new ArbExecutor(address(vault), weth, receiver);
        vm.prank(owner);
        exec.setTarget(address(protocol), true);
    }

    function _liqCall(uint256 repay, uint256 reward) internal view returns (ArbExecutor.ProtocolCall[] memory c) {
        c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol), abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), address(protocol), repay
        );
    }

    // ------------------------------------------------------------------ access control (fuzz)

    function testFuzz_onlyOwnerCanTrade(address caller) public {
        vm.assume(caller != owner);
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.startPrank(caller);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.execute(address(token), 1, 0, noHops);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.liquidate(address(token), address(token), 1, 0, _liqCall(1, 1), noHops);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.setTarget(caller, true);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.rescue(address(token));
        vm.stopPrank();
    }

    function testFuzz_callbacksRejectAnyone(address caller, int256 a0, int256 a1, bytes calldata data) public {
        vm.startPrank(caller);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.uniswapV3SwapCallback(a0, a1, data);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.pancakeV3SwapCallback(a0, a1, data);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.algebraSwapCallback(a0, a1, data);
        vm.stopPrank();
    }

    function testFuzz_flashCallbackRejectsAnyoneOutsideATrade(address caller, bytes calldata data) public {
        address[] memory t = new address[](1);
        uint256[] memory a = new uint256[](1);
        vm.prank(caller);
        vm.expectRevert(ArbExecutor.Unauthorized.selector);
        exec.receiveFlashLoan(t, a, a, data);
    }

    function testFuzz_mismatchedRoutesRevert(address loan, address first, address last) public {
        vm.assume(loan != first || loan != last);
        ArbExecutor.Hop[] memory hops = new ArbExecutor.Hop[](1);
        hops[0] = ArbExecutor.Hop(0, address(1), first, last, true, 30);
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.execute(loan, 1, 0, hops);
    }

    // ------------------------------------------------------------------ accounting (fuzz)

    function testFuzz_liquidationProfitAccounting(uint96 repay, uint96 extra, uint96 minProfit) public {
        vm.assume(repay > 0);
        uint256 reward = uint256(repay) + extra; // what the protocol pays back
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        uint256 before = token.balanceOf(receiver);
        vm.prank(owner);
        if (extra < minProfit) {
            vm.expectRevert(); // NotProfitable
            exec.liquidate(address(token), address(token), repay, minProfit, _liqCall(repay, reward), noHops);
            return;
        }
        uint256 profit =
            exec.liquidate(address(token), address(token), repay, minProfit, _liqCall(repay, reward), noHops);
        assertEq(profit, extra, "profit = reward - repayment of the flash loan");
        assertEq(token.balanceOf(receiver) - before, profit);
        assertEq(token.balanceOf(address(exec)), 0, "executor holds nothing");
        assertEq(token.allowance(address(exec), address(protocol)), 0, "approval reset");
    }

    function testFuzz_disallowedTargetsAlwaysRevert(address target) public {
        vm.assume(target != address(protocol));
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(target, "", address(0), 0);
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.TargetNotAllowed.selector, target));
        exec.liquidate(address(token), address(token), 1, 0, c, noHops);
    }

    function testFuzz_disallowedSpenderReverts(address spender) public {
        vm.assume(spender != address(protocol));
        ArbExecutor.ProtocolCall[] memory c = _liqCall(10, 20);
        c[0].approveSpender = spender;
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ArbExecutor.TargetNotAllowed.selector, spender));
        exec.liquidate(address(token), address(token), 10, 0, c, noHops);
    }
}

/// Drives random owner/attacker actions for the invariant test.
contract Handler is Test {
    ArbExecutor exec;
    MockERC20 token;
    MockProtocol protocol;
    address owner;

    constructor(ArbExecutor e, MockERC20 t, MockProtocol p, address o) {
        exec = e;
        token = t;
        protocol = p;
        owner = o;
    }

    function liquidate(uint96 repay, uint96 reward, uint96 minProfit) external {
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol), abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), address(protocol), repay
        );
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        vm.prank(owner);
        try exec.liquidate(address(token), address(token), repay, minProfit, c, noHops) {} catch {}
    }

    function attackerCallsCallback(int256 a0, int256 a1) external {
        try exec.uniswapV3SwapCallback(a0, a1, "") {} catch {}
    }

    function attackerSendsEth(uint96 amount) external {
        vm.deal(address(this), amount);
        (bool ok,) = address(exec).call{value: amount}("");
        ok;
    }

    function attackerToggles(address t) external {
        try exec.setTarget(t, true) {} catch {}
    }
}

contract ArbExecutorInvariantTest is StdInvariant, Test {
    MockERC20 token;
    ArbExecutor exec;
    Handler handler;

    function setUp() public {
        address owner = makeAddr("owner");
        token = new MockERC20();
        MockVault vault = new MockVault();
        MockProtocol protocol = new MockProtocol();
        token.mint(address(vault), 1e30);
        address weth = address(new MockWETH());
        vm.prank(owner);
        exec = new ArbExecutor(address(vault), weth, makeAddr("receiver"));
        vm.prank(owner);
        exec.setTarget(address(protocol), true);
        handler = new Handler(exec, token, protocol, owner);
        targetContract(address(handler));
    }

    /// Whatever sequence of trades and attacks happens, nothing is left in the executor.
    function invariant_executorNeverHoldsFunds() public view {
        assertEq(token.balanceOf(address(exec)), 0);
        assertEq(address(exec).balance, 0);
    }
}
