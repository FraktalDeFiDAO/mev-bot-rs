#!/usr/bin/env bash
# Allowlist liquidation call targets / approval spenders on the executor (owner-only tx each).
#   scripts/allow-targets.sh <address> [address...]
#   scripts/allow-targets.sh --revoke <address>
# Reads ARBITRUM_HTTP_URL, PRIVATE_KEY, EXECUTOR_ADDRESS from .env (never printed).
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
export PATH="$HOME/.foundry/bin:$PATH"
: "${EXECUTOR_ADDRESS:?set in .env}" "${PRIVATE_KEY:?set in .env}" "${ARBITRUM_HTTP_URL:?set in .env}"
ALLOW=true
if [[ ${1:-} == --revoke ]]; then ALLOW=false; shift; fi
[[ $# -gt 0 ]] || { sed -n '2,5p' "$0"; exit 2; }
for t in "$@"; do
  current=$(cast call "$EXECUTOR_ADDRESS" 'allowedTarget(address)(bool)' "$t" --rpc-url "$ARBITRUM_HTTP_URL")
  if [[ $current == "$ALLOW" ]]; then echo "$t already allowed=$ALLOW"; continue; fi
  cast send "$EXECUTOR_ADDRESS" 'setTarget(address,bool)' "$t" "$ALLOW" \
    --private-key "$PRIVATE_KEY" --rpc-url "$ARBITRUM_HTTP_URL" >/dev/null
  echo "$t allowed=$(cast call "$EXECUTOR_ADDRESS" 'allowedTarget(address)(bool)' "$t" --rpc-url "$ARBITRUM_HTTP_URL")"
done
