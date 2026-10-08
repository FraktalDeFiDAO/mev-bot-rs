// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

/// The Arbitrum Fast Feed `Tickets` contract (Arbitrum One `0x451DD62C26b753d07d6104aaC6865AaE73a6A598`).
/// Only the functions this vault uses. Tickets are counted per buyer address: a round's subscribers may
/// renew first (the "grandfather period", about the first half of each round, up to the number they held);
/// everyone else buys in the second half.
interface ITickets {
    function token() external view returns (address);
    function roundNumber() external view returns (uint256);
    function currentPrice() external view returns (uint256);
    function tokenBalance(address account) external view returns (uint256);
    function thisRoundTicketCount(address account) external view returns (uint256);
    function grandfatherCount(address account) external view returns (uint256);
    function depositToken(uint256 amount) external;
    function withdrawToken(uint256 amount) external;
    function purchaseTickets(
        uint256 expectedRound,
        uint256 expectedPrice,
        uint256 numTicketsDesired,
        bytes32 apiKeyHash
    ) external;
}

interface IERC20Min {
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// Buys and renews Fast Feed tickets so an unattended keeper can keep the subscription alive without ever
/// holding the money.
///
///  - The vault is the subscriber: tickets (and the renewal priority) belong to its address.
///  - `renew()` is callable by anyone. All it can do is buy `ticketsPerRound` tickets for the owner's
///    `apiKeyHash` at no more than `maxPricePerTicket`, at most once per round, while `renewalsLeft > 0`.
///    A griefer can trigger the renewal the owner already asked for, nothing else.
///  - Only the owner can fund, change those settings, withdraw, or sweep stray tokens. The keeper's own key
///    therefore needs gas only; if it leaks, the vault's USDG is not at risk.
///  - The API key itself never touches the chain: only its Keccak-256 hash is stored.
///  - No upgradeability, no proxy, no admin backdoor. Funds are the owner's to withdraw at any time.
contract FastFeedTicketVault {
    ITickets public immutable tickets;
    IERC20Min public immutable usdg;

    address public owner;
    address public pendingOwner;

    /// Keccak-256 of the API key, as presented in the Authorization header (the Fast Feed docs).
    bytes32 public apiKeyHash;
    /// Tickets to hold each round.
    uint256 public ticketsPerRound;
    /// Never pay more than this per ticket (USDG, 6 decimals). The price can move up to ~2x between rounds.
    uint256 public maxPricePerTicket;
    /// How many more rounds `renew()` may buy. Zero stops automation.
    uint256 public renewalsLeft;

    uint256 private locked = 1;

    event Renewed(uint256 indexed round, uint256 price, uint256 tickets, uint256 renewalsLeft);
    event Funded(uint256 amount);
    event Withdrawn(uint256 amount);
    event ConfigSet(bytes32 apiKeyHash, uint256 ticketsPerRound, uint256 maxPricePerTicket, uint256 renewalsLeft);
    event OwnershipTransferStarted(address indexed from, address indexed to);
    event OwnershipTransferred(address indexed from, address indexed to);

    error NotOwner();
    error NotConfigured();
    error NoRenewalsLeft();
    error AlreadyRenewed();
    error PriceAboveCap(uint256 price, uint256 cap);
    error InsufficientDeposit(uint256 have, uint256 need);
    error TransferFailed();
    error Reentrancy();

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

    constructor(ITickets tickets_, address owner_) {
        tickets = tickets_;
        usdg = IERC20Min(tickets_.token());
        owner = owner_;
        // Pre-approve nothing: each deposit approves exactly what it deposits.
        emit OwnershipTransferred(address(0), owner_);
    }

    // ---------------------------------------------------------------- the keeper's entry point

    /// Buy this round's tickets (valid from the next round). Permissionless; see the contract notes.
    function renew() external nonReentrant {
        if (apiKeyHash == bytes32(0) || ticketsPerRound == 0) revert NotConfigured();
        if (renewalsLeft == 0) revert NoRenewalsLeft();
        uint256 held = tickets.thisRoundTicketCount(address(this));
        if (held >= ticketsPerRound) revert AlreadyRenewed();
        uint256 wanted = ticketsPerRound - held;

        uint256 price = tickets.currentPrice();
        if (price > maxPricePerTicket) revert PriceAboveCap(price, maxPricePerTicket);
        uint256 need = price * wanted;
        uint256 have = tickets.tokenBalance(address(this));
        if (have < need) revert InsufficientDeposit(have, need);

        uint256 round = tickets.roundNumber();
        renewalsLeft -= 1;
        // The Tickets contract fills up to `wanted`, capped by the room left, and debits the vault's balance.
        tickets.purchaseTickets(round, price, wanted, apiKeyHash);
        emit Renewed(round, price, tickets.thisRoundTicketCount(address(this)) - held, renewalsLeft);
    }

    // ---------------------------------------------------------------- owner

    function setConfig(bytes32 apiKeyHash_, uint256 ticketsPerRound_, uint256 maxPricePerTicket_, uint256 renewals)
        external
        onlyOwner
    {
        apiKeyHash = apiKeyHash_;
        ticketsPerRound = ticketsPerRound_;
        maxPricePerTicket = maxPricePerTicket_;
        renewalsLeft = renewals;
        emit ConfigSet(apiKeyHash_, ticketsPerRound_, maxPricePerTicket_, renewals);
    }

    /// Move `amount` USDG from the owner (who must have approved this vault) into the Tickets deposit.
    function fund(uint256 amount) external onlyOwner nonReentrant {
        _pull(msg.sender, amount);
        _deposit(amount);
    }

    /// Deposit USDG that was sent to the vault directly.
    function depositHeld() external onlyOwner nonReentrant {
        _deposit(usdg.balanceOf(address(this)));
    }

    /// Take `amount` of the unused deposit back to the owner.
    function withdraw(uint256 amount) external onlyOwner nonReentrant {
        tickets.withdrawToken(amount);
        _send(address(usdg), owner, amount);
        emit Withdrawn(amount);
    }

    /// Recover any token (or USDG left in the vault) to the owner.
    function sweep(address token) external onlyOwner nonReentrant {
        uint256 bal = IERC20Min(token).balanceOf(address(this));
        _send(token, owner, bal);
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

    // ---------------------------------------------------------------- views for the keeper and the dashboard

    /// Everything a keeper needs to decide whether to call `renew()`.
    function status()
        external
        view
        returns (
            uint256 round,
            uint256 price,
            uint256 heldThisRound,
            uint256 grandfathered,
            uint256 deposit,
            uint256 left,
            bool canRenew
        )
    {
        round = tickets.roundNumber();
        price = tickets.currentPrice();
        heldThisRound = tickets.thisRoundTicketCount(address(this));
        grandfathered = tickets.grandfatherCount(address(this));
        deposit = tickets.tokenBalance(address(this));
        left = renewalsLeft;
        canRenew = apiKeyHash != bytes32(0) && ticketsPerRound > 0 && left > 0 && heldThisRound < ticketsPerRound
            && price <= maxPricePerTicket && deposit >= price * (ticketsPerRound - heldThisRound);
    }

    // ---------------------------------------------------------------- internals

    function _deposit(uint256 amount) private {
        if (!usdg.approve(address(tickets), amount)) revert TransferFailed();
        tickets.depositToken(amount);
        emit Funded(amount);
    }

    function _pull(address from, uint256 amount) private {
        (bool ok, bytes memory ret) =
            address(usdg).call(abi.encodeWithSelector(IERC20Min.transferFrom.selector, from, address(this), amount));
        if (!ok || (ret.length != 0 && !abi.decode(ret, (bool)))) revert TransferFailed();
    }

    function _send(address token, address to, uint256 amount) private {
        (bool ok, bytes memory ret) = token.call(abi.encodeWithSelector(IERC20Min.transfer.selector, to, amount));
        if (!ok || (ret.length != 0 && !abi.decode(ret, (bool)))) revert TransferFailed();
    }
}
