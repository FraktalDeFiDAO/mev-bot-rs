// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;
import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "audit-contracts/src/ArbExecutor.sol";
import {AtlasSolver} from "audit-contracts/src/AtlasSolver.sol";
import {MockERC20, MockVault, MockProtocol} from "audit-contracts/test/ArbExecutorUnit.t.sol";
import {MockAtlas, MockEnv} from "audit-contracts/test/AtlasSolver.t.sol";

// Unlike the existing MockWETH9, withdrawal requires owned wrapped tokens.
contract BalanceCheckedWETH is MockERC20 {
    function deposit() external payable {
        balanceOf[msg.sender] += msg.value;
    }

    function withdraw(uint256 amount) external {
        require(balanceOf[msg.sender] >= amount, "insufficient WETH");
        balanceOf[msg.sender] -= amount;
        (bool ok,) = msg.sender.call{value: amount}("");
        require(ok, "ETH transfer");
    }
}

contract AtlasBalanceAuditTest is Test {
    address owner = address(0x100);
    address receiver = address(0x200);
    BalanceCheckedWETH weth;
    MockVault vault;
    MockProtocol protocol;
    MockAtlas atlas;
    MockEnv env;
    AtlasSolver solver;

    function setUp() public {
        weth = new BalanceCheckedWETH();
        vault = new MockVault();
        protocol = new MockProtocol();
        atlas = new MockAtlas();
        env = new MockEnv();
        vm.prank(owner);
        solver = new AtlasSolver(address(vault), address(weth), receiver, address(atlas));
        vm.startPrank(owner);
        solver.setTarget(address(protocol), true);
        solver.setMaxBid(1 ether);
        vm.stopPrank();
        weth.mint(address(vault), 100 ether);
        weth.mint(address(solver), 1 ether); // documented bidding float
        vm.deal(address(weth), 100 ether);
    }

    function op(uint256 reward, uint256 minProfit) internal view returns (bytes memory) {
        ArbExecutor.ProtocolCall[] memory calls = new ArbExecutor.ProtocolCall[](1);
        calls[0] = ArbExecutor.ProtocolCall(
            address(protocol),
            abi.encodeCall(MockProtocol.liquidate, (MockERC20(address(weth)), 10 ether, reward)),
            address(protocol),
            10 ether
        );
        return abi.encodeCall(
            ArbExecutor.liquidate, (address(weth), address(weth), 10 ether, minProfit, calls, new ArbExecutor.Hop[](0))
        );
    }

    function test_documentedFloatSweptAndBidFails() public {
        vm.expectRevert("insufficient WETH");
        atlas.solverCall(solver, owner, address(env), address(0), 0.2 ether, op(11 ether, 0.8 ether));
        assertEq(weth.balanceOf(address(solver)), 1 ether); // reverted, no theft
    }

    function test_zeroBidReportsInitialFloatAsProfit() public {
        atlas.solverCall(solver, owner, address(env), address(0), 0, op(11 ether, 0.8 ether));
        assertEq(weth.balanceOf(receiver), 2 ether); // only 1 ether was earned
        assertEq(weth.balanceOf(address(solver)), 0);
    }

    function test_prefundedFloatLetsLosingTradePassProfitFloor() public {
        atlas.solverCall(solver, owner, address(env), address(0), 0, op(9.5 ether, 0.1 ether));
        assertEq(weth.balanceOf(receiver), 0.5 ether); // trade lost half of owner's float
        assertEq(weth.balanceOf(address(solver)), 0);
    }
}
