#!/usr/bin/env bash
# Deploy ArbExecutor using secrets from .env (never printed), then record its address in .env.
#   scripts/deploy-executor.sh                         first deployment on Arbitrum (EXECUTOR_ADDRESS)
#   scripts/deploy-executor.sh --redeploy              replace an existing executor (old address kept as a comment)
#   scripts/deploy-executor.sh --chain base [--redeploy]   Base (BASE_EXECUTOR_ADDRESS; BASE_HTTP_URL or the public RPC)
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
export PATH="$HOME/.foundry/bin:$PATH"
CHAIN=arbitrum; REDEPLOY=0
while [[ $# -gt 0 ]]; do
  case $1 in
    --redeploy) REDEPLOY=1 ;;
    --chain) CHAIN=$2; shift ;;
    *) sed -n '2,5p' "$0"; exit 2 ;;
  esac
  shift
done
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8   # Balancer V2 vault (same address on both chains)
case $CHAIN in
  arbitrum)
    : "${ARBITRUM_HTTP_URL:?set in .env}"; RPC=$ARBITRUM_HTTP_URL; KEY=EXECUTOR_ADDRESS
    AAVE_POOL=0x794a61358D6845594F94dc1DB02A252b5b4814aD
    WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
    POOL_MANAGER=0x360E68faCcca8cA495c1B759Fd9EEe466db9FB32   # Uniswap V4 PoolManager (Arbitrum One)
    ALLOW=("$AAVE_POOL") ;;
  base)
    RPC=${BASE_HTTP_URL:-https://mainnet.base.org}; KEY=BASE_EXECUTOR_ADDRESS
    WETH=0x4200000000000000000000000000000000000006
    POOL_MANAGER=0x498581fF718922c3f8e6A244956aF099B2652b2b   # Uniswap V4 PoolManager (Base)
    # Morpho Blue; Moonwell's markets: the bot prints the scripts/allow-targets.sh command at startup.
    ALLOW=(0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb) ;;
  *) echo "unknown chain $CHAIN (arbitrum | base)"; exit 2 ;;
esac
: "${PRIVATE_KEY:?set in .env}" "${PROFIT_RECEIVER:?set in .env}"
[[ $(cast chain-id --rpc-url "$RPC") == "$([[ $CHAIN == base ]] && echo 8453 || echo 42161)" ]] || { echo "RPC is not $CHAIN"; exit 1; }

DEPLOYER=$(cast wallet address --private-key "$PRIVATE_KEY")
BAL=$(cast balance "$DEPLOYER" --rpc-url "$RPC" --ether)
echo "chain: $CHAIN   deployer (owner): $DEPLOYER   balance: $BAL ETH"
echo "profit receiver:  $PROFIT_RECEIVER"
if grep -q "^$KEY=0x" .env && [[ $REDEPLOY == 0 ]]; then
  echo "$KEY already set in .env — pass --redeploy to replace it"; exit 1
fi

echo "running fork tests first..."
# FORK_RPC_URL (set when calling, not in .env) overrides a keyed ARBITRUM_HTTP_URL whose quota is spent.
(cd contracts && ARBITRUM_HTTP_URL="${FORK_RPC_URL:-${ARBITRUM_HTTP_URL:-https://arb1.arbitrum.io/rpc}}" forge test -q) \
  || { echo "fork tests failed (a spent RPC quota? retry with FORK_RPC_URL=https://arb1.arbitrum.io/rpc)"; exit 1; }

read -r -p "Deploy ArbExecutor to $CHAIN mainnet now? [y/N] " ok
[[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }

OUT=$(cd contracts && forge create src/ArbExecutor.sol:ArbExecutor --rpc-url "$RPC" \
  --private-key "$PRIVATE_KEY" --broadcast --constructor-args "$VAULT" "$WETH" "$PROFIT_RECEIVER")
ADDR=$(echo "$OUT" | awk '/Deployed to:/ {print $3}')
TX=$(echo "$OUT" | awk '/Transaction hash:/ {print $3}')
[[ -n $ADDR ]] || { echo "$OUT" | grep -v -i private; echo "deploy failed"; exit 1; }
if [[ $REDEPLOY == 1 ]]; then
  sed -i "s/^$KEY=/# replaced $(date -u +%F): $KEY=/" .env
fi
echo "$KEY=$ADDR" >> .env
echo "deployed: $ADDR   tx: $TX"
echo "verify owner: $(cast call "$ADDR" 'owner()(address)' --rpc-url "$RPC")"
# Uniswap V4 hops need the PoolManager set (owner-only, one tx).
cast send "$ADDR" 'setPoolManager(address)' "$POOL_MANAGER" --private-key "$PRIVATE_KEY" --rpc-url "$RPC" >/dev/null
echo "pool manager: $(cast call "$ADDR" 'poolManager()(address)' --rpc-url "$RPC")"
echo "next: executor_batch / executor_v4 = true in config/$CHAIN.toml (this executor has batch + V4 hops), then deploy the bot"
# Allowlist the default liquidation targets.
CHAIN=$CHAIN scripts/allow-targets.sh "${ALLOW[@]}"
