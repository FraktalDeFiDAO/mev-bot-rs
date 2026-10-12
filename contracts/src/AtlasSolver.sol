// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {ArbExecutor} from "./ArbExecutor.sol";

/// Atlas escrow (v1.6.4) functions a solver uses to settle its gas liability.
interface IAtlasEscrow {
    function shortfall() external view returns (uint256 gasLiability, uint256 borrowLiability);
    function reconcile(uint256 maxApprovedGasSpend) external payable returns (uint256 owed);
}

interface IWETHWithdraw {
    function withdraw(uint256 wad) external;
}

/// @notice Chainlink SVR / FastLane Atlas solver: runs one of `ArbExecutor`'s liquidations inside an
/// Atlas `metacall`, right after the oracle update the auction bundles with it, and pays the auction
/// bid. Optional: it is a separate deployment and does nothing until the owner bonds ETH on Atlas
/// and the off-chain bidder (`mev-atlas`) signs solver operations for it.
///
/// Safety model (adds to `ArbExecutor`'s):
///  - `atlasSolverCall` is accepted only from the Atlas contract, only for solver operations signed
///    by `owner` (Atlas checks the signature; we check `solverOpFrom`).
///  - The operation may only start `liquidate` or `liquidateViaAave`; the usual allowlist,
///    exact-approval and `minProfit` checks all apply. `setTarget`, `rescue` etc. stay owner-only.
///  - A bid above `maxBid` reverts (default 0: no bid is allowed until the owner sets a cap).
///  - Liquidation profit goes to `profitReceiver` as before; the bid is paid from this contract's
///    ETH, unwrapping WETH if needed, so the owner keeps a WETH float here sized to `maxBid`.
///  - Atlas gas is settled through `reconcile` against the owner's bonded balance.
contract AtlasSolver is ArbExecutor {
    address public immutable atlas;
    /// Largest bid (wei of the bid token) the owner allows; 0 disables bidding.
    uint256 public maxBid;

    /// True only while `atlasSolverCall` runs: lets the self-call start a liquidation and lets WETH
    /// unwrap send ETH here.
    bool transient atlasActive;

    error NotAtlas();
    error BadSolverOp();
    error BidTooHigh(uint256 bid, uint256 cap);
    error BidTransferFailed();

    event MaxBidSet(uint256 maxBid);

    constructor(address vault_, address weth_, address profitReceiver_, address atlas_)
        ArbExecutor(vault_, weth_, profitReceiver_)
    {
        atlas = atlas_;
    }

    receive() external payable override {
        if (!liquidating && !atlasActive && !v4Active) revert Unauthorized();
    }

    function setMaxBid(uint256 cap) external {
        if (msg.sender != owner) revert NotOwner();
        maxBid = cap;
        emit MaxBidSet(cap);
    }

    function _isOperator() internal view override returns (bool) {
        return super._isOperator() || (msg.sender == address(this) && atlasActive);
    }

    /// @notice Atlas solver entry point (ISolverContract). `solverOpData` is the ABI-encoded
    /// `liquidate(...)` or `liquidateViaAave(...)` call to run on this contract.
    function atlasSolverCall(
        address solverOpFrom,
        address executionEnvironment,
        address bidToken,
        uint256 bidAmount,
        bytes calldata solverOpData,
        bytes calldata /* forwardedData */
    ) external payable {
        if (msg.sender != atlas) revert NotAtlas();
        if (solverOpFrom != owner) revert NotOwner();
        if (bidAmount > maxBid) revert BidTooHigh(bidAmount, maxBid);
        if (solverOpData.length < 4) revert BadSolverOp();
        bytes4 sel = bytes4(solverOpData[:4]);
        if (sel != ArbExecutor.liquidate.selector && sel != ArbExecutor.liquidateViaAave.selector) {
            revert BadSolverOp();
        }

        atlasActive = true;
        (bool ok, bytes memory ret) = address(this).call(solverOpData);
        if (!ok) {
            assembly {
                revert(add(ret, 32), mload(ret))
            }
        }

        // Settle Atlas gas/borrow liabilities (SolverBase does the same): native value sent with the
        // call repays any borrow, the bonded balance approves the gas.
        (uint256 gasLiability, uint256 borrowLiability) = IAtlasEscrow(atlas).shortfall();
        uint256 repay = borrowLiability < msg.value ? borrowLiability : msg.value;
        _ensureEth(repay);
        IAtlasEscrow(atlas).reconcile{value: repay}(gasLiability);

        _payBid(executionEnvironment, bidToken, bidAmount);
        atlasActive = false;
    }

    /// Unwrap WETH if the native balance is short of `amount`.
    function _ensureEth(uint256 amount) internal {
        uint256 bal = address(this).balance;
        if (amount > bal) IWETHWithdraw(address(weth)).withdraw(amount - bal);
    }

    function _payBid(address to, address bidToken, uint256 amount) internal {
        if (bidToken == address(0)) {
            _ensureEth(amount);
            (bool ok,) = to.call{value: amount}("");
            if (!ok) revert BidTransferFailed();
        } else {
            _transfer(bidToken, to, amount);
        }
    }
}
