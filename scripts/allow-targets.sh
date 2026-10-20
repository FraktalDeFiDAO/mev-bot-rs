#!/usr/bin/env bash
# Allowlist liquidation call targets / approval spenders on the executor (owner-only tx each).
#   scripts/allow-targets.sh <address> [address...]
#   scripts/allow-targets.sh --revoke <address>
# Reads PRIVATE_KEY and, by CHAIN (arbitrum default | base), ARBITRUM_HTTP_URL + EXECUTOR_ADDRESS or
# BASE_HTTP_URL (public RPC if unset) + BASE_EXECUTOR_ADDRESS from .env (never printed).
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
export PATH="$HOME/.foundry/bin:$PATH"
: "${PRIVATE_KEY:?set in .env}"
case ${CHAIN:-arbitrum} in
  arbitrum) : "${EXECUTOR_ADDRESS:?set in .env}" "${ARBITRUM_HTTP_URL:?set in .env}"; EXEC=$EXECUTOR_ADDRESS; RPC=$ARBITRUM_HTTP_URL ;;
  base) : "${BASE_EXECUTOR_ADDRESS:?set in .env}"; EXEC=$BASE_EXECUTOR_ADDRESS; RPC=${BASE_HTTP_URL:-https://mainnet.base.org} ;;
  *) echo "unknown CHAIN ${CHAIN}"; exit 2 ;;
esac
ALLOW=true
if [[ ${1:-} == --revoke ]]; then ALLOW=false; shift; fi
[[ $# -gt 0 ]] || { sed -n '2,6p' "$0"; exit 2; }
for t in "$@"; do
  current=$(cast call "$EXEC" 'allowedTarget(address)(bool)' "$t" --rpc-url "$RPC")
  if [[ $current == "$ALLOW" ]]; then echo "$t already allowed=$ALLOW"; continue; fi
  cast send "$EXEC" 'setTarget(address,bool)' "$t" "$ALLOW" \
    --private-key "$PRIVATE_KEY" --rpc-url "$RPC" >/dev/null
  echo "$t allowed=$(cast call "$EXEC" 'allowedTarget(address)(bool)' "$t" --rpc-url "$RPC")"
done
