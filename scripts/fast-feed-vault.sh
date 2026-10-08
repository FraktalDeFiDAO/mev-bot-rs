#!/usr/bin/env bash
# Fast Feed ticket vault v2: deploy, configure, convert deposits to USDG, and keep it renewed.
# See docs/runbooks/fast-feed-experiment.md. The vault (contracts/src/FastFeedTicketVault.sol) holds the money
# and is the ticket subscriber; anyone can call renew() and convert(), only the owner can move money or change
# settings. So the KEEPER KEY NEEDS GAS ONLY.
#
#   OPERATOR (owner keystore account, run by you; every sending command is a dry run unless --yes):
#   scripts/fast-feed-vault.sh deploy [--yes]
#   scripts/fast-feed-vault.sh setup <vault> --tickets N --max-price USDG --renewals R --budget USDG [--expiry-days D] [--deposit USDG] [--yes]
#   scripts/fast-feed-vault.sh route <vault> usdc|eth|both [--yes]      whitelist deposits of USDC / ETH for conversion
#   scripts/fast-feed-vault.sh rotate-key <vault> <new-key-file> [--yes]
#   scripts/fast-feed-vault.sh withdraw <vault> <usdg> [--yes]
#   scripts/fast-feed-vault.sh exit <vault> [--yes]                      stop automation, return all USDG
#   KEEPER (gas-only keystore account; cron/systemd):
#   scripts/fast-feed-vault.sh keeper <vault> [--loop SECS]              convert if short, then renew when the chain allows
#   ANYONE:
#   scripts/fast-feed-vault.sh status <vault>
#   scripts/fast-feed-vault.sh convert <vault> usdc|eth <amount>         (keeper account) swap a deposit into USDG
#
# Environment: FF_RPC, FF_KEY_FILE (API key file from fast-feed-ticket.sh keygen), FF_OWNER_ACCOUNT / FF_KEEPER_ACCOUNT
# (Foundry keystore names, see signer()), FF_PASSWORD_FILE for unattended keepers.
set -euo pipefail
export PATH="$HOME/.foundry/bin:$PATH"
cd "$(dirname "$0")/.."
RPC="${FF_RPC:-https://arb1.arbitrum.io/rpc}"
KEY_FILE="${FF_KEY_FILE:-$HOME/.config/mev-bot/fast-feed.key}"
TICKETS=0x451DD62C26b753d07d6104aaC6865AaE73a6A598
USDG=0x004B506865409877C9fA29bfb1ebA929984B9bbC
USDC=0xaf88d065e77c8cC2239327C5EDb3A432268e5831
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
POOL_MANAGER=0x360E68faCcca8cA495c1B759Fd9EEe466db9FB32
SEQUENCER_FEED=0xFdB631F5EE196F0ed6FAa767959853A9F217697D   # Chainlink L2 sequencer uptime
ETH_USD=0x639Fe6ab55C921f74e7fac1ee960C0B6293ba612          # Chainlink ETH/USD, 8 decimals, 24 h heartbeat
V3_WETH_USDC=0xC6962004f452bE9203591991D15f6b388e09E8D0     # Uniswap V3 WETH/USDC 0.05%
HOOK=0x630e9cac0fff557bd3a01daa111ed253c771bcc0            # hooked, dynamic-fee USDG/USDC V4 pool (the only deep USDG pool)
ZERO=0x0000000000000000000000000000000000000000
die() { echo "error: $*" >&2; exit 1; }
raw() { python3 -c "print(int(round(float('$1')*1e6)))"; }
usdg() { python3 -c "print('%.6f' % ($1/1e6))"; }
num() { awk '{print $1}'; }

# Signer selection: a Foundry KEYSTORE account only (`cast wallet import <name> --interactive`; the key is typed once and stored
# encrypted in ~/.foundry/keystores, override with FF_KEYSTORE_DIR). The raw key is never accepted: Foundry 1.8 has no environment
# variable for it, so every use would put it on a command line. cast asks for the keystore password; for an unattended keeper
# set FF_PASSWORD_FILE to a file (mode 600) holding it. Sets SIGN (extra cast args) and SIGNER (the address).
signer() {
  local acct="${1}_ACCOUNT"
  [[ -n "${!acct:-}" ]] || die "set ${acct} to a keystore account name (create it: cast wallet import <name> --interactive)"
  SIGN=(--keystore "${FF_KEYSTORE_DIR:-$HOME/.foundry/keystores}/${!acct}")
  [[ -z "${FF_PASSWORD_FILE:-}" ]] || SIGN+=(--password-file "$FF_PASSWORD_FILE")
  SIGNER=$(cast wallet address "${SIGN[@]}")
}

# Route presets (the Route struct as a cast tuple): steps[(kind,tokenIn,tokenOut,pool,fee,tickSpacing,hooks)], maxSlippageBps,
# maxPerCall, feed, tokenDecimals, maxAge. kind 0 = Uniswap V3 pool, 1 = Uniswap V4 pool key.
V4_STEP="(1,$USDC,$USDG,$ZERO,8388608,1,$HOOK)"
route_usdc() { echo "([$V4_STEP],10,50000000000,$ZERO,6,0)"; }                                      # 10 bps, up to 50,000 USDC per call, 1:1 stable
route_eth()  { echo "([(0,$WETH,$USDC,$V3_WETH_USDC,0,0,$ZERO),(1,$USDC,$USDG,$ZERO,8388608,1,$HOOK)],100,25000000000000000000,$ETH_USD,18,90000)"; }  # 100 bps, up to 25 ETH, Chainlink-bounded, feed max age 25 h
ROUTE_SIG='setRoute(address,((uint8,address,address,address,uint24,int24,address)[],uint16,uint128,address,uint8,uint32))'

yes_flag() { yes=0; for a in "$@"; do [[ "$a" == --yes ]] && yes=1; done; return 0; }
confirm() { read -r -p "$1 type '$2': " ok; [[ "$ok" == "$2" ]] || die aborted; }

view() { cast call "$vault" "$@" --rpc-url "$RPC"; }

# Name a custom-error selector from a revert message (the vault's, the converter's and the Tickets contract's).
errname() {
  local sel; sel=$(grep -oE '0x[0-9a-f]{8}' <<<"$1" | head -1)
  [[ -n "$sel" ]] || { echo "$1"; return; }
  for e in 'NotConfigured()' 'NoRenewalsLeft()' 'AlreadyRenewed()' 'Expired()' 'BudgetExceeded(uint256,uint256)' \
           'PriceAboveCap(uint256,uint256)' 'InsufficientFunds(uint256,uint256)' 'NoTicketsBought()' 'TokenChanged()' \
           'NoRoute()' 'BadRoute()' 'BadAmount()' 'Insufficient()' 'StaleOracle()' 'SequencerDown()' 'Slippage(uint256,uint256)' \
           'Unauthorized()' 'NotOwner()' 'NotEnoughGrandfatheredTickets(uint256,uint256)'; do
    [[ "$(cast sig "$e")" == "$sel" ]] && { echo "${e%%(*}"; return; }
  done
  echo "$1" | cut -c1-120
}

cmd="${1:-}"; shift || true
case "$cmd" in
  deploy)
    yes_flag "$@"
    signer FF_OWNER; owner="$SIGNER"
    echo "deploy FastFeedTicketVault(tickets=$TICKETS, owner=$owner, poolManager=$POOL_MANAGER, weth=$WETH, sequencerFeed=$SEQUENCER_FEED) on $RPC"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    confirm "Deploy to Arbitrum One with this wallet?" deploy
    forge create src/FastFeedTicketVault.sol:FastFeedTicketVault "${SIGN[@]}" \
      --root contracts --rpc-url "$RPC" --broadcast --constructor-args "$TICKETS" "$owner" "$POOL_MANAGER" "$WETH" "$SEQUENCER_FEED" ;;
  setup)
    vault="${1:?vault address}"; shift
    tickets=1; maxp=""; renewals=""; deposit="0"; budget=""; expiry_days=0; yes=0
    while [[ $# -gt 0 ]]; do case "$1" in
      --tickets) tickets="$2"; shift 2;; --max-price) maxp="$2"; shift 2;; --renewals) renewals="$2"; shift 2;;
      --budget) budget="$2"; shift 2;; --expiry-days) expiry_days="$2"; shift 2;; --deposit) deposit="$2"; shift 2;;
      --yes) yes=1; shift;; *) die "unknown option $1";; esac; done
    [[ -n "$maxp" && -n "$renewals" && -n "$budget" ]] || die "--max-price, --renewals and --budget are required"
    [[ -f "$KEY_FILE" ]] || die "no API key: run scripts/fast-feed-ticket.sh keygen first"
    signer FF_OWNER; owner="$SIGNER"
    hash=$(cast keccak "$(cat "$KEY_FILE")"); dep=$(raw "$deposit")
    expiry=0; (( expiry_days > 0 )) && expiry=$(( $(date +%s) + expiry_days * 86400 ))
    [[ "$(view 'owner()(address)')" == "$owner" ]] || die "$owner does not own $vault"
    bal=$(cast call "$USDG" 'balanceOf(address)(uint256)' "$owner" --rpc-url "$RPC" | num)
    echo "owner $owner holds $(usdg "$bal") USDG"
    echo "plan: setConfig(hash $hash, $tickets ticket(s), max $(usdg "$(raw "$maxp")") USDG each, $renewals renewals, budget $(usdg "$(raw "$budget")") USDG, expiry ${expiry})"
    (( dep > 0 )) && echo "      approve + fund $(usdg "$dep") USDG into the vault"
    echo "worst-case spend = min(budget, renewals x tickets x max price) = $(python3 -c "print(min(float('$budget'),$renewals*$tickets*float('$maxp')))") USDG"
    (( bal >= dep )) || die "wallet holds less than the deposit"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    confirm "Send setConfig (+ fund) with REAL funds?" setup
    cast send "$vault" 'setConfig(bytes32,uint256,uint256,uint256,uint256,uint256)' "$hash" "$tickets" "$(raw "$maxp")" "$renewals" "$(raw "$budget")" "$expiry" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    if (( dep > 0 )); then
      cast send "$USDG" 'approve(address,uint256)' "$vault" "$dep" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
      cast send "$vault" 'fund(uint256)' "$dep" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    fi
    echo "configured. Next: route (whitelist USDC/ETH) if you want conversions; then start the keeper:  FF_KEEPER_ACCOUNT=<name> $0 keeper $vault --loop 300" ;;
  route)
    vault="${1:?vault}"; which="${2:?usdc|eth|both}"; yes_flag "$@"
    signer FF_OWNER
    [[ "$(view 'owner()(address)')" == "$SIGNER" ]] || die "$SIGNER does not own $vault"
    echo "whitelist for conversion to USDG: $which"
    echo "  usdc: 1 hop through the hooked V4 USDG/USDC pool, 1:1 bound, 10 bps max slippage, 50,000 USDC per call"
    echo "  eth : WETH->USDC (Uniswap V3 0.05%) then USDC->USDG, Chainlink ETH/USD bound (fresh within 25 h, sequencer up), 100 bps, 25 ETH per call"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    confirm "Send the route transaction(s)?" route
    [[ "$which" == usdc || "$which" == both ]] && cast send "$vault" "$ROUTE_SIG" "$USDC" "$(route_usdc)" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    [[ "$which" == eth || "$which" == both ]] && cast send "$vault" "$ROUTE_SIG" "$ZERO" "$(route_eth)" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    echo "routes set. Anyone can now send USDC or ETH to the vault; a keeper converts it." ;;
  rotate-key)
    vault="${1:?vault}"; newfile="${2:?new key file}"; yes_flag "$@"
    [[ -f "$newfile" ]] || die "no such key file (create one: FF_KEY_FILE=$newfile scripts/fast-feed-ticket.sh keygen)"
    signer FF_OWNER; hash=$(cast keccak "$(cat "$newfile")")
    echo "queue API-key hash $hash for the next renewal (tickets already bought keep the old key until their round ends; there is no revoke)"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    cast send "$vault" 'rotateKey(bytes32)' "$hash" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null && echo "queued. After the next renewal, move FF_KEY_FILE to the new file." ;;
  withdraw)
    vault="${1:?vault}"; amt="${2:?usdg amount}"; yes_flag "$@"
    signer FF_OWNER
    echo "withdraw $amt USDG from the vault to the owner"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    cast send "$vault" 'withdraw(uint256)' "$(raw "$amt")" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null; echo "withdrawn" ;;
  exit)
    vault="${1:?vault}"; yes_flag "$@"
    signer FF_OWNER
    echo "EXIT: stop renewals, pull back anything inside the Tickets contract, send ALL the vault's USDG to $SIGNER"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    confirm "Exit the vault?" exit
    cast send "$vault" 'exit()' "${SIGN[@]}" --rpc-url "$RPC" >/dev/null; echo "exited. Other tokens/ETH: cast send $vault 'sweep(address)' <token>  /  'sweepEth()'" ;;
  status)
    vault="${1:?vault}"
    mapfile -t f <<<"$(view 'status()(uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,bool)' | num)"
    now=$(date +%s); end=$(cast call "$TICKETS" 'roundEnd()(uint256)' --rpc-url "$RPC" | num)
    echo "round ${f[0]}; price $(usdg "${f[1]}") USDG; tickets held this round ${f[2]} (grandfathered into the next: ${f[3]})"
    echo "vault USDG $(usdg "${f[4]}") (inside Tickets: $(usdg "${f[5]}")); renewals left ${f[6]}; budget left $(usdg "${f[7]}") USDG; ready to renew: ${f[8]}"
    echo "ETH $(cast balance "$vault" --ether --rpc-url "$RPC"), USDC $(usdg "$(cast call "$USDC" 'balanceOf(address)(uint256)' "$vault" --rpc-url "$RPC" | num)") (convertible if whitelisted)"
    echo "round ends in $(( (end-now)/3600 ))h $(( (end-now)%3600/60 ))m" ;;
  convert)
    vault="${1:?vault}"; tok="${2:?usdc|eth}"; amt="${3:?amount}"
    signer FF_KEEPER
    case "$tok" in usdc) addr=$USDC; wei=$(raw "$amt");; eth) addr=$ZERO; wei=$(python3 -c "print(int(float('$amt')*10**18))");; *) die "usdc or eth";; esac
    cast call "$vault" 'convert(address,uint256)(uint256)' "$addr" "$wei" --from "$SIGNER" --rpc-url "$RPC" >/dev/null || die "simulation says convert would revert (not whitelisted, over the per-call cap, oracle stale, or pool refuses)"
    cast send "$vault" 'convert(address,uint256)' "$addr" "$wei" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null && echo "converted" ;;
  keeper)
    vault="${1:?vault}"; shift; loop=0
    [[ "${1:-}" == --loop ]] && loop="${2:?seconds}"
    signer FF_KEEPER   # a key that holds only gas ETH
    log() { echo "$(date -u +%FT%TZ) $*"; }
    sim_send() { # name sig args...  : simulate as the keeper, send only if the simulation succeeds
      local name="$1"; shift
      if ! cast call "$vault" "$@" --from "$SIGNER" --rpc-url "$RPC" >/dev/null 2>/tmp/ff-keeper.err; then
        log "$name not possible now: $(errname "$(cat /tmp/ff-keeper.err)")"; return 1
      fi
      if out=$(cast send "$vault" "$@" "${SIGN[@]}" --rpc-url "$RPC" 2>&1); then
        log "$name sent: $(echo "$out" | awk '/^transactionHash/ {print $2}')"; return 0
      else log "$name failed: $(echo "$out" | tail -1 | cut -c1-120)"; return 1; fi
    }
    tick() {
      mapfile -t f <<<"$(cast call "$vault" 'status()(uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,bool)' --rpc-url "$RPC" | awk '{print $1}')"
      price=${f[1]}; held=${f[2]}; usdg_held=${f[4]}; can=${f[8]}
      # Short of USDG for the next ticket? Convert whitelisted deposits first (each simulated before sending).
      if [[ "$can" != true ]] && (( usdg_held < price )); then
        usdc_bal=$(cast call "$USDC" 'balanceOf(address)(uint256)' "$vault" --rpc-url "$RPC" | num)
        (( usdc_bal >= 1000000 )) && sim_send "convert USDC" 'convert(address,uint256)(uint256)' "$USDC" "$usdc_bal" || true
        eth_bal=$(cast balance "$vault" --rpc-url "$RPC")
        (( eth_bal >= 500000000000000 )) && sim_send "convert ETH" 'convert(address,uint256)(uint256)' "$ZERO" "$eth_bal" || true
      fi
      sim_send "renew" 'renew()' || true
    }
    if (( loop )); then while true; do tick || true; sleep "$loop"; done; else tick; fi ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
