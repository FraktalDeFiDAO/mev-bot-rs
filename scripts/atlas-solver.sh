#!/usr/bin/env bash
# Bring the optional Atlas/SVR solver online step by step. Every step that spends asks first.
#   scripts/atlas-solver.sh status            read-only: owner, bond, float, caps, allowlist
#   scripts/atlas-solver.sh check             read-only: EIP-712 digest vs the deployed AtlasVerification
#   scripts/atlas-solver.sh deploy            deploy AtlasSolver (records ATLAS_SOLVER_ADDRESS in .env)
#   scripts/atlas-solver.sh allow             allowlist the executor's liquidation targets on the solver
#   scripts/atlas-solver.sh bond <ETH>        Atlas.depositAndBond (recommended 0.1 ETH): pays solver gas
#   scripts/atlas-solver.sh float <ETH>       wrap ETH to WETH and send it to the solver: the bid float
#   scripts/atlas-solver.sh cap <ETH>         solver.setMaxBid: the largest bid the contract will pay
#   scripts/atlas-solver.sh unbond <ETH>      start withdrawing bonded ETH (escrow delay, then `redeem`)
# Reads ARBITRUM_HTTP_URL, PRIVATE_KEY, PROFIT_RECEIVER from .env (never printed). After these steps
# set `[atlas] bid = true` and `solver = "<address>"` in config/arbitrum.toml (docs/research/ATLAS.md).
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env; set +a
export PATH="$HOME/.foundry/bin:$PATH"
: "${ARBITRUM_HTTP_URL:?set in .env}" "${PRIVATE_KEY:?set in .env}"
RPC=$ARBITRUM_HTTP_URL
ATLAS=0x8ad1aE9D97C79aA68A0a151E83ff3942f68F86C1
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
AAVE_POOL=0x794a61358D6845594F94dc1DB02A252b5b4814aD
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
OWNER=$(cast wallet address --private-key "$PRIVATE_KEY")
SOLVER=${ATLAS_SOLVER_ADDRESS:-}
need_solver() { [[ -n $SOLVER ]] || { echo "ATLAS_SOLVER_ADDRESS not set: run deploy first"; exit 1; }; }
confirm() { read -r -p "$1 [y/N] " ok; [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }; }
send() { cast send "$@" --private-key "$PRIVATE_KEY" --rpc-url "$RPC" >/dev/null; }

case "${1:-}" in
  status)
    echo "owner / bonded EOA:   $OWNER   balance $(cast balance "$OWNER" --rpc-url "$RPC" --ether) ETH"
    echo "bonded on Atlas:      $(cast call "$ATLAS" 'balanceOfBonded(address)(uint256)' "$OWNER" --rpc-url "$RPC" | awk '{print $1}') wei"
    if [[ -n $SOLVER ]]; then
      echo "solver:               $SOLVER (owner $(cast call "$SOLVER" 'owner()(address)' --rpc-url "$RPC"))"
      echo "  maxBid (wei):       $(cast call "$SOLVER" 'maxBid()(uint256)' --rpc-url "$RPC" | awk '{print $1}')"
      echo "  WETH float (wei):   $(cast call "$WETH" 'balanceOf(address)(uint256)' "$SOLVER" --rpc-url "$RPC" | awk '{print $1}')"
      echo "  Aave pool allowed:  $(cast call "$SOLVER" 'allowedTarget(address)(bool)' "$AAVE_POOL" --rpc-url "$RPC")"
    else echo "solver:               not deployed (ATLAS_SOLVER_ADDRESS unset)"; fi ;;
  check)
    cargo run -q -p mev-atlas --bin mev-atlas-check -- --rpc "$RPC" ;;
  deploy)
    [[ -z $SOLVER ]] || { echo "ATLAS_SOLVER_ADDRESS already set ($SOLVER)"; exit 1; }
    : "${PROFIT_RECEIVER:?set in .env}"
    echo "running contract tests first..."
    (cd contracts && ARBITRUM_HTTP_URL="$RPC" forge test -q)
    echo "deployer/owner $OWNER, profit receiver $PROFIT_RECEIVER, atlas $ATLAS"
    confirm "Deploy AtlasSolver to Arbitrum mainnet now?"
    OUT=$(cd contracts && forge create src/AtlasSolver.sol:AtlasSolver --rpc-url "$RPC" --private-key "$PRIVATE_KEY" \
      --broadcast --constructor-args "$VAULT" "$WETH" "$PROFIT_RECEIVER" "$ATLAS")
    ADDR=$(echo "$OUT" | awk '/Deployed to:/ {print $3}')
    [[ -n $ADDR ]] || { echo "$OUT" | grep -v -i private; echo "deploy failed"; exit 1; }
    echo "ATLAS_SOLVER_ADDRESS=$ADDR" >> .env
    echo "deployed AtlasSolver: $ADDR  (next: allow, cap, float, bond)" ;;
  allow)
    need_solver
    for t in "$AAVE_POOL"; do   # extend with the other protocol targets the executor allows
      send "$SOLVER" 'setTarget(address,bool)' "$t" true; echo "allowed $t"
    done ;;
  bond)
    need_solver; AMT=${2:?amount in ETH}
    confirm "Bond $AMT ETH from $OWNER on Atlas (it pays solver gas; unbond has an escrow delay)?"
    send "$ATLAS" 'depositAndBond(uint256)' "$(cast to-wei "$AMT")" --value "$(cast to-wei "$AMT")"
    echo "bonded: $(cast call "$ATLAS" 'balanceOfBonded(address)(uint256)' "$OWNER" --rpc-url "$RPC" | awk '{print $1}') wei" ;;
  float)
    need_solver; AMT=${2:?amount in ETH}
    confirm "Wrap $AMT ETH and send it to the solver as the bid float?"
    send "$WETH" 'deposit()' --value "$(cast to-wei "$AMT")"
    send "$WETH" 'transfer(address,uint256)' "$SOLVER" "$(cast to-wei "$AMT")"
    echo "float: $(cast call "$WETH" 'balanceOf(address)(uint256)' "$SOLVER" --rpc-url "$RPC" | awk '{print $1}') wei" ;;
  cap)
    need_solver; AMT=${2:?amount in ETH}
    confirm "Set the solver's maxBid to $AMT ETH (also set [atlas] max_bid_eth to match)?"
    send "$SOLVER" 'setMaxBid(uint256)' "$(cast to-wei "$AMT")"; echo "maxBid set" ;;
  unbond)
    AMT=${2:?amount in ETH}
    confirm "Start unbonding $AMT ETH (redeem after Atlas's escrow duration)?"
    send "$ATLAS" 'unbond(uint256)' "$(cast to-wei "$AMT")"; echo "unbond started" ;;
  *) sed -n '2,12p' "$0"; exit 2 ;;
esac
