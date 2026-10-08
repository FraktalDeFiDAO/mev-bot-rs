#!/usr/bin/env bash
# Fast Feed ticket vault: deploy, configure, and keep it renewed. See docs/runbooks/fast-feed-experiment.md.
# The vault (contracts/src/FastFeedTicketVault.sol) holds the USDG deposit and is the ticket subscriber;
# anyone can call renew(), only the owner can move money. So the KEEPER KEY NEEDS GAS ONLY.
#
#   OPERATOR (owner key, run by you):
#   scripts/fast-feed-vault.sh deploy [--yes]
#   scripts/fast-feed-vault.sh setup <vault> --tickets N --max-price USDG --renewals R --deposit USDG [--yes]
#   scripts/fast-feed-vault.sh withdraw <vault> <usdg> [--yes]
#   KEEPER (gas-only key; cron/systemd):
#   scripts/fast-feed-vault.sh keeper <vault> [--loop SECS]
#   ANYONE:
#   scripts/fast-feed-vault.sh status <vault>
#
# Environment: FF_RPC, FF_KEY_FILE (API key file from fast-feed-ticket.sh keygen).
#   Signers: FF_OWNER_ACCOUNT / FF_KEEPER_ACCOUNT, names of Foundry keystore accounts (see signer()). Every sending command
#   is a dry run unless --yes. Unattended keeper: FF_PASSWORD_FILE=<mode-600 file with the keystore password>.
set -euo pipefail
export PATH="$HOME/.foundry/bin:$PATH"
cd "$(dirname "$0")/.."
RPC="${FF_RPC:-https://arb1.arbitrum.io/rpc}"
KEY_FILE="${FF_KEY_FILE:-$HOME/.config/mev-bot/fast-feed.key}"
TICKETS=0x451DD62C26b753d07d6104aaC6865AaE73a6A598
USDG=0x004B506865409877C9fA29bfb1ebA929984B9bbC
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

cmd="${1:-}"; shift || true
case "$cmd" in
  deploy)
    yes=0; [[ "${1:-}" == --yes ]] && yes=1
    signer FF_OWNER   # the wallet that will own the vault and fund it
    owner="$SIGNER"
    echo "deploy FastFeedTicketVault(tickets=$TICKETS, owner=$owner) on $RPC"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    read -r -p "Deploy to Arbitrum One with this wallet? type 'deploy': " ok; [[ "$ok" == deploy ]] || die aborted
    forge create src/FastFeedTicketVault.sol:FastFeedTicketVault "${SIGN[@]}" \
      --root contracts --rpc-url "$RPC" --broadcast --constructor-args "$TICKETS" "$owner" ;;
  setup)
    vault="${1:?vault address}"; shift
    tickets=1; maxp=""; renewals=""; deposit=""; yes=0
    while [[ $# -gt 0 ]]; do case "$1" in
      --tickets) tickets="$2"; shift 2;; --max-price) maxp="$2"; shift 2;; --renewals) renewals="$2"; shift 2;;
      --deposit) deposit="$2"; shift 2;; --yes) yes=1; shift;; *) die "unknown option $1";; esac; done
    [[ -n "$maxp" && -n "$renewals" && -n "$deposit" ]] || die "--max-price, --renewals and --deposit are required"
    [[ -f "$KEY_FILE" ]] || die "no API key: run scripts/fast-feed-ticket.sh keygen first"
    signer FF_OWNER; owner="$SIGNER"
    hash=$(cast keccak "$(cat "$KEY_FILE")"); dep=$(raw "$deposit")
    [[ "$(cast call "$vault" 'owner()(address)' --rpc-url "$RPC")" == "$owner" ]] || die "$owner does not own $vault"
    bal=$(cast call "$USDG" 'balanceOf(address)(uint256)' "$owner" --rpc-url "$RPC" | num)
    echo "owner $owner holds $(usdg "$bal") USDG"
    echo "plan: setConfig(hash $hash, $tickets ticket(s), max $(usdg "$(raw "$maxp")") USDG each, $renewals renewals); approve + fund $(usdg "$dep") USDG"
    echo "worst-case spend: renewals x tickets x max price = $(python3 -c "print($renewals*$tickets*float('$maxp'))") USDG, bounded by the deposit"
    (( bal >= dep )) || die "wallet holds less than the deposit"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    read -r -p "Send setConfig + approve + fund with REAL funds? type 'setup': " ok; [[ "$ok" == setup ]] || die aborted
    cast send "$vault" 'setConfig(bytes32,uint256,uint256,uint256)' "$hash" "$tickets" "$(raw "$maxp")" "$renewals" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    cast send "$USDG" 'approve(address,uint256)' "$vault" "$dep" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    cast send "$vault" 'fund(uint256)' "$dep" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    echo "configured and funded. Start the keeper:  FF_KEEPER_ACCOUNT=<name> $0 keeper $vault --loop 300" ;;
  withdraw)
    vault="${1:?vault}"; amt="${2:?usdg amount}"; yes=0; [[ "${3:-}" == --yes ]] && yes=1
    signer FF_OWNER
    echo "withdraw $amt USDG from the vault's Tickets deposit to the owner"
    (( yes )) || { echo "DRY RUN: nothing sent. Re-run with --yes."; exit 0; }
    cast send "$vault" 'withdraw(uint256)' "$(raw "$amt")" "${SIGN[@]}" --rpc-url "$RPC" >/dev/null
    echo "withdrawn" ;;
  status)
    vault="${1:?vault}"
    s=$(cast call "$vault" 'status()(uint256,uint256,uint256,uint256,uint256,uint256,bool)' --rpc-url "$RPC")
    mapfile -t f <<<"$(echo "$s" | awk '{print $1}')"
    now=$(date +%s); end=$(cast call "$TICKETS" 'roundEnd()(uint256)' --rpc-url "$RPC" | num)
    echo "round ${f[0]}; price $(usdg "${f[1]}") USDG; tickets held this round ${f[2]} (grandfathered into the next: ${f[3]})"
    echo "vault deposit $(usdg "${f[4]}") USDG; renewals left ${f[5]}; can renew now: ${f[6]}"
    echo "round ends in $(( (end-now)/3600 ))h $(( (end-now)%3600/60 ))m" ;;
  keeper)
    vault="${1:?vault}"; shift; loop=0
    [[ "${1:-}" == --loop ]] && loop="${2:?seconds}"
    signer FF_KEEPER   # a key that holds only gas ETH
    tick() {
      can=$(cast call "$vault" 'status()(uint256,uint256,uint256,uint256,uint256,uint256,bool)' --rpc-url "$RPC" | tail -1)
      if [[ "$can" == true ]]; then
        if out=$(cast send "$vault" 'renew()' "${SIGN[@]}" --rpc-url "$RPC" 2>&1); then echo "$(date -u +%FT%TZ) renewed: $(echo "$out" | grep -E '^transactionHash' | awk '{print $2}')"
        else echo "$(date -u +%FT%TZ) renew failed: $(echo "$out" | tail -1)"; fi
      else echo "$(date -u +%FT%TZ) nothing to do"; fi
    }
    if (( loop )); then while true; do tick || true; sleep "$loop"; done; else tick; fi ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
