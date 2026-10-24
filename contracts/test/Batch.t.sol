// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";
import {MockERC20, MockVault, MockProtocol, MockWETH} from "./ArbExecutorUnit.t.sol";

/// A protocol that tries to start a trade on the executor while a batch is running.
contract HijackProtocol {
    ArbExecutor public exec;

    function set(ArbExecutor e) external {
        exec = e;
    }

    function liquidate(MockERC20 debt, uint256, uint256) external {
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](0);
        ArbExecutor.Hop[] memory h = new ArbExecutor.Hop[](0);
        exec.liquidate(address(debt), address(debt), 1, 0, c, h);
    }
}

/// `batch`: several trades in one tx, each isolated; nobody but the owner can start one, and a
/// protocol cannot use the batch window to start trades of its own.
contract BatchTest is Test {
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
        address weth = address(new MockWETH());
        vm.prank(owner);
        exec = new ArbExecutor(address(vault), weth, receiver);
        vm.prank(owner);
        exec.setTarget(address(protocol), true);
    }

    function _liq(uint256 repay, uint256 reward) internal view returns (bytes memory) {
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol), abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), address(protocol), repay
        );
        ArbExecutor.Hop[] memory none = new ArbExecutor.Hop[](0);
        return abi.encodeCall(ArbExecutor.liquidate, (address(token), address(token), repay, 0, c, none));
    }

    function test_runsEveryItemAndPaysEachProfit() public {
        bytes[] memory calls = new bytes[](3);
        calls[0] = _liq(1000, 1100);
        calls[1] = _liq(2000, 2300);
        calls[2] = _liq(500, 520);
        vm.prank(owner);
        bool[] memory ok = exec.batch(calls);
        assertTrue(ok[0] && ok[1] && ok[2]);
        assertEq(token.balanceOf(receiver), 100 + 300 + 20);
    }

    function test_oneFailingItemDoesNotUndoTheOthers() public {
        bytes[] memory calls = new bytes[](3);
        calls[0] = _liq(1000, 1100); // +100
        calls[1] = _liq(1000, 900); // loses: the item reverts on its own profit check
        calls[2] = _liq(500, 600); // +100
        vm.prank(owner);
        vm.expectEmit(true, true, false, true);
        emit ArbExecutor.BatchItem(1, ArbExecutor.liquidate.selector, false);
        bool[] memory ok = exec.batch(calls);
        assertTrue(ok[0] && !ok[1] && ok[2]);
        assertEq(token.balanceOf(receiver), 200, "both good items were paid");
    }

    function test_allItemsFailingRevertsTheBatch() public {
        bytes[] memory calls = new bytes[](2);
        calls[0] = _liq(1000, 900);
        calls[1] = _liq(1000, 800);
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.NothingExecuted.selector);
        exec.batch(calls);
        assertEq(token.balanceOf(receiver), 0);
    }

    function test_onlyOwnerCanBatch() public {
        bytes[] memory calls = new bytes[](1);
        calls[0] = _liq(1000, 1100);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.batch(calls);
    }

    function test_rejectsEmptyOversizedAndForeignCalls() public {
        vm.startPrank(owner);
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.batch(new bytes[](0));

        bytes[] memory big = new bytes[](9);
        for (uint256 i; i < 9; ++i) {
            big[i] = _liq(1000, 1100);
        }
        vm.expectRevert(ArbExecutor.BatchTooLarge.selector);
        exec.batch(big);

        bytes[] memory bad = new bytes[](1);
        bad[0] = abi.encodeCall(ArbExecutor.setTarget, (address(this), true)); // not a trade
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.batch(bad);

        bytes[] memory short_ = new bytes[](1);
        short_[0] = hex"010203";
        vm.expectRevert(ArbExecutor.BadRoute.selector);
        exec.batch(short_);
        vm.stopPrank();
    }

    function test_aProtocolCannotStartATradeDuringTheBatch() public {
        HijackProtocol evil = new HijackProtocol();
        evil.set(exec);
        vm.prank(owner);
        exec.setTarget(address(evil), true);
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(evil), abi.encodeCall(HijackProtocol.liquidate, (token, 1000, 1100)), address(evil), 1000
        );
        ArbExecutor.Hop[] memory none = new ArbExecutor.Hop[](0);
        bytes[] memory calls = new bytes[](1);
        calls[0] = abi.encodeCall(ArbExecutor.liquidate, (address(token), address(token), 1000, 0, c, none));
        vm.prank(owner);
        vm.expectRevert(ArbExecutor.NothingExecuted.selector); // the hijack item failed, nothing ran
        exec.batch(calls);
    }

    function test_batchingFlagDoesNotLeakAfterTheBatch() public {
        bytes[] memory calls = new bytes[](1);
        calls[0] = _liq(1000, 1100);
        vm.prank(owner);
        exec.batch(calls);
        // Outside a batch a stranger (or the contract itself via a stranger) still cannot trade.
        ArbExecutor.Hop[] memory none = new ArbExecutor.Hop[](0);
        vm.expectRevert(ArbExecutor.NotOwner.selector);
        exec.liquidate(address(token), address(token), 1, 0, new ArbExecutor.ProtocolCall[](0), none);
    }

    /// Gas a batch adds per item over calling the same trade directly (self-call, selector check,
    /// BatchItem event, loop, calldata copy), for 1-4 items at several calldata sizes; logged as
    /// (item calldata bytes, overhead per item). The engine's per-item allowance comes from these.
    function test_measureBatchOverheadPerItem() public {
        for (uint256 pad = 0; pad <= 8192; pad = pad == 0 ? 512 : pad * 4) {
            bytes memory item = _liqPadded(1000, 1100, pad);
            uint256 snap = vm.snapshotState();
            vm.prank(owner);
            uint256 g0 = gasleft();
            (bool okd,) = address(exec).call(item);
            uint256 direct = g0 - gasleft();
            assertTrue(okd);
            vm.revertToState(snap);
            for (uint256 n = 1; n <= 4; n *= 2) {
                bytes[] memory calls = new bytes[](n);
                for (uint256 i; i < n; ++i) {
                    calls[i] = item;
                }
                vm.prank(owner);
                uint256 g = gasleft();
                exec.batch(calls);
                uint256 used = g - gasleft();
                vm.revertToState(snap);
                int256 per = (int256(used) - int256(n * direct)) / int256(n);
                // The engine allows PER_ITEM_OVERHEAD_GAS (crates/mev-engine/src/batch.rs) per item:
                // a higher measurement means that allowance must be re-measured.
                assertLe(per, 6_753);
                emit log_named_string(
                    string.concat("bytes ", vm.toString(item.length), " n ", vm.toString(n)), vm.toString(per)
                );
            }
        }
    }

    function _liqPadded(uint256 repay, uint256 reward, uint256 pad) internal view returns (bytes memory) {
        ArbExecutor.ProtocolCall[] memory c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol),
            bytes.concat(abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), new bytes(pad)),
            address(protocol),
            repay
        );
        ArbExecutor.Hop[] memory none = new ArbExecutor.Hop[](0);
        return abi.encodeCall(ArbExecutor.liquidate, (address(token), address(token), repay, 0, c, none));
    }

    function _calls(uint256 repay, uint256 reward) internal view returns (ArbExecutor.ProtocolCall[] memory c) {
        c = new ArbExecutor.ProtocolCall[](1);
        c[0] = ArbExecutor.ProtocolCall(
            address(protocol), abi.encodeCall(MockProtocol.liquidate, (token, repay, reward)), address(protocol), repay
        );
    }
}
