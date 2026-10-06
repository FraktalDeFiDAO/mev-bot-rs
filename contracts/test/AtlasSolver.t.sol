// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";
import {AtlasSolver} from "../src/AtlasSolver.sol";
import {MockERC20, MockVault, MockProtocol} from "./ArbExecutorUnit.t.sol";

/// WETH9-like: deposit/withdraw, ETH held by the token.
contract MockWETH9 {
    function deposit() external payable {}

    function withdraw(uint256 wad) external {
        (bool ok,) = msg.sender.call{value: wad}("");
        require(ok, "send");
    }

    receive() external payable {}
}

/// Atlas stand-in: forwards a solver call like `metacall` does and records the settlement.
contract MockAtlas {
    uint256 public gasLiability = 5;
    uint256 public borrowLiability;
    uint256 public reconciled;
    uint256 public reconcileValue;

    function setLiabilities(uint256 g, uint256 b) external {
        gasLiability = g;
        borrowLiability = b;
    }

    function shortfall() external view returns (uint256, uint256) {
        return (gasLiability, borrowLiability);
    }

    function reconcile(uint256 maxGas) external payable returns (uint256) {
        reconciled = maxGas;
        reconcileValue = msg.value;
        return 0;
    }

    function solverCall(
        AtlasSolver solver,
        address from,
        address env,
        address bidToken,
        uint256 bid,
        bytes calldata data
    ) external payable {
        solver.atlasSolverCall{value: msg.value}(from, env, bidToken, bid, data, "");
    }
}

/// The execution environment: receives the bid.
contract MockEnv {
    receive() external payable {}
}

contract AtlasSolverTest is Test {
    address owner = makeAddr("owner");
    address receiver = makeAddr("receiver");
    MockERC20 token;
    MockVault vault;
    MockProtocol protocol;
    MockWETH9 weth;
    MockAtlas atlas;
    MockEnv env;
    AtlasSolver solver;

    function setUp() public {
        token = new MockERC20();
        vault = new MockVault();
        protocol = new MockProtocol();
        weth = new MockWETH9();
        atlas = new MockAtlas();
        env = new MockEnv();
        token.mint(address(vault), 1e30);
        vm.prank(owner);
        solver = new AtlasSolver(address(vault), address(weth), receiver, address(atlas));
        vm.startPrank(owner);
        solver.setTarget(address(protocol), true);
        solver.setMaxBid(1 ether);
        vm.stopPrank();
    }

    function _op(uint256 repay, uint256 reward) internal view returns (bytes memory) {
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol), abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), address(protocol), repay
        );
        ArbExecutor.Hop[] memory noHops = new ArbExecutor.Hop[](0);
        return abi.encodeCall(ArbExecutor.liquidate, (address(token), address(token), 1000, 0, c, noHops));
    }

    function test_liquidatesAndPaysBidFromEth() public {
        vm.deal(address(solver), 1 ether);
        atlas.solverCall(solver, owner, address(env), address(0), 0.2 ether, _op(1000, 1100));
        assertEq(address(env).balance, 0.2 ether, "bid paid");
        assertEq(token.balanceOf(receiver), 100, "profit to receiver");
        assertEq(atlas.reconciled(), 5, "gas liability reconciled");
    }

    function test_unwrapsWethWhenEthIsShort() public {
        vm.deal(address(weth), 1 ether); // the WETH contract's ETH backing the float
        atlas.solverCall(solver, owner, address(env), address(0), 0.3 ether, _op(1000, 1100));
        assertEq(address(env).balance, 0.3 ether, "bid paid from unwrapped WETH");
    }

    function test_borrowRepaidFromCallValue() public {
        vm.deal(address(weth), 1 ether); // float is WETH: the liquidation wraps native ETH
        atlas.setLiabilities(7, 40);
        atlas.solverCall{value: 100}(solver, owner, address(env), address(0), 0, _op(1000, 1100));
        assertEq(atlas.reconcileValue(), 40, "repays min(borrow, msg.value)");
        assertEq(atlas.reconciled(), 7);
    }

    function test_rejectsEveryoneButAtlas() public {
        vm.expectRevert(AtlasSolver.NotAtlas.selector);
        solver.atlasSolverCall(owner, address(env), address(0), 0, _op(1000, 1100), "");
    }

    function test_rejectsOtherSolverOpSigners() public {
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        atlas.solverCall(solver, makeAddr("stranger"), address(env), address(0), 0, _op(1000, 1100));
    }

    function test_bidCapDefaultsToZeroAndIsEnforced() public {
        vm.prank(owner);
        solver.setMaxBid(0.1 ether);
        vm.deal(address(solver), 1 ether);
        vm.expectRevert(abi.encodeWithSelector(AtlasSolver.BidTooHigh.selector, 0.2 ether, 0.1 ether));
        atlas.solverCall(solver, owner, address(env), address(0), 0.2 ether, _op(1000, 1100));
    }

    function test_onlyLiquidationSelectorsAllowed() public {
        bytes memory bad = abi.encodeCall(ArbExecutor.setTarget, (address(this), true));
        vm.expectRevert(AtlasSolver.BadSolverOp.selector);
        atlas.solverCall(solver, owner, address(env), address(0), 0, bad);
        vm.expectRevert(AtlasSolver.BadSolverOp.selector);
        atlas.solverCall(solver, owner, address(env), address(0), 0, hex"01");
    }

    function test_unprofitableLiquidationRevertsTheWholeSolverCall() public {
        vm.deal(address(solver), 1 ether);
        // reward < repay: the executor's profit check reverts and so does the solver op.
        vm.expectRevert();
        atlas.solverCall(solver, owner, address(env), address(0), 0.1 ether, _op(1000, 900));
        assertEq(address(env).balance, 0, "no bid paid");
    }

    function test_ethOnlyAcceptedDuringSolverCall() public {
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(solver).call{value: 1}("");
        assertFalse(ok, "stray ETH rejected");
    }

    function test_onlyOwnerStillOwnsTheExecutorSurface() public {
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        solver.setMaxBid(1);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        solver.liquidate(
            address(token), address(token), 1, 0, new ArbExecutor.ProtocolCall[](0), new ArbExecutor.Hop[](0)
        );
    }
}
