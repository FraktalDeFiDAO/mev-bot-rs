#!/usr/bin/env bash
# Deploy ArbExecutor to Arbitrum using secrets from .env (never printed), then record its address in .env.
#   scripts/deploy-executor.sh             first deployment
#   scripts/deploy-executor.sh --redeploy  replace an existing executor (old address kept as a comment)
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
: "${ARBITRUM_HTTP_URL:?set in .env}" "${PRIVATE_KEY:?set in .env}" "${PROFIT_RECEIVER:?set in .env}"
export PATH="$HOME/.foundry/bin:$PATH"
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
AAVE_POOL=0x794a61358D6845594F94dc1DB02A252b5b4814aD
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
POOL_MANAGER=0x360E68faCcca8cA495c1B759Fd9EEe466db9FB32   # Uniswap V4 PoolManager (Arbitrum One)

DEPLOYER=$(cast wallet address --private-key "$PRIVATE_KEY")
BAL=$(cast balance "$DEPLOYER" --rpc-url "$ARBITRUM_HTTP_URL" --ether)
echo "deployer (owner): $DEPLOYER   balance: $BAL ETH"
echo "profit receiver:  $PROFIT_RECEIVER"
REDEPLOY=0; [[ ${1:-} == --redeploy ]] && REDEPLOY=1
if grep -q '^EXECUTOR_ADDRESS=0x' .env && [[ $REDEPLOY == 0 ]]; then
  echo "EXECUTOR_ADDRESS already set in .env — pass --redeploy to replace it"; exit 1
fi

echo "running fork tests first..."
(cd contracts && ARBITRUM_HTTP_URL="$ARBITRUM_HTTP_URL" forge test -q)

read -r -p "Deploy ArbExecutor to Arbitrum mainnet now? [y/N] " ok
[[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }

OUT=$(cd contracts && forge create src/ArbExecutor.sol:ArbExecutor --rpc-url "$ARBITRUM_HTTP_URL" \
  --private-key "$PRIVATE_KEY" --broadcast --constructor-args "$VAULT" "$WETH" "$PROFIT_RECEIVER")
ADDR=$(echo "$OUT" | awk '/Deployed to:/ {print $3}')
TX=$(echo "$OUT" | awk '/Transaction hash:/ {print $3}')
[[ -n $ADDR ]] || { echo "$OUT" | grep -v -i private; echo "deploy failed"; exit 1; }
if [[ $REDEPLOY == 1 ]]; then
  sed -i "s/^EXECUTOR_ADDRESS=/# replaced $(date -u +%F): EXECUTOR_ADDRESS=/" .env
fi
echo "EXECUTOR_ADDRESS=$ADDR" >> .env
echo "deployed: $ADDR   tx: $TX"
echo "verify owner: $(cast call "$ADDR" 'owner()(address)' --rpc-url "$ARBITRUM_HTTP_URL")"
# Uniswap V4 hops need the PoolManager set (owner-only, one tx).
cast send "$ADDR" 'setPoolManager(address)' "$POOL_MANAGER" --private-key "$PRIVATE_KEY" --rpc-url "$ARBITRUM_HTTP_URL" >/dev/null
echo "pool manager: $(cast call "$ADDR" 'poolManager()(address)' --rpc-url "$ARBITRUM_HTTP_URL")"
echo "next: set executor_batch = true and executor_v4 = true in config/arbitrum.toml (this executor has batch + V4 hops), then deploy the bot"
# Allowlist the liquidation targets configured by default (Aave v3 pool).
scripts/allow-targets.sh "$AAVE_POOL"
