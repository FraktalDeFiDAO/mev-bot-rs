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

/// Aave V3 pool: single-asset flash loan (premium 5 bps on Arbitrum, pulled back via transferFrom).
interface IAaveFlashPool {
    function flashLoanSimple(
        address receiver,
        address asset,
        uint256 amount,
        bytes calldata params,
        uint16 referralCode
    ) external;
}

interface IWETH {
    function deposit() external payable;
    function withdraw(uint256 wad) external;
}

/// Uniswap V4 PoolManager (singleton). Currencies are plain addresses on the wire (address(0) =
/// native ETH); `BalanceDelta` is an int256 with amount0 in the upper and amount1 in the lower 128 bits.
interface IPoolManager {
    struct PoolKey {
        address currency0;
        address currency1;
        uint24 fee;
        int24 tickSpacing;
        address hooks;
    }

    struct SwapParams {
        bool zeroForOne;
        int256 amountSpecified; // negative = exact input
        uint160 sqrtPriceLimitX96;
    }

    function unlock(bytes calldata data) external returns (bytes memory);
    function swap(PoolKey memory key, SwapParams memory params, bytes calldata hookData)
        external
        returns (int256 swapDelta);
    function sync(address currency) external;
    function settle() external payable returns (uint256 paid);
    function take(address currency, address to, uint256 amount) external;
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
    /// Uniswap V4 hookless pool, both currencies ERC20. The hop's `pool` field carries the pool's tick
    /// spacing (1..32767) instead of an address; the key is rebuilt from the tokens, `feeBps` (the pool
    /// fee, hundredths of a bip) and that spacing, so a wrong hop reverts with PoolNotInitialized.
    uint8 internal constant UNI_V4 = 3;
    /// Same, for a pool whose currency is native ETH: the hop's WETH side is unwrapped / wrapped here.
    uint8 internal constant UNI_V4_NATIVE = 4;

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
    /// Native ETH received from a protocol during a liquidation is wrapped into this.
    IWETH public immutable weth;
    address public immutable owner;
    address public immutable profitReceiver;

    /// Liquidation call targets / approval spenders the owner has vetted.
    mapping(address => bool) public allowedTarget;

    bool transient flashActive;
    /// True only while liquidation protocol calls run: the only time ETH may be received.
    bool transient liquidating;
    address transient expectedPool;
    address transient expectedTokenIn;
    uint256 transient expectedMaxIn;
    uint256 transient lastProfit;
    /// The pool a flash swap was started on; its callback (and only that one) runs the route.
    address transient flashPool;
    /// The Aave pool an Aave-funded liquidation borrowed from; only its callback is accepted.
    address transient aaveFlashPool;
    /// True only while `batch` runs its items: lets each item's self-call start its trade.
    bool transient batching;
    /// True only while a V4 hop runs: the only time the PoolManager may call back and ETH may arrive.
    bool transient v4Active;
    /// Uniswap V4 PoolManager (address(0) = V4 hops disabled). Owner-set; V4 hops hand it tokens.
    address public poolManager;

    error NotOwner();
    error Unauthorized();
    error NotProfitable(uint256 balance, uint256 required);
    error BadRoute();
    error TransferFailed();
    error TargetNotAllowed(address target);
    error NothingExecuted();
    error BatchTooLarge();

    event TargetSet(address indexed target, bool allowed);
    event PoolManagerSet(address indexed poolManager);
    /// One item of a `batch`: its position, selector and whether it executed.
    event BatchItem(uint256 indexed index, bytes4 indexed selector, bool ok);

    constructor(address vault_, address weth_, address profitReceiver_) {
        vault = IBalancerVault(vault_);
        weth = IWETH(weth_);
        owner = msg.sender;
        profitReceiver = profitReceiver_;
    }

    /// @dev Protocols that pay seized collateral in native ETH (e.g. Fluid ETH vaults). Rejected
    /// outside a liquidation so ETH can't be parked here.
    receive() external payable virtual {
        if (!liquidating && !v4Active) revert Unauthorized();
    }

    /// @dev Who may start a liquidation: the owner. A subclass can additionally admit itself
    /// during a call it authenticated (see `AtlasSolver`).
    function _isOperator() internal view virtual returns (bool) {
        return msg.sender == owner || (msg.sender == address(this) && batching);
    }

    /// @notice Set the Uniswap V4 PoolManager V4 hops use (address(0) disables them).
    function setPoolManager(address pm) external {
        if (msg.sender != owner) revert NotOwner();
        poolManager = pm;
        emit PoolManagerSet(pm);
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
        if (!_isOperator()) revert NotOwner();
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

    /// @notice Flash swap: swap `amount` of the route's token on the first hop's V3-style pool, which
    /// pays out before being paid; the remaining hops run inside its callback and the pool is repaid
    /// from their proceeds. No lender, no loan fee, no lender-balance cap. Distinct pools only (the
    /// first pool is locked during its own callback).
    /// @return profit amount of the route token sent to `profitReceiver`
    function executeFlashSwap(uint256 amount, uint256 minProfit, Hop[] calldata hops)
        external
        returns (uint256 profit)
    {
        if (!_isOperator()) revert NotOwner();
        if (hops.length < 2 || hops[0].kind != UNI_V3 || hops[0].tokenIn != hops[hops.length - 1].tokenOut) {
            revert BadRoute();
        }
        Hop calldata first = hops[0];
        flashPool = first.pool;
        IUniV3Pool(first.pool)
            .swap(
                address(this),
                first.zeroForOne,
                int256(amount),
                first.zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE,
                abi.encode(minProfit, hops)
            );
        if (flashPool != address(0)) revert Unauthorized(); // callback must have run
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
        if (!_isOperator()) revert NotOwner();
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

    /// @notice Like `liquidate`, but funded by an Aave V3 `flashLoanSimple` from `aavePool` instead of
    /// Balancer: deep liquidity for large positions (Balancer's Arbitrum balances cap loans), at a
    /// 5 bps premium. Same allowlist, route checks and profit accounting.
    /// @return profit amount of `debt` sent to `profitReceiver`
    function liquidateViaAave(
        address aavePool,
        address debt,
        address collateral,
        uint256 flashAmount,
        uint256 minProfit,
        ProtocolCall[] calldata calls,
        Hop[] calldata hops
    ) external returns (uint256 profit) {
        if (!_isOperator()) revert NotOwner();
        if (calls.length == 0) revert BadRoute();
        if (collateral == debt) {
            if (hops.length != 0) revert BadRoute();
        } else if (hops.length == 0 || hops[0].tokenIn != collateral || hops[hops.length - 1].tokenOut != debt) {
            revert BadRoute();
        }
        aaveFlashPool = aavePool;
        IAaveFlashPool(aavePool)
            .flashLoanSimple(address(this), debt, flashAmount, abi.encode(collateral, minProfit, calls, hops), 0);
        if (aaveFlashPool != address(0)) revert Unauthorized(); // callback must have run
        profit = lastProfit;
    }

    /// Most trades one `batch` call may carry (bounds the gas an owner mistake can burn).
    uint256 internal constant MAX_BATCH = 8;

    /// @notice Run several trades in one transaction: each item is ABI-encoded calldata for
    /// `execute`, `executeFlashSwap`, `liquidate` or `liquidateViaAave` on this contract. Items run
    /// one after another, each in its own self-call, so an item that reverts (a lost race, a
    /// failed `minProfit`) only undoes itself and the rest still execute; every item keeps its own
    /// route checks, allowlist, exact approvals and `minProfit`. Reverts with `NothingExecuted` if
    /// no item succeeded. Each item's profit is sent to `profitReceiver` as usual.
    /// @return ok per item: whether it executed
    function batch(bytes[] calldata calls) external returns (bool[] memory ok) {
        if (msg.sender != owner) revert NotOwner();
        uint256 n = calls.length;
        if (n == 0) revert BadRoute();
        if (n > MAX_BATCH) revert BatchTooLarge();
        ok = new bool[](n);
        bool any;
        batching = true;
        for (uint256 i; i < n; ++i) {
            if (calls[i].length < 4) revert BadRoute();
            bytes4 sel = bytes4(calls[i][:4]);
            if (
                sel != this.execute.selector && sel != this.executeFlashSwap.selector && sel != this.liquidate.selector
                    && sel != this.liquidateViaAave.selector
            ) revert BadRoute();
            (bool success,) = address(this).call(calls[i]);
            ok[i] = success;
            any = any || success;
            emit BatchItem(i, sel, success);
        }
        batching = false;
        if (!any) revert NothingExecuted();
    }

    /// Aave V3 flash-loan callback: only the pool `liquidateViaAave` borrowed from, once, for a loan
    /// this contract started. The pool pulls `amount + premium` after this returns.
    function executeOperation(address asset, uint256 amount, uint256 premium, address initiator, bytes calldata params)
        external
        returns (bool)
    {
        if (msg.sender != aaveFlashPool || aaveFlashPool == address(0) || initiator != address(this)) {
            revert Unauthorized();
        }
        aaveFlashPool = address(0);
        uint256 minProfit = _liquidate(params, asset);
        uint256 owed = amount + premium;
        uint256 balance = IERC20(asset).balanceOf(address(this));
        if (balance < owed + minProfit) revert NotProfitable(balance, owed + minProfit);
        uint256 profit = balance - owed;
        if (profit > 0) _transfer(asset, profitReceiver, profit);
        _approve(asset, msg.sender, owed);
        lastProfit = profit;
        return true;
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
        liquidating = true;
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
        liquidating = false;
        // Native ETH collateral -> WETH, so the swap route and accounting stay ERC20-only.
        if (address(this).balance > 0) weth.deposit{value: address(this).balance}();
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
    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        if (data.length == 0) _payPool(amount0Delta, amount1Delta);
        else _flashSwapCallback(amount0Delta, amount1Delta, data);
    }

    /// @dev PancakeSwap V3 — same swap() as Uniswap V3, different callback name.
    function pancakeV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        if (data.length == 0) _payPool(amount0Delta, amount1Delta);
        else _flashSwapCallback(amount0Delta, amount1Delta, data);
    }

    /// @dev Algebra (Camelot V3) — same swap() shape as Uniswap V3, different callback name.
    function algebraSwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        if (data.length == 0) _payPool(amount0Delta, amount1Delta);
        else _flashSwapCallback(amount0Delta, amount1Delta, data);
    }

    /// Inside a flash swap: run the rest of the route on what the pool paid out, repay the pool,
    /// send the profit. Only the pool `executeFlashSwap` started, once.
    function _flashSwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) internal {
        if (msg.sender != flashPool || flashPool == address(0)) revert Unauthorized();
        flashPool = address(0);
        (uint256 minProfit, Hop[] memory hops) = abi.decode(data, (uint256, Hop[]));
        Hop memory first = hops[0];
        uint256 owed = uint256(first.zeroForOne ? amount0Delta : amount1Delta);
        uint256 received = uint256(-(first.zeroForOne ? amount1Delta : amount0Delta));
        uint256 amountIn = received;
        for (uint256 i = 1; i < hops.length; ++i) {
            if (hops[i].tokenIn != hops[i - 1].tokenOut || hops[i].pool == first.pool) revert BadRoute();
            amountIn = _swap(hops[i], amountIn);
        }
        address token = first.tokenIn;
        uint256 balance = IERC20(token).balanceOf(address(this));
        if (balance < owed + minProfit) revert NotProfitable(balance, owed + minProfit);
        _transfer(token, msg.sender, owed);
        uint256 profit = balance - owed;
        if (profit > 0) _transfer(token, profitReceiver, profit);
        lastProfit = profit;
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

    /// One V4 swap, as carried through `unlock` to `unlockCallback`.
    struct V4Swap {
        uint8 kind;
        address tokenIn;
        address tokenOut;
        uint24 fee;
        int24 tickSpacing;
        uint256 amountIn;
    }

    /// Swap `amountIn` of `hop.tokenIn` through a Uniswap V4 pool: unlock the manager, swap, settle
    /// the input, take the output (flash accounting nets to zero or the unlock reverts).
    function _swapV4(Hop memory hop, uint256 amountIn) internal returns (uint256 amountOut) {
        address pm = poolManager;
        uint160 spacing = uint160(hop.pool);
        if (pm == address(0) || spacing == 0 || spacing > 32767) revert BadRoute();
        v4Active = true;
        bytes memory ret = IPoolManager(pm)
            .unlock(
                abi.encode(V4Swap(hop.kind, hop.tokenIn, hop.tokenOut, hop.feeBps, int24(uint24(spacing)), amountIn))
            );
        v4Active = false;
        amountOut = abi.decode(ret, (uint256));
    }

    /// @notice PoolManager callback for `_swapV4`: only from the manager, only while our own V4 hop runs.
    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        if (msg.sender != poolManager || !v4Active) revert Unauthorized();
        return abi.encode(_v4Execute(IPoolManager(msg.sender), abi.decode(data, (V4Swap))));
    }

    /// A native pool's ETH side is addressed as WETH everywhere else; in the pool it is currency 0.
    function _v4Currencies(V4Swap memory a) internal view returns (address cIn, address cOut) {
        address w = address(weth);
        bool native = a.kind == UNI_V4_NATIVE;
        cIn = (native && a.tokenIn == w) ? address(0) : a.tokenIn;
        cOut = (native && a.tokenOut == w) ? address(0) : a.tokenOut;
        if (cIn == cOut) revert BadRoute();
    }

    /// Exact-input swap; returns what we owe the pool (`owed`) and what it owes us (`got`).
    function _v4Swap(IPoolManager pm, V4Swap memory a, address cIn, address cOut)
        internal
        returns (uint256 owed, uint256 got)
    {
        bool zeroForOne = cIn < cOut;
        int256 delta = pm.swap(
            IPoolManager.PoolKey(zeroForOne ? cIn : cOut, zeroForOne ? cOut : cIn, a.fee, a.tickSpacing, address(0)),
            IPoolManager.SwapParams(
                zeroForOne, -int256(a.amountIn), zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE
            ),
            ""
        );
        (int128 inDelta, int128 outDelta) =
            zeroForOne ? (int128(delta >> 128), int128(delta)) : (int128(delta), int128(delta >> 128));
        if (inDelta > 0 || outDelta < 0) revert BadRoute();
        owed = uint256(uint128(-inDelta));
        got = uint256(uint128(outDelta));
    }

    function _v4Execute(IPoolManager pm, V4Swap memory a) internal returns (uint256 got) {
        (address cIn, address cOut) = _v4Currencies(a);
        uint256 owed;
        (owed, got) = _v4Swap(pm, a, cIn, cOut);
        if (cIn == address(0)) {
            weth.withdraw(owed);
            pm.settle{value: owed}();
        } else {
            pm.sync(cIn);
            _transfer(cIn, address(pm), owed);
            pm.settle();
        }
        pm.take(cOut, address(this), got);
        if (cOut == address(0)) weth.deposit{value: got}();
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
        } else if (hop.kind == UNI_V4 || hop.kind == UNI_V4_NATIVE) {
            amountOut = _swapV4(hop, amountIn);
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
