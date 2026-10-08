// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {FastFeedTicketVault, ITickets} from "../src/FastFeedTicketVault.sol";
import {UsdgConverter, IERC20Min, IPoolManagerV4, IWETHMin, Owned} from "../src/UsdgConverter.sol";

interface ITicketsAdmin is ITickets {
    function roundStart() external view returns (uint256);
    function roundEnd() external view returns (uint256);
    function roundDuration() external view returns (uint256);
    function grandfatherPeriodFraction() external view returns (uint256);
    function setPricingParams(uint64 minimumPrice, uint40 priceUpdateFraction, uint56 extra) external;
    function setRoundDuration(uint24 duration) external;
}

/// Shared fork setup against the real contracts on Arbitrum One. Set VAULT_FORK_BLOCK to pin the fork (needs an
/// archive-capable ARBITRUM_HTTP_URL); unset, the latest block is used and tests warp relative to the current round.
abstract contract VaultFork is Test {
    ITicketsAdmin constant TICKETS = ITicketsAdmin(0x451DD62C26b753d07d6104aaC6865AaE73a6A598);
    IERC20Min constant USDG = IERC20Min(0x004B506865409877C9fA29bfb1ebA929984B9bbC);
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant POOL_MANAGER = 0x360E68faCcca8cA495c1B759Fd9EEe466db9FB32;
    address constant SEQUENCER_FEED = 0xFdB631F5EE196F0ed6FAa767959853A9F217697D;
    address constant ETH_USD = 0x639Fe6ab55C921f74e7fac1ee960C0B6293ba612;
    address constant V3_WETH_USDC = 0xC6962004f452bE9203591991D15f6b388e09E8D0; // 0.05%, token0 WETH
    address constant HOOK = 0x630e9CaC0ffF557Bd3A01DAa111ed253c771Bcc0;
    address constant PARAMS_SETTER = 0xe128a8100d1b543dE82d04450B3406d57aBF553b;
    bytes32 constant TICKETS_PURCHASED = 0xceaf54bf42f8c7a19d411dddc261c173306656d278197351855ec86be3ff307a;
    bytes32 constant KEY_HASH = keccak256("test-api-key");

    address owner = makeAddr("owner");
    address keeper = makeAddr("keeper");
    FastFeedTicketVault vault;

    function setUp() public virtual {
        string memory url = vm.envOr("ARBITRUM_HTTP_URL", string("https://arb1.arbitrum.io/rpc"));
        uint256 pin = vm.envOr("VAULT_FORK_BLOCK", uint256(0));
        if (pin == 0) vm.createSelectFork(url);
        else vm.createSelectFork(url, pin);
        // Labelled test addresses can collide with real mainnet accounts that carry (delegated) code.
        vm.etch(owner, "");
        vm.etch(keeper, "");
        vault = new FastFeedTicketVault(TICKETS, owner, IPoolManagerV4(POOL_MANAGER), IWETHMin(WETH), SEQUENCER_FEED);
        deal(address(USDG), owner, 500e6);
    }

    /// Past the grandfather period (only the previous round's subscribers may buy before it ends), still in the round.
    function _intoSecondHalf() internal {
        uint256 start = TICKETS.roundStart();
        vm.warp(start + (TICKETS.roundDuration() * TICKETS.grandfatherPeriodFraction()) / 256 + 3600);
        assertLt(block.timestamp, TICKETS.roundEnd(), "still in the round");
    }

    function _configure(uint256 tickets, uint256 renewals) internal {
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, tickets, 40e6, renewals, 1_000e6, 0);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
    }

    function _hookedStep() internal pure returns (UsdgConverter.Step memory) {
        return UsdgConverter.Step(1, USDC, address(USDG), address(0), 8_388_608, 1, HOOK);
    }

    function _usdcRoute(uint16 bps, uint128 maxPerCall) internal pure returns (UsdgConverter.Route memory r) {
        r.steps = new UsdgConverter.Step[](1);
        r.steps[0] = _hookedStep();
        r.maxSlippageBps = bps;
        r.maxPerCall = maxPerCall;
        r.tokenDecimals = 6;
    }

    function _ethRoute(uint16 bps, uint128 maxPerCall) internal pure returns (UsdgConverter.Route memory r) {
        r.steps = new UsdgConverter.Step[](2);
        r.steps[0] = UsdgConverter.Step(0, WETH, USDC, V3_WETH_USDC, 0, 0, address(0));
        r.steps[1] = _hookedStep();
        r.maxSlippageBps = bps;
        r.maxPerCall = maxPerCall;
        r.feed = ETH_USD;
        r.tokenDecimals = 18;
        r.maxAge = 2 days; // tests warp; the feed's own freshness is mocked where it matters
    }
}

contract FastFeedTicketVaultForkTest is VaultFork {
    // ------------------------------------------------------------ renewal

    function test_a_contract_can_be_the_subscriber_and_a_keeper_can_renew_just_in_time() public {
        _intoSecondHalf();
        _configure(1, 3);
        uint256 price = TICKETS.currentPrice();
        assertEq(TICKETS.tokenBalance(address(vault)), 0, "nothing is parked inside the Tickets contract");
        assertEq(USDG.balanceOf(address(vault)), 100e6, "the money sits in the vault");

        vm.prank(keeper); // anyone
        vault.renew();

        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1, "the vault holds a ticket");
        assertEq(USDG.balanceOf(address(vault)), 100e6 - price, "exactly one price was spent");
        assertEq(TICKETS.tokenBalance(address(vault)), 0, "and nothing is left inside the Tickets contract");
        assertEq(USDG.allowance(address(vault), address(TICKETS)), 0, "no allowance is left standing");
        assertEq(vault.renewalsLeft(), 2);
        assertEq(vault.budgetLeft(), 1_000e6 - price);
        (,, uint256 held,,,, uint256 left,, bool can) = vault.status();
        assertEq(held, 1);
        assertEq(left, 2);
        assertFalse(can, "already renewed this round");
    }

    function test_it_buys_at_most_once_per_round() public {
        _intoSecondHalf();
        _configure(1, 5);
        vault.renew();
        vm.expectRevert(FastFeedTicketVault.AlreadyRenewed.selector);
        vault.renew();
    }

    function test_several_tickets_are_bought_and_paid_for_exactly() public {
        _intoSecondHalf();
        _configure(3, 4);
        uint256 price = TICKETS.currentPrice();
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 3);
        assertEq(USDG.balanceOf(address(vault)), 100e6 - 3 * price);
        assertEq(TICKETS.tokenBalance(address(vault)), 0);
        assertEq(vault.renewalsLeft(), 3, "one round, however many tickets");
    }

    /// Found on chain: in the grandfather window, asking for more than last round's holding reverts the whole
    /// purchase (`NotEnoughGrandfatheredTickets`). The vault buys what the window allows instead.
    function test_in_the_grandfather_window_it_buys_what_is_allowed() public {
        _intoSecondHalf();
        _configure(3, 10);
        vault.renew(); // 3 tickets this round => grandfatherCount 3 next round
        vm.warp(TICKETS.roundEnd() + 600); // first half of the next round
        assertEq(TICKETS.grandfatherCount(address(vault)), 3);
        vm.startPrank(owner);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6); // enough for the full ask of 5
        vm.stopPrank();
        vm.prank(owner);
        vault.setConfig(KEY_HASH, 5, 40e6, 10, 1_000e6, 0); // now ask for 5
        uint256 before_ = USDG.balanceOf(address(vault));
        uint256 price = TICKETS.currentPrice();
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 3, "capped at the grandfather allowance, not reverted");
        assertEq(before_ - USDG.balanceOf(address(vault)), 3 * price, "paid for 3 only");
        assertEq(TICKETS.tokenBalance(address(vault)), 0, "the unused part of the deposit came back");
        // Once the window is over the rest can be bought: same round, no extra renewal counted.
        uint256 left = vault.renewalsLeft();
        vm.warp(TICKETS.roundStart() + (TICKETS.roundDuration() * TICKETS.grandfatherPeriodFraction()) / 256 + 3600);
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 5);
        assertEq(vault.renewalsLeft(), left, "the round was already counted");
    }

    function test_a_subscriber_renews_in_the_next_rounds_grandfather_period() public {
        _intoSecondHalf();
        _configure(1, 3);
        vault.renew();
        uint256 round = TICKETS.roundNumber();
        vm.warp(TICKETS.roundEnd() + 600);
        assertEq(TICKETS.roundNumber(), round + 1, "the round number follows the clock");
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1, "renewed inside the grandfather period");
        assertEq(vault.renewalsLeft(), 1);
        assertEq(vault.lastRenewedRound(), round + 1);
    }

    function test_a_new_buyer_cannot_buy_during_the_grandfather_period() public {
        vm.warp(TICKETS.roundStart() + 60);
        _configure(1, 3);
        vm.expectRevert();
        vault.renew();
    }

    // ------------------------------------------------------------ limits

    function test_the_price_cap_stops_an_expensive_round() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, TICKETS.currentPrice() - 1, 3, 1_000e6, 0);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        uint256 p = TICKETS.currentPrice();
        vm.expectRevert(abi.encodeWithSelector(FastFeedTicketVault.PriceAboveCap.selector, p, p - 1));
        vault.renew();
    }

    /// The price follows the DAO's parameters from the next round: a doubled floor is what the vault pays.
    function test_it_pays_the_moved_price_and_the_cap_still_binds() public {
        _intoSecondHalf();
        _configure(1, 5);
        vault.renew(); // become a subscriber at 17
        vm.prank(PARAMS_SETTER);
        TICKETS.setPricingParams(34_000_000, 144, 0);
        vm.warp(TICKETS.roundEnd() + 600); // next round, grandfather window
        assertEq(TICKETS.currentPrice(), 34e6, "the new floor applies from the next round");
        uint256 before_ = USDG.balanceOf(address(vault));
        vault.renew();
        assertEq(before_ - USDG.balanceOf(address(vault)), 34e6, "paid the moved price, not the old one");
        // With a cap below the new price the next round is refused.
        vm.prank(owner);
        vault.setConfig(KEY_HASH, 1, 30e6, 5, 1_000e6, 0);
        vm.warp(TICKETS.roundEnd() + 600);
        vm.expectRevert(abi.encodeWithSelector(FastFeedTicketVault.PriceAboveCap.selector, 34e6, 30e6));
        vault.renew();
    }

    /// Funds for one of two tickets: keep part of the seat rather than lose it; with funds for none, refuse.
    function test_an_underfunded_vault_buys_what_it_can_afford_and_refuses_when_it_can_afford_nothing() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 2, 40e6, 3, 1_000e6, 0);
        USDG.approve(address(vault), 20e6);
        vault.fund(20e6);
        vm.stopPrank();
        uint256 price = TICKETS.currentPrice();
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1, "one of the two configured tickets");
        vm.expectRevert(abi.encodeWithSelector(FastFeedTicketVault.InsufficientFunds.selector, 20e6 - price, price));
        vault.renew();
    }

    function test_the_budget_is_a_hard_ceiling() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 5, 10e6, 0); // 10 USDG budget < a 17 USDG ticket
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        vm.expectRevert(abi.encodeWithSelector(FastFeedTicketVault.BudgetExceeded.selector, 17e6, 10e6));
        vault.renew();
        vm.prank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 5, 20e6, 0);
        vault.renew();
        assertEq(vault.budgetLeft(), 3e6, "budget decreases by what was actually paid");
    }

    function test_the_expiry_stops_renewals() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, 5, 1_000e6, block.timestamp + 100);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        vm.warp(block.timestamp + 101);
        vm.expectRevert(FastFeedTicketVault.Expired.selector);
        vault.renew();
    }

    function test_renewals_run_out_and_unconfigured_vaults_do_nothing() public {
        vm.expectRevert(FastFeedTicketVault.NotConfigured.selector);
        vault.renew();
        _intoSecondHalf();
        _configure(1, 0);
        vm.expectRevert(FastFeedTicketVault.NoRenewalsLeft.selector);
        vault.renew();
    }

    function test_a_purchase_that_buys_nothing_reverts_and_spends_nothing() public {
        _intoSecondHalf();
        _configure(1, 3);
        // The Tickets contract claims no ticket was added: the vault must not count or keep that renewal.
        vm.mockCall(
            address(TICKETS),
            abi.encodeWithSelector(ITickets.thisRoundTicketCount.selector, address(vault)),
            abi.encode(uint256(0))
        );
        uint256 before_ = USDG.balanceOf(address(vault));
        vm.expectRevert(FastFeedTicketVault.NoTicketsBought.selector);
        vault.renew();
        assertEq(USDG.balanceOf(address(vault)), before_);
        assertEq(vault.renewalsLeft(), 3);
    }

    function test_a_changed_payment_token_stops_renewals() public {
        _intoSecondHalf();
        _configure(1, 3);
        vm.mockCall(address(TICKETS), abi.encodeWithSelector(ITickets.token.selector), abi.encode(address(0xdead)));
        vm.expectRevert(FastFeedTicketVault.TokenChanged.selector);
        vault.renew();
    }

    // ------------------------------------------------------------ keys

    function test_a_queued_key_is_used_from_the_next_renewal() public {
        _intoSecondHalf();
        _configure(1, 3);
        bytes32 next = keccak256("next-key");
        vm.prank(owner);
        vault.rotateKey(next);
        vm.recordLogs();
        vault.renew();
        Vm.Log[] memory logs = vm.getRecordedLogs();
        bool seen;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].topics[0] == TICKETS_PURCHASED) {
                assertEq(logs[i].topics[1], bytes32(uint256(uint160(address(vault)))), "the vault is the buyer");
                assertEq(logs[i].topics[3], next, "the queued hash was registered");
                seen = true;
            }
        }
        assertTrue(seen, "a TicketsPurchased event was emitted");
        assertEq(vault.apiKeyHash(), next);
        assertEq(vault.nextApiKeyHash(), bytes32(0));
    }

    // ------------------------------------------------------------ owner

    function test_only_the_owner_controls_the_money_and_the_settings() public {
        _intoSecondHalf();
        _configure(1, 3);
        vm.startPrank(keeper);
        vm.expectRevert(Owned.NotOwner.selector);
        vault.setConfig(bytes32(uint256(1)), 100, type(uint256).max, 1000, type(uint256).max, 0);
        vm.expectRevert(Owned.NotOwner.selector);
        vault.withdraw(1e6);
        vm.expectRevert(Owned.NotOwner.selector);
        vault.fund(1e6);
        vm.expectRevert(Owned.NotOwner.selector);
        vault.sweep(address(USDG));
        vm.expectRevert(Owned.NotOwner.selector);
        vault.sweepEth();
        vm.expectRevert(Owned.NotOwner.selector);
        vault.exit();
        vm.expectRevert(Owned.NotOwner.selector);
        vault.rotateKey(bytes32(uint256(2)));
        vm.expectRevert(Owned.NotOwner.selector);
        vault.transferOwnership(keeper);
        vm.expectRevert(Owned.NotOwner.selector);
        vault.setRoute(USDC, _usdcRoute(10, 1e12));
        vm.expectRevert(Owned.NotOwner.selector);
        vault.removeRoute(USDC);
        vm.stopPrank();
    }

    function test_the_owner_can_take_money_back_and_exit_in_one_call() public {
        _intoSecondHalf();
        _configure(1, 3);
        vault.renew();
        uint256 ownerBefore = USDG.balanceOf(owner);
        vm.prank(owner);
        vault.withdraw(30e6);
        assertEq(USDG.balanceOf(owner), ownerBefore + 30e6);
        uint256 rest = USDG.balanceOf(address(vault));
        vm.prank(owner);
        vault.exit();
        assertEq(USDG.balanceOf(owner), ownerBefore + 30e6 + rest, "everything came back");
        assertEq(USDG.balanceOf(address(vault)), 0);
        assertEq(TICKETS.tokenBalance(address(vault)), 0);
        assertEq(vault.renewalsLeft(), 0, "automation stopped");
    }

    function test_exit_also_recovers_a_deposit_left_inside_the_tickets_contract() public {
        // Someone (or a failed run) left USDG deposited under the vault's name.
        deal(address(USDG), address(vault), 50e6);
        vm.startPrank(address(vault));
        USDG.approve(address(TICKETS), 50e6);
        TICKETS.depositToken(50e6);
        vm.stopPrank();
        assertEq(TICKETS.tokenBalance(address(vault)), 50e6);
        uint256 ownerBefore = USDG.balanceOf(owner);
        vm.prank(owner);
        vault.exit();
        assertEq(USDG.balanceOf(owner), ownerBefore + 50e6);
        assertEq(TICKETS.tokenBalance(address(vault)), 0);
    }

    function test_directly_sent_usdg_works_and_other_tokens_can_be_swept() public {
        vm.startPrank(owner);
        USDG.transfer(address(vault), 30e6);
        assertEq(USDG.balanceOf(address(vault)), 30e6, "a plain transfer is a deposit");
        deal(USDC, address(vault), 5e6);
        uint256 b = IERC20Min(USDC).balanceOf(owner);
        vault.sweep(USDC);
        assertEq(IERC20Min(USDC).balanceOf(owner), b + 5e6);
        vm.stopPrank();
        vm.deal(address(vault), 1 ether);
        uint256 e = owner.balance;
        vm.prank(owner);
        vault.sweepEth();
        assertEq(owner.balance, e + 1 ether);
    }

    function test_ownership_moves_in_two_steps() public {
        vm.prank(owner);
        vault.transferOwnership(keeper);
        assertEq(vault.owner(), owner, "not yet");
        vm.prank(makeAddr("stranger"));
        vm.expectRevert(Owned.NotOwner.selector);
        vault.acceptOwnership();
        vm.prank(keeper);
        vault.acceptOwnership();
        assertEq(vault.owner(), keeper);
    }

    // ------------------------------------------------------------ property

    /// Whatever the keeper does (renew or not), the owner's claim (USDG held + tickets deposit) falls by exactly the
    /// ticket price per ticket and the budget moves by the same amount.
    /// forge-config: default.fuzz.runs = 32
    function testFuzz_a_keeper_can_only_spend_the_configured_ticket_price(uint8 tickets, bool doRenew) public {
        tickets = uint8(bound(tickets, 1, 4));
        _intoSecondHalf();
        _configure(tickets, 3);
        uint256 claimBefore = USDG.balanceOf(address(vault)) + TICKETS.tokenBalance(address(vault));
        uint256 budgetBefore = vault.budgetLeft();
        uint256 price = TICKETS.currentPrice();
        if (doRenew) vault.renew();
        uint256 claimAfter = USDG.balanceOf(address(vault)) + TICKETS.tokenBalance(address(vault));
        uint256 spent = claimBefore - claimAfter;
        assertEq(spent, doRenew ? price * tickets : 0);
        assertEq(budgetBefore - vault.budgetLeft(), spent, "budget and spend agree");
        assertLe(spent, 40e6 * tickets, "never above the cap per ticket");
    }
}
