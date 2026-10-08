// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {UsdgConverter, IERC20Min, IPoolManagerV4, IWETHMin} from "./UsdgConverter.sol";

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

/// Buys and renews Fast Feed tickets so an unattended keeper can keep the subscription alive without ever
/// holding the money, and turns deposits of ETH / USDC / other whitelisted tokens into the USDG it needs.
///
///  - The vault is the subscriber: tickets (and the renewal priority) belong to its address.
///  - **Money stays in the vault.** `renew()` deposits exactly `price x tickets` into the Tickets contract right before
///    buying, and takes back whatever was not spent. The Tickets contract is a DAO-governed upgradeable proxy; the
///    vault never leaves a budget inside it.
///  - `renew()` is callable by anyone. All it can do is buy `ticketsPerRound` tickets for the owner's API-key hash at
///    no more than `maxPricePerTicket`, at most once per round, while `renewalsLeft`, `budgetLeft` and `expiry` allow.
///  - `convert()` (see `UsdgConverter`) is callable by anyone and bounded on chain by an oracle or a 1:1 peg.
///  - Only the owner can fund, change settings, add routes, withdraw, `exit()` or sweep. The keeper's own key therefore
///    needs gas only; if it leaks, the vault's money is not at risk.
///  - The API key itself never touches the chain: only its Keccak-256 hash is stored.
///  - No upgradeability, no proxy, no admin backdoor. Funds are the owner's to withdraw at any time.
contract FastFeedTicketVault is UsdgConverter {
    ITickets public immutable tickets;

    /// `NotEnoughGrandfatheredTickets(uint256,uint256)`: in the grandfather window a subscriber may buy at most the
    /// number of tickets it held last round, and asking for more reverts the whole purchase (no partial fill).
    bytes4 private constant NOT_ENOUGH_GRANDFATHERED = 0x1b350bf6;

    /// Keccak-256 of the API key, as presented in the Authorization header (the Fast Feed docs).
    bytes32 public apiKeyHash;
    /// Used from the next renewal on (planned key rotation); zero = none queued.
    bytes32 public nextApiKeyHash;
    /// Tickets to hold each round.
    uint256 public ticketsPerRound;
    /// Never pay more than this per ticket (USDG, 6 decimals). The price can move up to ~2x between rounds.
    uint256 public maxPricePerTicket;
    /// How many more rounds `renew()` may start. Zero stops automation.
    uint256 public renewalsLeft;
    /// Most USDG the vault may still spend on tickets.
    uint256 public budgetLeft;
    /// After this timestamp `renew()` reverts (0 = no expiry).
    uint256 public expiry;
    /// The round `renew()` last bought in (a round is counted once, however many calls it takes).
    uint256 public lastRenewedRound;

    event Renewed(
        uint256 indexed round, uint256 price, uint256 bought, uint256 paid, uint256 renewalsLeft, uint256 budgetLeft
    );
    event Funded(uint256 amount);
    event Withdrawn(uint256 amount);
    event Exited(uint256 usdgReturned);
    event ConfigSet(
        bytes32 apiKeyHash,
        uint256 ticketsPerRound,
        uint256 maxPricePerTicket,
        uint256 renewalsLeft,
        uint256 budgetLeft,
        uint256 expiry
    );
    event KeyRotationQueued(bytes32 nextApiKeyHash);

    error NotConfigured();
    error NoRenewalsLeft();
    error AlreadyRenewed();
    error Expired();
    error BudgetExceeded(uint256 need, uint256 budget);
    error PriceAboveCap(uint256 price, uint256 cap);
    error InsufficientFunds(uint256 have, uint256 need);
    error NoTicketsBought();
    error TokenChanged();

    constructor(ITickets tickets_, address owner_, IPoolManagerV4 poolManager_, IWETHMin weth_, address sequencerFeed_)
        UsdgConverter(IERC20Min(tickets_.token()), poolManager_, weth_, sequencerFeed_)
    {
        tickets = tickets_;
        _setOwner(owner_);
    }

    // ---------------------------------------------------------------- the keeper's entry point

    /// Buy this round's tickets (valid from the next round). Permissionless; see the contract notes.
    function renew() external nonReentrant {
        if (apiKeyHash == bytes32(0) || ticketsPerRound == 0) revert NotConfigured();
        if (expiry != 0 && block.timestamp > expiry) revert Expired();
        if (tickets.token() != address(usdg)) revert TokenChanged();

        uint256 round = tickets.roundNumber();
        uint256 held = tickets.thisRoundTicketCount(address(this));
        if (held >= ticketsPerRound) revert AlreadyRenewed();
        bool newRound = lastRenewedRound != round;
        if (newRound && renewalsLeft == 0) revert NoRenewalsLeft();

        uint256 price = tickets.currentPrice();
        if (price > maxPricePerTicket) revert PriceAboveCap(price, maxPricePerTicket);

        // Ask for what is configured, but never more than the vault can pay for or the budget allows: keeping part of
        // the subscription (and the renewal priority that comes with it) beats losing all of it.
        uint256 wanted = ticketsPerRound - held;
        uint256 have = usdg.balanceOf(address(this));
        if (price != 0) {
            uint256 affordable = have / price;
            uint256 inBudget = budgetLeft / price;
            if (affordable < wanted) wanted = affordable;
            if (inBudget < wanted) wanted = inBudget;
            if (wanted == 0) {
                if (affordable == 0) revert InsufficientFunds(have, price);
                revert BudgetExceeded(price, budgetLeft);
            }
        }
        uint256 need = price * wanted;

        bytes32 key = nextApiKeyHash != bytes32(0) ? nextApiKeyHash : apiKeyHash;

        // Just-in-time: deposit exactly what this purchase can cost, buy, take back the rest.
        if (!usdg.approve(address(tickets), need)) revert TransferFailed();
        tickets.depositToken(need);
        _purchase(round, price, wanted, held, key);
        uint256 bought = tickets.thisRoundTicketCount(address(this)) - held;
        if (bought == 0) revert NoTicketsBought();
        uint256 leftover = tickets.tokenBalance(address(this));
        if (leftover != 0) tickets.withdrawToken(leftover);
        // (If the Tickets contract did not pull the whole allowance, do not leave one standing.)
        if (!usdg.approve(address(tickets), 0)) revert TransferFailed();

        uint256 paid = price * bought;
        budgetLeft -= paid;
        if (newRound) {
            renewalsLeft -= 1;
            lastRenewedRound = round;
        }
        if (nextApiKeyHash != bytes32(0)) {
            apiKeyHash = nextApiKeyHash;
            nextApiKeyHash = bytes32(0);
        }
        emit Renewed(round, price, bought, paid, renewalsLeft, budgetLeft);
    }

    /// Buy `wanted` tickets; if the Tickets contract refuses because that exceeds the grandfather allowance, buy what
    /// the allowance permits instead (the rest can be bought once the window is over). Any other failure bubbles up.
    function _purchase(uint256 round, uint256 price, uint256 wanted, uint256 held, bytes32 key) private {
        (bool ok, bytes memory err) =
            address(tickets).call(abi.encodeCall(ITickets.purchaseTickets, (round, price, wanted, key)));
        if (ok) return;
        if (bytes4(err) != NOT_ENOUGH_GRANDFATHERED) _bubble(err);
        uint256 allowance = tickets.grandfatherCount(address(this));
        if (allowance <= held) _bubble(err);
        uint256 ask = allowance - held;
        tickets.purchaseTickets(round, price, ask < wanted ? ask : wanted, key);
    }

    function _bubble(bytes memory err) private pure {
        assembly {
            revert(add(err, 32), mload(err))
        }
    }

    // ---------------------------------------------------------------- owner

    function setConfig(
        bytes32 apiKeyHash_,
        uint256 ticketsPerRound_,
        uint256 maxPricePerTicket_,
        uint256 renewals,
        uint256 budget,
        uint256 expiry_
    ) external onlyOwner {
        apiKeyHash = apiKeyHash_;
        ticketsPerRound = ticketsPerRound_;
        maxPricePerTicket = maxPricePerTicket_;
        renewalsLeft = renewals;
        budgetLeft = budget;
        expiry = expiry_;
        emit ConfigSet(apiKeyHash_, ticketsPerRound_, maxPricePerTicket_, renewals, budget, expiry_);
    }

    /// Queue a new API-key hash for the next renewal (planned rotation; tickets already bought keep the old hash
    /// until their round ends: there is no revoke on the Tickets contract).
    function rotateKey(bytes32 next) external onlyOwner {
        nextApiKeyHash = next;
        emit KeyRotationQueued(next);
    }

    /// Move `amount` USDG from the owner (who must have approved this vault) into the vault. Sending USDG, ETH or any
    /// whitelisted token directly works too.
    function fund(uint256 amount) external onlyOwner nonReentrant {
        _pull(address(usdg), msg.sender, amount);
        emit Funded(amount);
    }

    /// Take `amount` of the vault's USDG back to the owner.
    function withdraw(uint256 amount) external onlyOwner nonReentrant {
        _send(address(usdg), owner, amount);
        emit Withdrawn(amount);
    }

    /// Emergency exit: stop automation, pull back anything held in the Tickets contract, and send all USDG to the
    /// owner in one call (e.g. on a suspicious upgrade of the Tickets proxy). Other tokens: `sweep` / `sweepEth`.
    function exit() external onlyOwner nonReentrant {
        renewalsLeft = 0;
        uint256 inTickets = tickets.tokenBalance(address(this));
        if (inTickets != 0) tickets.withdrawToken(inTickets);
        uint256 bal = usdg.balanceOf(address(this));
        if (bal != 0) _send(address(usdg), owner, bal);
        emit Exited(bal);
    }

    /// Recover any token held by the vault to the owner.
    function sweep(address token) external onlyOwner nonReentrant {
        _send(token, owner, IERC20Min(token).balanceOf(address(this)));
    }

    function sweepEth() external onlyOwner nonReentrant {
        (bool ok,) = owner.call{value: address(this).balance}("");
        if (!ok) revert TransferFailed();
    }

    // ---------------------------------------------------------------- views for the keeper and the dashboard

    /// Everything a keeper needs. `canRenew` says the vault is configured and funded for another purchase this round;
    /// it does not model the Tickets contract's timing rules (a new buyer in the grandfather period would still
    /// revert), so a keeper should simulate `renew()` before sending.
    function status()
        external
        view
        returns (
            uint256 round,
            uint256 price,
            uint256 heldThisRound,
            uint256 grandfathered,
            uint256 usdgHeld,
            uint256 inTickets,
            uint256 left,
            uint256 budget,
            bool canRenew
        )
    {
        round = tickets.roundNumber();
        price = tickets.currentPrice();
        heldThisRound = tickets.thisRoundTicketCount(address(this));
        grandfathered = tickets.grandfatherCount(address(this));
        usdgHeld = usdg.balanceOf(address(this));
        inTickets = tickets.tokenBalance(address(this));
        left = renewalsLeft;
        budget = budgetLeft;
        bool roundOk = lastRenewedRound == round || left > 0;
        // Ready means: configured, not expired, price within the cap, and funds and budget for at least one ticket.
        canRenew = apiKeyHash != bytes32(0) && ticketsPerRound > 0 && roundOk && heldThisRound < ticketsPerRound
            && (expiry == 0 || block.timestamp <= expiry) && price <= maxPricePerTicket && price <= budget
            && usdgHeld >= price && tickets.token() == address(usdg);
    }
}
