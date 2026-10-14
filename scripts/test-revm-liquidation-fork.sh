#!/usr/bin/env bash
# JEV-LIQ-015 anvil-fork tests: REVM exact-call simulation vs anvil eth_call vs mined receipt.
# Run in the dev container:  scripts/dev.sh bash scripts/test-revm-liquidation-fork.sh
#
#   1. anvil fork of Arbitrum (chain id 31337)  2. deploy ArbExecutor, allowlist the Aave pool
#   3. borrower opens a 5 WETH / USDC position near the limit
#   4. install MockPriceSource over the WETH Aave price source (price in slot 0)
#   5. run the ignored `anvil_*` tests in crates/mev-localsim/tests/liquidation_fork.rs
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"
export ETH_RPC_TIMEOUT=240
FORK_URL="${E2E_FORK_URL:-https://arb1.arbitrum.io/rpc}"
PORT=8549
RPC="http://127.0.0.1:$PORT"
MNEMONIC="test test test test test test test test test test test junk"   # anvil's public dev mnemonic
OWNER_KEY=$(cast wallet private-key --mnemonic "$MNEMONIC" --mnemonic-index 0)
BORROWER_KEY=$(cast wallet private-key --mnemonic "$MNEMONIC" --mnemonic-index 1)
OWNER=$(cast wallet address --private-key "$OWNER_KEY")
BORROWER=$(cast wallet address --private-key "$BORROWER_KEY")
RECEIVER=0x000000000000000000000000000000000000bEEF
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
USDC=0xaf88d065e77c8cC2239327C5EDb3A432268e5831
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
AAVE_POOL=0x794a61358D6845594F94dc1DB02A252b5b4814aD
AAVE_ORACLE=0xb56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7
pid=
cleanup() { [[ -n $pid ]] && kill "$pid" 2>/dev/null || true; }
trap cleanup EXIT
send() {
  local out
  out=$(cast send "$@" --rpc-url "$RPC" 2>&1) && ! grep -qE '^status +0' <<<"$out" || { echo "tx failed: $* :: $out" >&2; exit 1; }
}

anvil --fork-url "$FORK_URL" --chain-id 31337 --port "$PORT" --silent &
pid=$!
for _ in $(seq 60); do cast block-number --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 1; done
echo "fork block: $(cast block-number --rpc-url "$RPC")"

OUT=$(cd contracts && forge create src/ArbExecutor.sol:ArbExecutor --rpc-url "$RPC" --private-key "$OWNER_KEY" \
  --broadcast --constructor-args "$VAULT" "$WETH" "$RECEIVER" 2>&1)
EXEC=$(echo "$OUT" | awk '/Deployed to:/ {print $3}')
send "$EXEC" 'setTarget(address,bool)' "$AAVE_POOL" true --private-key "$OWNER_KEY"

send "$WETH" 'deposit()' --value 5ether --private-key "$BORROWER_KEY"
send "$WETH" 'approve(address,uint256)' "$AAVE_POOL" 5ether --private-key "$BORROWER_KEY"
send "$AAVE_POOL" 'supply(address,uint256,address,uint16)' "$WETH" 5ether "$BORROWER" 0 --private-key "$BORROWER_KEY"
AVAIL=$(cast call "$AAVE_POOL" 'getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)' "$BORROWER" --rpc-url "$RPC" | sed -n 3p | cut -d' ' -f1)
BORROW=$(python3 -c "print(int($AVAIL) * 99 // 100 // 100)")   # base (8 dec USD) -> USDC (6 dec), 99%
send "$AAVE_POOL" 'borrow(address,uint256,uint256,uint16,address)' "$USDC" "$BORROW" 2 0 "$BORROWER" --private-key "$BORROWER_KEY"

SRC=$(cast call "$AAVE_ORACLE" 'getSourceOfAsset(address)(address)' "$WETH" --rpc-url "$RPC")
PX=$(cast call "$AAVE_ORACLE" 'getAssetPrice(address)(uint256)' "$WETH" --rpc-url "$RPC" | cut -d' ' -f1)
CODE=$(cd contracts && forge inspect test/mocks/MockPriceSource.sol:MockPriceSource deployedBytecode)
cast rpc anvil_setCode "$SRC" "$CODE" --rpc-url "$RPC" >/dev/null
cast rpc anvil_setStorageAt "$SRC" 0x0 "$(cast to-uint256 "$PX")" --rpc-url "$RPC" >/dev/null
cast rpc evm_mine --rpc-url "$RPC" >/dev/null
echo "executor=$EXEC borrower=$BORROWER weth_price=$PX source=$SRC"

REVM_FORK_RPC="$RPC" REVM_FORK_EXECUTOR="$EXEC" REVM_FORK_OWNER="$OWNER" REVM_FORK_BORROWER="$BORROWER" \
REVM_FORK_RECEIVER="$RECEIVER" REVM_FORK_PRICE_SRC="$SRC" REVM_FORK_PX0="$PX" \
  cargo test -q -p mev-localsim --test liquidation_fork -- --ignored --nocapture --test-threads=1 anvil_
