// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

interface IERC20Min {
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
}

interface IWETHMin {
    function deposit() external payable;
}

interface IChainlinkFeed {
    function decimals() external view returns (uint8);
    function latestRoundData()
        external
        view
        returns (uint80, int256 answer, uint256 startedAt, uint256 updatedAt, uint80);
}

/// Uniswap V3 pool, the parts used for a direct pool swap (no router).
interface IUniV3PoolMin {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);
}

/// Uniswap V4 PoolManager (singleton). `BalanceDelta` is an int256, amount0 in the upper and amount1 in the lower 128 bits.
interface IPoolManagerV4 {
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
    function swap(PoolKey memory key, SwapParams memory params, bytes calldata hookData) external returns (int256);
    function sync(address currency) external;
    function settle() external payable returns (uint256 paid);
    function take(address currency, address to, uint256 amount) external;
}

/// Minimal ownership (two-step) and a re-entrancy lock shared by the vault and its converter.
abstract contract Owned {
    address public owner;
    address public pendingOwner;
    uint256 private locked = 1;

    event OwnershipTransferStarted(address indexed from, address indexed to);
    event OwnershipTransferred(address indexed from, address indexed to);

    error NotOwner();
    error Reentrancy();
    error TransferFailed();

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier nonReentrant() {
        if (locked != 1) revert Reentrancy();
        locked = 2;
        _;
        locked = 1;
    }

    function transferOwnership(address to) external onlyOwner {
        pendingOwner = to;
        emit OwnershipTransferStarted(owner, to);
    }

    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotOwner();
        emit OwnershipTransferred(owner, msg.sender);
        owner = msg.sender;
        pendingOwner = address(0);
    }

    function _setOwner(address to) internal {
        owner = to;
        emit OwnershipTransferred(address(0), to);
    }

    function _pull(address token, address from, uint256 amount) internal {
        (bool ok, bytes memory ret) =
            token.call(abi.encodeWithSelector(IERC20Min.transferFrom.selector, from, address(this), amount));
        if (!ok || (ret.length != 0 && !abi.decode(ret, (bool)))) revert TransferFailed();
    }

    function _send(address token, address to, uint256 amount) internal {
        (bool ok, bytes memory ret) = token.call(abi.encodeWithSelector(IERC20Min.transfer.selector, to, amount));
        if (!ok || (ret.length != 0 && !abi.decode(ret, (bool)))) revert TransferFailed();
    }
}

/// Turns whitelisted deposits (ETH, USDC, ...) into USDG without trusting whoever triggers the conversion.
///
///  - The owner whitelists a token with a **route** (up to 3 chained swaps through Uniswap V3 pools and Uniswap V4
///    pools, ending in USDG), a slippage bound, a per-call size cap and a valuation (a Chainlink feed, or 1:1 for a
///    stablecoin). ETH is routed as WETH.
///  - **Anyone** may call `convert(token, amountIn)`. The contract computes `minOut` itself from the valuation and
///    reverts unless the vault's USDG balance rose by at least that much. A caller can therefore only sandwich a
///    conversion inside the slippage bound, never steal it, and cannot choose the route.
///  - Pools are called directly (V3 swap + callback, V4 unlock + callback): no router, no approvals. Callbacks are
///    accepted only from the configured pool / the PoolManager, only while `convert` is running.
///  - USDG has no price feed; it is treated as $1. If it trades below $1 conversions revert (fail safe); above $1 they
///    simply return more.
abstract contract UsdgConverter is Owned {
    uint8 internal constant V3 = 0;
    uint8 internal constant V4 = 1;
    uint160 internal constant MIN_SQRT_RATIO_PLUS_ONE = 4295128740;
    uint160 internal constant MAX_SQRT_RATIO_MINUS_ONE = 1461446703485210103287273052203988822378723970341;
    uint256 internal constant MAX_STEPS = 3;
    uint16 internal constant MAX_SLIPPAGE_BPS = 1_000;
    uint256 internal constant SEQUENCER_GRACE = 3_600;

    /// One swap of a route. For `V3`, `pool` is the pool; for `V4`, `(fee, tickSpacing, hooks)` complete the pool key
    /// (currencies are `tokenIn`/`tokenOut` sorted; both must be ERC-20s).
    struct Step {
        uint8 kind;
        address tokenIn;
        address tokenOut;
        address pool;
        uint24 fee;
        int24 tickSpacing;
        address hooks;
    }

    /// `feed == address(0)`: a stablecoin valued 1:1 with USD. `tokenDecimals` is the input token's (18 for ETH).
    struct Route {
        Step[] steps;
        uint16 maxSlippageBps;
        uint128 maxPerCall;
        address feed;
        uint8 tokenDecimals;
        uint32 maxAge;
    }

    IERC20Min public immutable usdg;
    IPoolManagerV4 public immutable poolManager;
    IWETHMin public immutable weth;
    /// Chainlink L2 sequencer-uptime feed; `address(0)` disables the check (tests on chains without one).
    address public immutable sequencerFeed;

    mapping(address => Route) internal routes;
    /// Whitelisted input tokens (`address(0)` = ETH), for enumeration.
    address[] public routeTokens;

    bool transient _converting;
    address transient _expectedPool;
    address transient _expectedTokenIn;
    bool transient _expectedZeroForOne;
    uint256 transient _expectedMax;

    event RouteSet(address indexed token, uint256 steps, uint16 maxSlippageBps, uint128 maxPerCall, address feed);
    event RouteRemoved(address indexed token);
    /// `expected` is the oracle (or 1:1) value of `amountIn` in USDG units; `out` what the vault actually received.
    event Converted(address indexed token, uint256 amountIn, uint256 out, uint256 minOut, uint256 expected);

    error NoRoute();
    error BadRoute();
    error BadAmount();
    error Insufficient();
    error Unauthorized();
    error Slippage(uint256 out, uint256 minOut);
    error StaleOracle();
    error SequencerDown();

    constructor(IERC20Min usdg_, IPoolManagerV4 poolManager_, IWETHMin weth_, address sequencerFeed_) {
        usdg = usdg_;
        poolManager = poolManager_;
        weth = weth_;
        sequencerFeed = sequencerFeed_;
    }

    receive() external payable {}

    // ---------------------------------------------------------------- owner: whitelist

    function setRoute(address token, Route calldata r) external onlyOwner {
        uint256 n = r.steps.length;
        if (n == 0 || n > MAX_STEPS) revert BadRoute();
        if (r.maxSlippageBps > MAX_SLIPPAGE_BPS || r.maxPerCall == 0 || r.tokenDecimals > 30) revert BadRoute();
        address cur = token == address(0) ? address(weth) : token;
        for (uint256 i; i < n; ++i) {
            Step calldata s = r.steps[i];
            if (s.tokenIn != cur || s.tokenOut == address(0) || s.tokenOut == s.tokenIn) revert BadRoute();
            if (s.kind == V3) {
                if (s.pool == address(0)) revert BadRoute();
                (address a, address b) = (IUniV3PoolMin(s.pool).token0(), IUniV3PoolMin(s.pool).token1());
                (address lo, address hi) = s.tokenIn < s.tokenOut ? (s.tokenIn, s.tokenOut) : (s.tokenOut, s.tokenIn);
                if (a != lo || b != hi) revert BadRoute();
            } else if (s.kind == V4) {
                if (s.tokenIn == address(0) || s.tickSpacing <= 0 || address(poolManager) == address(0)) {
                    revert BadRoute();
                }
            } else {
                revert BadRoute();
            }
            cur = s.tokenOut;
        }
        if (cur != address(usdg)) revert BadRoute();

        Route storage dst = routes[token];
        bool isNew = dst.steps.length == 0;
        delete dst.steps;
        for (uint256 i; i < n; ++i) {
            dst.steps.push(r.steps[i]);
        }
        dst.maxSlippageBps = r.maxSlippageBps;
        dst.maxPerCall = r.maxPerCall;
        dst.feed = r.feed;
        dst.tokenDecimals = r.tokenDecimals;
        dst.maxAge = r.maxAge;
        if (isNew) routeTokens.push(token);
        emit RouteSet(token, n, r.maxSlippageBps, r.maxPerCall, r.feed);
    }

    function removeRoute(address token) external onlyOwner {
        if (routes[token].steps.length == 0) revert NoRoute();
        delete routes[token];
        uint256 n = routeTokens.length;
        for (uint256 i; i < n; ++i) {
            if (routeTokens[i] == token) {
                routeTokens[i] = routeTokens[n - 1];
                routeTokens.pop();
                break;
            }
        }
        emit RouteRemoved(token);
    }

    // ---------------------------------------------------------------- anyone: convert

    /// Swap `amountIn` of a whitelisted token (`address(0)` = ETH) held by the vault into USDG along its route.
    function convert(address token, uint256 amountIn) external nonReentrant returns (uint256 out) {
        Route storage r = routes[token];
        uint256 n = r.steps.length;
        if (n == 0) revert NoRoute();
        if (amountIn == 0 || amountIn > r.maxPerCall) revert BadAmount();

        uint256 expected = _valueInUsdg(r, amountIn);
        uint256 minOut = expected * (10_000 - r.maxSlippageBps) / 10_000;

        if (token == address(0)) {
            if (address(this).balance < amountIn) revert Insufficient();
            weth.deposit{value: amountIn}();
        } else if (IERC20Min(token).balanceOf(address(this)) < amountIn) {
            revert Insufficient();
        }

        uint256 before = usdg.balanceOf(address(this));
        uint256 amt = amountIn;
        _converting = true;
        for (uint256 i; i < n; ++i) {
            amt = _swapStep(r.steps[i], amt);
        }
        _converting = false;

        out = usdg.balanceOf(address(this)) - before;
        if (out < minOut) revert Slippage(out, minOut);
        emit Converted(token, amountIn, out, minOut, expected);
    }

    /// What `convert` would require for `amountIn`: the oracle (or 1:1) value and the minimum USDG it must produce.
    function quote(address token, uint256 amountIn) external view returns (uint256 expected, uint256 minOut) {
        Route storage r = routes[token];
        if (r.steps.length == 0) revert NoRoute();
        expected = _valueInUsdg(r, amountIn);
        minOut = expected * (10_000 - r.maxSlippageBps) / 10_000;
    }

    function routeOf(address token) external view returns (Route memory) {
        return routes[token];
    }

    function routeTokenCount() external view returns (uint256) {
        return routeTokens.length;
    }

    /// Balances of every whitelisted token held by the vault (`address(0)` = ETH).
    function holdings() external view returns (address[] memory tokens, uint256[] memory balances) {
        uint256 n = routeTokens.length;
        tokens = new address[](n);
        balances = new uint256[](n);
        for (uint256 i; i < n; ++i) {
            tokens[i] = routeTokens[i];
            balances[i] =
                tokens[i] == address(0) ? address(this).balance : IERC20Min(tokens[i]).balanceOf(address(this));
        }
    }

    // ---------------------------------------------------------------- valuation

    function _valueInUsdg(Route storage r, uint256 amountIn) internal view returns (uint256) {
        uint256 dec = r.tokenDecimals;
        if (r.feed == address(0)) return amountIn * 1e6 / (10 ** dec);
        if (sequencerFeed != address(0)) {
            (, int256 up, uint256 startedAt,,) = IChainlinkFeed(sequencerFeed).latestRoundData();
            if (up != 0 || block.timestamp - startedAt <= SEQUENCER_GRACE) revert SequencerDown();
        }
        (, int256 answer,, uint256 updatedAt,) = IChainlinkFeed(r.feed).latestRoundData();
        if (answer <= 0 || block.timestamp - updatedAt > r.maxAge) revert StaleOracle();
        uint256 fd = IChainlinkFeed(r.feed).decimals();
        return amountIn * uint256(answer) * 1e6 / ((10 ** fd) * (10 ** dec));
    }

    // ---------------------------------------------------------------- swaps

    /// One exact-input swap; returns what the vault actually received of `tokenOut` (a balance difference, so a
    /// pool's reported amounts are never trusted).
    function _swapStep(Step storage s, uint256 amt) private returns (uint256 got) {
        address tokenOut = s.tokenOut;
        uint256 before = IERC20Min(tokenOut).balanceOf(address(this));
        bool zeroForOne = s.tokenIn < tokenOut;
        if (s.kind == V3) {
            _expectedPool = s.pool;
            _expectedTokenIn = s.tokenIn;
            _expectedZeroForOne = zeroForOne;
            _expectedMax = amt;
            IUniV3PoolMin(s.pool)
                .swap(
                    address(this),
                    zeroForOne,
                    int256(amt),
                    zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE,
                    ""
                );
            _expectedPool = address(0);
        } else {
            poolManager.unlock(abi.encode(s.tokenIn, tokenOut, s.fee, s.tickSpacing, s.hooks, amt));
        }
        got = IERC20Min(tokenOut).balanceOf(address(this)) - before;
        if (got == 0) revert BadRoute();
    }

    /// Uniswap V3 pool callback: pay the pool what it is owed, only during our own swap and only to the pool we called.
    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata) external {
        if (!_converting || msg.sender != _expectedPool) revert Unauthorized();
        int256 owed = _expectedZeroForOne ? amount0Delta : amount1Delta;
        if (owed <= 0 || uint256(owed) > _expectedMax) revert Unauthorized();
        _send(_expectedTokenIn, msg.sender, uint256(owed));
    }

    /// Uniswap V4 PoolManager callback for a V4 step: swap, settle the input, take the output.
    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        if (!_converting || msg.sender != address(poolManager)) revert Unauthorized();
        (address tIn, address tOut, uint24 fee, int24 spacing, address hooks, uint256 amt) =
            abi.decode(data, (address, address, uint24, int24, address, uint256));
        bool zeroForOne = tIn < tOut;
        int256 delta = poolManager.swap(
            IPoolManagerV4.PoolKey(zeroForOne ? tIn : tOut, zeroForOne ? tOut : tIn, fee, spacing, hooks),
            IPoolManagerV4.SwapParams(
                zeroForOne, -int256(amt), zeroForOne ? MIN_SQRT_RATIO_PLUS_ONE : MAX_SQRT_RATIO_MINUS_ONE
            ),
            ""
        );
        (int128 inDelta, int128 outDelta) =
            zeroForOne ? (int128(delta >> 128), int128(delta)) : (int128(delta), int128(delta >> 128));
        if (inDelta > 0 || outDelta < 0) revert BadRoute();
        poolManager.sync(tIn);
        _send(tIn, address(poolManager), uint256(uint128(-inDelta)));
        poolManager.settle();
        poolManager.take(tOut, address(this), uint256(uint128(outDelta)));
        return "";
    }
}
