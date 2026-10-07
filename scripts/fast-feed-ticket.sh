#!/usr/bin/env bash
# Fast Feed ticket helper. RUN BY THE OPERATOR, never by an agent: `buy` moves real USDG.
# Docs: https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/use-fast-feed
#
#   scripts/fast-feed-ticket.sh status [wallet]     read-only: round, price, when a ticket becomes active, your balances
#   scripts/fast-feed-ticket.sh keygen              make the API key (stored 0600 outside the repo) and print its hash
#   scripts/fast-feed-ticket.sh buy --tickets N --deposit-usdg X [--yes]
#                                                   approve + deposit + purchase. Dry run unless --yes.
#   scripts/fast-feed-ticket.sh withdraw <usdg>     take unused deposit back to the wallet
#
# Environment:
#   FF_RPC                 Arbitrum One RPC (default: the public one)
#   FF_KEY_FILE            where the API key lives (default ~/.config/mev-bot/fast-feed.key)
#   FF_BUYER_PRIVATE_KEY   the paying wallet's key, for buy/withdraw only. Use a SEPARATE wallet (not the bot's hot wallet).
#                          Passed to cast through ETH_PRIVATE_KEY, so it never appears on a command line.
set -euo pipefail
export PATH="$HOME/.foundry/bin:$PATH"
RPC="${FF_RPC:-https://arb1.arbitrum.io/rpc}"
KEY_FILE="${FF_KEY_FILE:-$HOME/.config/mev-bot/fast-feed.key}"
TICKETS=0x451DD62C26b753d07d6104aaC6865AaE73a6A598   # docs: Tickets contract (proxy)
USDG=0x004B506865409877C9fA29bfb1ebA929984B9bbC      # docs: payment token, 6 decimals
FEED=wss://arb1-txfeed.arbitrum.io/

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "$1 not found (Foundry: https://getfoundry.sh)"; }
need cast
call() { cast call "$@" --rpc-url "$RPC" | awk '{print $1}'; }
usdg() { python3 -c "print('%.6f' % ($1/1e6))"; }

round_info() {
  ROUND=$(call "$TICKETS" 'roundNumber()(uint256)')
  PRICE=$(call "$TICKETS" 'currentPrice()(uint256)')
  END=$(call "$TICKETS" 'roundEnd()(uint256)')
  [[ "$(call "$TICKETS" 'token()(address)')" == "$USDG" ]] || die "Tickets.token() is not the USDG address in the docs: stop and re-read the docs page"
}

show_round() {
  round_info
  local now left
  now=$(date +%s); left=$(( END - now ))
  echo "round $ROUND, ticket price $(usdg "$PRICE") USDG (the docs: it can rise ~2x between rounds)"
  echo "this round ends $(date -u -d "@$END" '+%F %T UTC') (in $(( left / 3600 ))h $(( left % 3600 / 60 ))m)."
  echo "A ticket bought now is active for the NEXT round only: connect after that time, for 24 h."
  echo "feed: $FEED"
}

case "${1:-}" in
  status)
    show_round
    if [[ -n "${2:-}" ]]; then
      echo "wallet $2: USDG $(usdg "$(call "$USDG" 'balanceOf(address)(uint256)' "$2")"), deposited with Tickets $(usdg "$(call "$TICKETS" 'tokenBalance(address)(uint256)' "$2")"), ETH $(cast balance "$2" --ether --rpc-url "$RPC")"
    fi ;;
  keygen)
    need openssl
    [[ -e "$KEY_FILE" ]] && die "$KEY_FILE already exists (delete it deliberately to make a new key)"
    mkdir -p "$(dirname "$KEY_FILE")"; umask 077
    key=$(openssl rand -hex 32)
    printf '%s' "$key" > "$KEY_FILE"; chmod 600 "$KEY_FILE"
    # The docs: the hash covers the UTF-8 bytes of the key string exactly as sent in the header.
    echo "API key saved to $KEY_FILE (mode 600; never commit or paste it)."
    echo "key hash (goes on-chain): $(cast keccak "$key")"
    echo "To use it:  export ARBITRUM_FAST_FEED_KEY=\$(cat $KEY_FILE)" ;;
  buy)
    shift; tickets=1; deposit=""; yes=0
    while [[ $# -gt 0 ]]; do case "$1" in
      --tickets) tickets="$2"; shift 2 ;; --deposit-usdg) deposit="$2"; shift 2 ;; --yes) yes=1; shift ;;
      *) die "unknown option $1" ;; esac; done
    [[ -n "$deposit" ]] || die "--deposit-usdg is required (the docs advise enough for several rounds: price x 2 x rounds)"
    [[ -n "${FF_BUYER_PRIVATE_KEY:-}" ]] || die "set FF_BUYER_PRIVATE_KEY (a separate wallet holding USDG and a little ETH for gas)"
    [[ -f "$KEY_FILE" ]] || die "no API key: run keygen first"
    export ETH_PRIVATE_KEY="$FF_BUYER_PRIVATE_KEY"
    buyer=$(cast wallet address --private-key "$ETH_PRIVATE_KEY")
    hash=$(cast keccak "$(cat "$KEY_FILE")")
    round_info
    dep_raw=$(python3 -c "print(int(round(float('$deposit')*1e6)))")
    min_raw=$(( PRICE * tickets ))
    (( dep_raw >= min_raw )) || die "deposit $deposit USDG is below $tickets x price = $(usdg "$min_raw") USDG"
    bal=$(call "$USDG" 'balanceOf(address)(uint256)' "$buyer")
    eth=$(cast balance "$buyer" --rpc-url "$RPC")
    echo "buyer $buyer: USDG $(usdg "$bal"), ETH $(cast balance "$buyer" --ether --rpc-url "$RPC")"
    echo "plan: approve $(usdg "$dep_raw") USDG -> depositToken -> purchaseTickets(round $ROUND, price $(usdg "$PRICE"), $tickets, hash $hash)"
    (( bal >= dep_raw )) || die "wallet holds $(usdg "$bal") USDG, less than the $(usdg "$dep_raw") deposit"
    (( eth > 0 )) || die "wallet has no ETH for gas"
    if (( yes == 0 )); then echo "DRY RUN: nothing sent. Re-run with --yes to execute."; exit 0; fi
    read -r -p "Send these 3 transactions with REAL funds? type 'buy' to continue: " ok
    [[ "$ok" == buy ]] || die "aborted"
    cast send "$USDG" 'approve(address,uint256)' "$TICKETS" "$dep_raw" --rpc-url "$RPC" >/dev/null
    cast send "$TICKETS" 'depositToken(uint256)' "$dep_raw" --rpc-url "$RPC" >/dev/null
    echo "deposited; Tickets balance: $(usdg "$(call "$TICKETS" 'tokenBalance(address)(uint256)' "$buyer")") USDG"
    round_info   # re-read: the price or round may have moved
    cast send "$TICKETS" 'purchaseTickets(uint256,uint256,uint256,bytes32)' "$ROUND" "$PRICE" "$tickets" "$hash" --rpc-url "$RPC" >/dev/null
    echo "ticket purchased. Active from $(date -u -d "@$END" '+%F %T UTC'). Then:"
    echo "  export ARBITRUM_FAST_FEED_KEY=\$(cat $KEY_FILE)"
    echo "  cargo run --release -p mev-analytics --bin mev-feedlead -- config/arbitrum.toml 3600" ;;
  withdraw)
    [[ -n "${2:-}" ]] || die "usage: withdraw <usdg>"
    [[ -n "${FF_BUYER_PRIVATE_KEY:-}" ]] || die "set FF_BUYER_PRIVATE_KEY"
    export ETH_PRIVATE_KEY="$FF_BUYER_PRIVATE_KEY"
    cast send "$TICKETS" 'withdrawToken(uint256)' "$(python3 -c "print(int(round(float('$2')*1e6)))")" --rpc-url "$RPC" >/dev/null
    echo "withdrawn $2 USDG to the wallet" ;;
  *) sed -n '2,19p' "$0"; exit 2 ;;
esac
