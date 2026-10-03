// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

interface IERC20 {
    function balanceOf(address) external view returns (uint256);
}

interface IBalancerVault {
    function flashLoan(
        address recipient,
        address[] calldata tokens,
        uint256[] calldata amounts,
        bytes calldata userData
    ) external;
}

interface IUniV2Pair {
    function getReserves() external view returns (uint112, uint112, uint32);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

interface ICamelotPair {
    function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
}

interface IUniV3Pool {
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);
}

/// @notice Flash-loan executor for atomic arbitrage and protocol-neutral liquidations.
/// Safety model:
///  - Only `owner` can start a trade; flash-loan and swap callbacks are accepted only while that trade is
///    in progress and only from the exact vault/pool we called.
///  - Holds no funds between transactions: the loan is repaid and all profit is sent to `profitReceiver`
///    inside the same transaction.
///  - Liquidations only call owner-allowlisted protocol contracts. The only approvals ever granted are
///    exact amounts of the flash-borrowed debt asset to allowlisted spenders, reset to zero right after.
///  - Reverts unless the strategy returns at least `loan + fee + minProfit`.
contract ArbExecutor {
    /// Hop kinds
    uint8 internal constant UNI_V2 = 0; // Uniswap V2 / Sushi V2 style pair with fixed fee `feeBps`
    uint8 internal constant CAMELOT_V2 = 1; // Camelot V2 pair (directional fees, uses pair.getAmountOut)
    uint8 internal constant UNI_V3 = 2; // Uniswap V3 / Sushi V3 / PancakeSwap V3 / Algebra (Camelot V3) pools

    /// Flash-loan modes (first field of the vault userData).
    uint8 internal constant MODE_ARB = 0;
    uint8 internal constant MODE_LIQUIDATE = 1;

    uint160 internal constant MIN_SQRT_RATIO_PLUS_ONE = 4295128740;
    uint160 internal constant MAX_SQRT_RATIO_MINUS_ONE = 1461446703485210103287273052203988822378723970341;

    /// A protocol call made during a liquidation (e.g. Aave `liquidationCall`, Comet `absorb`).
    struct ProtocolCall {
        address target;
        bytes data;
        address approveSpender;
        uint256 approveAmount;
    }

    struct Hop {
        uint8 kind;
        address pool;
        address tokenIn;
        address tokenOut;
        bool zeroForOne;
        uint16 feeBps;
    }

    IBalancerVault public immutable vault;
    address public immutable owner;
    address public immutable profitReceiver;

    /// Liquidation call targets / approval spenders the owner has vetted.
    mapping(address => bool) public allowedTarget;

    bool transient flashActive;
    address transient expectedPool;
    address transient expectedTokenIn;
    uint256 transient expectedMaxIn;
    uint256 transient lastProfit;

    error NotOwner();
    error Unauthorized();
    error NotProfitable(uint256 balance, uint256 required);
    error BadRoute();
    error TransferFailed();
    error TargetNotAllowed(address target);

    event TargetSet(address indexed target, bool allowed);

    constructor(address vault_, address profitReceiver_) {
        vault = IBalancerVault(vault_);
        owner = msg.sender;
        profitReceiver = profitReceiver_;
    }

    /// @notice Allow or revoke a liquidation call target / approval spender.
    function setTarget(address target, bool allowed) external {
        if (msg.sender != owner) revert NotOwner();
        allowedTarget[target] = allowed;
        emit TargetSet(target, allowed);
    }

    /// @notice Borrow `amount` of `token`, run `hops`, repay, and send profit to `profitReceiver`.
    /// @return profit amount of `token` sent to `profitReceiver`
    function execute(address token, uint256 amount, uint256 minProfit, Hop[] calldata hops)
        external
        returns (uint256 profit)
    {
        if (msg.sender != owner) revert NotOwner();
        if (hops.length == 0 || hops[0].tokenIn != token || hops[hops.length - 1].tokenOut != token) revert BadRoute();

        address[] memory tokens = new address[](1);
        tokens[0] = token;
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = amount;

        flashActive = true;
        vault.flashLoan(address(this), tokens, amounts, abi.encode(MODE_ARB, abi.encode(minProfit, hops)));
        flashActive = false;
        profit = lastProfit;
    }

    /// @notice Liquidate through any allowlisted protocol: flash-borrow `flashAmount` of `debt`, make
    /// `calls` (each with an optional exact approval of `debt`), swap the seized `collateral` back to
    /// `debt` via `hops`, repay the loan, and send the profit (in `debt`) to `profitReceiver`.
    /// `hops` must be empty when collateral == debt.
    /// @return profit amount of `debt` sent to `profitReceiver`
    function liquidate(
        address debt,
        address collateral,
        uint256 flashAmount,
        uint256 minProfit,
        ProtocolCall[] calldata calls,
        Hop[] calldata hops
    ) external returns (uint256 profit) {
        if (msg.sender != owner) revert NotOwner();
        if (calls.length == 0) revert BadRoute();
        if (collateral == debt) {
            if (hops.length != 0) revert BadRoute();
        } else if (hops.length == 0 || hops[0].tokenIn != collateral || hops[hops.length - 1].tokenOut != debt) {
            revert BadRoute();
        }

        address[] memory tokens = new address[](1);
        tokens[0] = debt;
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = flashAmount;

        flashActive = true;
        vault.flashLoan(
            address(this), tokens, amounts, abi.encode(MODE_LIQUIDATE, abi.encode(collateral, minProfit, calls, hops))
        );
        flashActive = false;
        profit = lastProfit;
    }

    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external {
        if (msg.sender != address(vault) || !flashActive) revert Unauthorized();
        flashActive = false; // single use per execute()

        (uint8 mode, bytes memory inner) = abi.decode(userData, (uint8, bytes));
        address token = tokens[0];
        uint256 owed = amounts[0] + feeAmounts[0];
        uint256 minProfit;
        if (mode == MODE_ARB) minProfit = _arbitrage(inner, amounts[0]);
        else if (mode == MODE_LIQUIDATE) minProfit = _liquidate(inner, token);
        else revert BadRoute();

        uint256 balance = IERC20(token).balanceOf(address(this));
        if (balance < owed + minProfit) revert NotProfitable(balance, owed + minProfit);

        _transfer(token, address(vault), owed);
        uint256 profit = balance - owed;
        if (profit > 0) _transfer(token, profitReceiver, profit);
        lastProfit = profit;
    }

    function _arbitrage(bytes memory inner, uint256 amount) internal returns (uint256 minProfit) {
        Hop[] memory hops;
        (minProfit, hops) = abi.decode(inner, (uint256, Hop[]));
        _runHops(hops, amount);
    }

    function _liquidate(bytes memory inner, address debt) internal returns (uint256 minProfit) {
        (address collateral, uint256 minProfit_, ProtocolCall[] memory calls, Hop[] memory hops) =
            abi.decode(inner, (address, uint256, ProtocolCall[], Hop[]));
        for (uint256 i; i < calls.length; ++i) {
            ProtocolCall memory c = calls[i];
            if (!allowedTarget[c.target]) revert TargetNotAllowed(c.target);
            if (c.approveAmount > 0) {
                if (!allowedTarget[c.approveSpender]) revert TargetNotAllowed(c.approveSpender);
                _approve(debt, c.approveSpender, c.approveAmount);
            }
            (bool ok, bytes memory ret) = c.target.call(c.data);
            if (!ok) {
                assembly {
                    revert(add(ret, 32), mload(ret))
                }
            }
            if (c.approveAmount > 0) _approve(debt, c.approveSpender, 0);
        }
        if (collateral != debt) _runHops(hops, IERC20(collateral).balanceOf(address(this)));
        return minProfit_;
    }

    function _runHops(Hop[] memory hops, uint256 amountIn) internal {
        for (uint256 i; i < hops.length; ++i) {
            if (i > 0 && hops[i].tokenIn != hops[i - 1].tokenOut) revert BadRoute();
            amountIn = _swap(hops[i], amountIn);
        }
    }

    /// @dev Uniswap V3 / Sushi V3.
    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata) external {
        _payPool(amount0Delta, amount1Delta);
    }

    /// @dev PancakeSwap V3 — same swap() as Uniswap V3, different callback name.
    function pancakeV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata) external {
        _payPool(amount0Delta, amount1Delta);
    }

    /// @dev Algebra (Camelot V3) — same swap() shape as Uniswap V3, different callback name.
    function algebraSwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata) external {
        _payPool(amount0Delta, amount1Delta);
    }

    /// Pay the pool we're swapping on, only during that swap, never more than the hop's input.
    function _payPool(int256 amount0Delta, int256 amount1Delta) internal {
        if (msg.sender != expectedPool) revert Unauthorized();
        uint256 owed = uint256(amount0Delta > 0 ? amount0Delta : amount1Delta);
        if (owed > expectedMaxIn) revert Unauthorized();
        expectedPool = address(0);
        _transfer(expectedTokenIn, msg.sender, owed);
    }

    /// @notice Recover tokens sent here by mistake. Funds can only go to `profitReceiver`.
    function rescue(address token) external {
        if (msg.sender != owner) revert NotOwner();
        _transfer(token, profitReceiver, IERC20(token).balanceOf(address(this)));
    }

    function _swap(Hop memory hop, uint256 amountIn) internal returns (uint256 amountOut) {
        if (hop.kind == UNI_V3) {
            expectedPool = hop.pool;
            expectedTokenIn = hop.tokenIn;
            expectedMaxIn = amountIn;
            (int256 a0, int256 a1) = IUniV3Pool(hop.pool)
                .swap(
                    address(this),
                    hop.zeroForOne,
                    int256(amountIn),
                    hop.zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE,
                    ""
                );
            if (expectedPool != address(0)) revert Unauthorized(); // callback must have run
            amountOut = uint256(-(hop.zeroForOne ? a1 : a0));
        } else {
            if (hop.kind == CAMELOT_V2) {
                amountOut = ICamelotPair(hop.pool).getAmountOut(amountIn, hop.tokenIn);
            } else if (hop.kind == UNI_V2) {
                (uint112 r0, uint112 r1,) = IUniV2Pair(hop.pool).getReserves();
                (uint256 rIn, uint256 rOut) = hop.zeroForOne ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
                uint256 inWithFee = amountIn * (10_000 - hop.feeBps);
                amountOut = (inWithFee * rOut) / (rIn * 10_000 + inWithFee);
            } else {
                revert BadRoute();
            }
            _transfer(hop.tokenIn, hop.pool, amountIn);
            (uint256 out0, uint256 out1) = hop.zeroForOne ? (uint256(0), amountOut) : (amountOut, uint256(0));
            IUniV2Pair(hop.pool).swap(out0, out1, address(this), "");
        }
    }

    function _approve(address token, address spender, uint256 amount) internal {
        (bool ok, bytes memory data) = token.call(abi.encodeWithSelector(0x095ea7b3, spender, amount));
        if (!ok || (data.length != 0 && !abi.decode(data, (bool)))) revert TransferFailed();
    }

    function _transfer(address token, address to, uint256 amount) internal {
        (bool ok, bytes memory data) = token.call(abi.encodeWithSelector(0xa9059cbb, to, amount));
        if (!ok || (data.length != 0 && !abi.decode(data, (bool)))) revert TransferFailed();
    }
}
