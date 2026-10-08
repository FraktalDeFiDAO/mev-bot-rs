// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {FastFeedTicketVault, ITickets, IERC20Min} from "../src/FastFeedTicketVault.sol";

interface ITicketsAdmin is ITickets {
    function roundStart() external view returns (uint256);
    function roundEnd() external view returns (uint256);
    function roundDuration() external view returns (uint256);
    function grandfatherPeriodFraction() external view returns (uint256);
}

/// Fork tests against the real Fast Feed `Tickets` contract on Arbitrum One. They answer what the docs do
/// not: whether a contract can be the subscriber, when a new buyer may buy, and how renewal carries over.
contract FastFeedTicketVaultForkTest is Test {
    ITicketsAdmin constant TICKETS = ITicketsAdmin(0x451DD62C26b753d07d6104aaC6865AaE73a6A598);
    IERC20Min constant USDG = IERC20Min(0x004B506865409877C9fA29bfb1ebA929984B9bbC);
    bytes32 constant KEY_HASH = keccak256("test-api-key");

    address owner = makeAddr("owner");
    address keeper = makeAddr("keeper");
    FastFeedTicketVault vault;

    function setUp() public {
        vm.createSelectFork(vm.envOr("ARBITRUM_HTTP_URL", string("https://arb1.arbitrum.io/rpc")));
        vault = new FastFeedTicketVault(TICKETS, owner);
        deal(address(USDG), owner, 500e6);
    }

    /// Past the grandfather period (only the previous round's subscribers may buy before it ends), still inside the round.
    function _intoSecondHalf() internal {
        uint256 start = TICKETS.roundStart();
        uint256 dur = TICKETS.roundDuration();
        uint256 gf = TICKETS.grandfatherPeriodFraction();
        vm.warp(start + (dur * gf) / 256 + 3600);
        assertLt(block.timestamp, TICKETS.roundEnd(), "still in the round");
    }

    function _configure(uint256 renewals) internal {
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, 40e6, renewals);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
    }

    function test_a_contract_can_be_the_subscriber_and_a_keeper_can_renew() public {
        _intoSecondHalf();
        _configure(3);
        uint256 price = TICKETS.currentPrice();
        uint256 before_ = TICKETS.tokenBalance(address(vault));
        assertEq(before_, 100e6, "the deposit is held by the vault, not the owner");

        vm.prank(keeper); // anyone
        vault.renew();

        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1, "the vault holds a ticket");
        assertEq(TICKETS.tokenBalance(address(vault)), before_ - price, "the price came out of the deposit");
        assertEq(vault.renewalsLeft(), 2);
        (,, uint256 held,,, uint256 left, bool can) = vault.status();
        assertEq(held, 1);
        assertEq(left, 2);
        assertFalse(can, "already renewed this round");
    }

    function test_it_buys_at_most_once_per_round() public {
        _intoSecondHalf();
        _configure(5);
        vault.renew();
        vm.expectRevert(FastFeedTicketVault.AlreadyRenewed.selector);
        vault.renew();
    }

    function test_the_price_cap_stops_an_expensive_round() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 1, TICKETS.currentPrice() - 1, 3);
        USDG.approve(address(vault), 100e6);
        vault.fund(100e6);
        vm.stopPrank();
        vm.expectRevert(
            abi.encodeWithSelector(
                FastFeedTicketVault.PriceAboveCap.selector, TICKETS.currentPrice(), TICKETS.currentPrice() - 1
            )
        );
        vault.renew();
    }

    function test_an_underfunded_vault_cannot_renew() public {
        _intoSecondHalf();
        vm.startPrank(owner);
        vault.setConfig(KEY_HASH, 2, 40e6, 3);
        USDG.approve(address(vault), 20e6);
        vault.fund(20e6);
        vm.stopPrank();
        vm.expectRevert(
            abi.encodeWithSelector(FastFeedTicketVault.InsufficientDeposit.selector, 20e6, 2 * TICKETS.currentPrice())
        );
        vault.renew();
    }

    function test_renewals_run_out_and_unconfigured_vaults_do_nothing() public {
        vm.expectRevert(FastFeedTicketVault.NotConfigured.selector);
        vault.renew();
        _intoSecondHalf();
        _configure(0);
        vm.expectRevert(FastFeedTicketVault.NoRenewalsLeft.selector);
        vault.renew();
    }

    function test_only_the_owner_controls_the_money_and_the_settings() public {
        _intoSecondHalf();
        _configure(3);
        vm.startPrank(keeper);
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.setConfig(bytes32(uint256(1)), 100, type(uint256).max, 1000);
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.withdraw(1e6);
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.fund(1e6);
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.sweep(address(USDG));
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.transferOwnership(keeper);
        vm.stopPrank();
    }

    function test_the_owner_can_take_the_unused_deposit_back() public {
        _intoSecondHalf();
        _configure(3);
        vault.renew();
        uint256 left = TICKETS.tokenBalance(address(vault));
        uint256 ownerBefore = USDG.balanceOf(owner);
        vm.prank(owner);
        vault.withdraw(left);
        assertEq(USDG.balanceOf(owner), ownerBefore + left);
        assertEq(TICKETS.tokenBalance(address(vault)), 0);
        assertEq(USDG.balanceOf(address(vault)), 0, "nothing is stranded in the vault");
    }

    function test_directly_sent_usdg_can_be_deposited_or_swept() public {
        vm.startPrank(owner);
        USDG.transfer(address(vault), 30e6);
        vault.depositHeld();
        assertEq(TICKETS.tokenBalance(address(vault)), 30e6);
        USDG.transfer(address(vault), 5e6);
        uint256 b = USDG.balanceOf(owner);
        vault.sweep(address(USDG));
        assertEq(USDG.balanceOf(owner), b + 5e6);
        vm.stopPrank();
    }

    function test_ownership_moves_in_two_steps() public {
        vm.prank(owner);
        vault.transferOwnership(keeper);
        assertEq(vault.owner(), owner, "not yet");
        vm.prank(makeAddr("stranger"));
        vm.expectRevert(FastFeedTicketVault.NotOwner.selector);
        vault.acceptOwnership();
        vm.prank(keeper);
        vault.acceptOwnership();
        assertEq(vault.owner(), keeper);
    }

    /// A new buyer is locked out during the grandfather period: the vault reports the Tickets contract's revert.
    function test_a_new_buyer_cannot_buy_during_the_grandfather_period() public {
        vm.warp(TICKETS.roundStart() + 60);
        _configure(3);
        vm.expectRevert();
        vault.renew();
    }

    /// Subscribing in round N, the vault must be able to renew in the first half of N+1 (its grandfather right).
    function test_a_subscriber_renews_in_the_next_rounds_grandfather_period() public {
        _intoSecondHalf();
        _configure(3);
        vault.renew(); // a ticket for the next round
        uint256 round = TICKETS.roundNumber();
        // The next round begins; the ticket is now active and the vault is a previous-round subscriber.
        vm.warp(TICKETS.roundEnd() + 600);
        assertEq(TICKETS.roundNumber(), round + 1, "the round number follows the clock");
        vault.renew();
        assertEq(TICKETS.thisRoundTicketCount(address(vault)), 1, "renewed inside the grandfather period");
        assertEq(vault.renewalsLeft(), 1);
    }
}
