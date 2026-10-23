#!/usr/bin/env bash
# Allowlist liquidation call targets / approval spenders on the executor (owner-only tx each).
#   scripts/allow-targets.sh [--chain arbitrum|base] <address> [address...]
#   scripts/allow-targets.sh [--chain arbitrum|base] --revoke <address>
# Reads PRIVATE_KEY and EXECUTOR_ADDRESS + ARBITRUM_HTTP_URL (arbitrum) or BASE_EXECUTOR_ADDRESS +
# BASE_HTTP_URL / the public RPC (base) from .env (never printed). Targets already in the wanted state are skipped.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
export PATH="$HOME/.foundry/bin:$PATH"
CHAIN=arbitrum; ALLOW=true
while [[ ${1:-} == --* ]]; do
  case $1 in
    --chain) CHAIN=$2; shift ;;
    --revoke) ALLOW=false ;;
    *) sed -n '2,6p' "$0"; exit 2 ;;
  esac
  shift
done
case $CHAIN in
  arbitrum) : "${EXECUTOR_ADDRESS:?set in .env}" "${ARBITRUM_HTTP_URL:?set in .env}"
            EXEC=$EXECUTOR_ADDRESS; RPC=$ARBITRUM_HTTP_URL ;;
  base)     : "${BASE_EXECUTOR_ADDRESS:?set in .env}"
            EXEC=$BASE_EXECUTOR_ADDRESS; RPC=${BASE_HTTP_URL:-https://mainnet.base.org} ;;
  *) echo "unknown chain $CHAIN" >&2; exit 2 ;;
esac
: "${PRIVATE_KEY:?set in .env}"
[[ $# -gt 0 ]] || { sed -n '2,6p' "$0"; exit 2; }
for t in "$@"; do
  current=$(cast call "$EXEC" 'allowedTarget(address)(bool)' "$t" --rpc-url "$RPC")
  if [[ $current == "$ALLOW" ]]; then echo "$t already allowed=$ALLOW"; continue; fi
  cast send "$EXEC" 'setTarget(address,bool)' "$t" "$ALLOW" \
    --private-key "$PRIVATE_KEY" --rpc-url "$RPC" >/dev/null
  echo "$t allowed=$(cast call "$EXEC" 'allowedTarget(address)(bool)' "$t" --rpc-url "$RPC")"
done
