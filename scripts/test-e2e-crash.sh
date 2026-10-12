#!/usr/bin/env bash
# Crash-day rehearsal: the real mev-bot (live mode, batching on) faces N Aave v3 positions that all go
# underwater in ONE price drop on an anvil fork of Arbitrum, and must liquidate every one of them.
#   CRASH_N=5 scripts/test-e2e-crash.sh      (N = 2..9 borrowers; default 5)
# Asserts on-chain state (each position's debt falls), then reports how many batches were sent and the
# time from the crash to the last repayment.
#
#   1. anvil fork, chain id 31337 (fork-signed txs are invalid on mainnet)
#   2. deploy executor (dev account 0 = bot owner) and allowlist the Aave pool
#   3. dev account 1 opens a real Aave position: 1 WETH collateral, USDC borrowed near the limit
#   4. start the bot: Aave adapter only, scanning just the fork's own blocks, fast sweeps/polls
#   5. crash ETH/USD: install MockPriceSource code over the Aave WETH price source, set 70% price
#   6. assert: liquidatable -> plan -> simulate -> send -> land -> receiver profit; position repaid
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"
export ETH_RPC_TIMEOUT=240   # a cold fork fetches state from the upstream node on first touch
if [[ -z ${E2E_FORK_URL:-} && -f .env ]]; then
  E2E_FORK_URL=$(grep -E '^ARBITRUM_HTTP_URL=' .env | head -1 | cut -d= -f2-)
fi
FORK_URL="${E2E_FORK_URL:-https://arb1.arbitrum.io/rpc}"
PORT=8549
N="${CRASH_N:-5}"
RPC="http://127.0.0.1:$PORT"
WORK=$(mktemp -d)
MNEMONIC="test test test test test test test test test test test junk"   # anvil's public dev mnemonic
OWNER_KEY=$(cast wallet private-key --mnemonic "$MNEMONIC" --mnemonic-index 0)
RECEIVER=0x000000000000000000000000000000000000bEEF
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
USDC=0xaf88d065e77c8cC2239327C5EDb3A432268e5831
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
AAVE_POOL=0x794a61358D6845594F94dc1DB02A252b5b4814aD
AAVE_ORACLE=0xb56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$WORK"; }
trap cleanup EXIT
fail() { echo "E2E-CRASH FAIL: $*"; echo "--- executor ${EXEC:-unset}: $(cast code "${EXEC:-0x0}" --rpc-url "$RPC" 2>&1 | wc -c) chars of code; owner: $(cast call "${EXEC:-0x0}" "owner()(address)" --rpc-url "$RPC" 2>&1 | tail -1)"; echo "--- bot log tail:"; tail -30 "$WORK/bot.log" 2>/dev/null || true; exit 1; }
# `cast send` exits 0 even when the transaction reverts, so read the receipt status; retry a transient failure.
send() {
  local out try
  for _ in 1 2 3; do
    out=$(cast send "$@" --rpc-url "$RPC" 2>&1) && ! grep -qE '^status +0' <<<"$out" && return 0
    sleep 2
  done
  fail "tx failed after 3 tries: $* :: $(tail -2 <<<"$out" | tr '\n' ' ')"
}

echo "[1/6] anvil fork (chain id 31337)"
anvil --fork-url "$FORK_URL" --chain-id 31337 --port "$PORT" --silent &
pids+=($!)
for _ in $(seq 60); do cast block-number --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 1; done
[[ $(cast chain-id --rpc-url "$RPC") == 31337 ]] || fail "anvil not up"
FORK_BLOCK=$(cast block-number --rpc-url "$RPC")

echo "[2/6] deploy executor + allowlist Aave pool"
OUT=$(cd contracts && forge create src/ArbExecutor.sol:ArbExecutor --rpc-url "$RPC" --private-key "$OWNER_KEY" \
  --broadcast --constructor-args "$VAULT" "$WETH" "$RECEIVER" 2>&1) || fail "deploy: $(echo "$OUT" | tail -2)"
EXEC=$(echo "$OUT" | awk '/Deployed to:/ {print $3}')
send "$EXEC" 'setTarget(address,bool)' "$AAVE_POOL" true --private-key "$OWNER_KEY"

echo "[3/6] $N borrowers open Aave positions (1 WETH collateral each, USDC near the limit)"
BORROWERS=()
DEBT0=()
for i in $(seq 1 "$N"); do
  # Fresh keys: the well-known anvil dev addresses carry real history on a mainnet fork.
  KEY=$(cast keccak "mev-bot-crash-rehearsal-borrower-$i")
  ADDR=$(cast wallet address --private-key "$KEY")
  cast rpc anvil_setBalance "$ADDR" 0x56BC75E2D63100000 --rpc-url "$RPC" >/dev/null   # 100 ETH
  send "$WETH" 'deposit()' --value 1ether --private-key "$KEY"
  send "$WETH" 'approve(address,uint256)' "$AAVE_POOL" 1ether --private-key "$KEY"
  send "$AAVE_POOL" 'supply(address,uint256,address,uint16)' "$WETH" 1ether "$ADDR" 0 --private-key "$KEY"
  AVAIL=$(cast call "$AAVE_POOL" 'getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)' "$ADDR" --rpc-url "$RPC" | sed -n 3p | cut -d' ' -f1)
  BORROW=$(python3 -c "print(int($AVAIL) * 99 // 100 // 100)")
  send "$AAVE_POOL" 'borrow(address,uint256,uint256,uint16,address)' "$USDC" "$BORROW" 2 0 "$ADDR" --private-key "$KEY"
  BORROWERS+=("$ADDR")
  DEBT0+=("$(cast call "$AAVE_POOL" 'getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)' "$ADDR" --rpc-url "$RPC" | sed -n 2p | cut -d' ' -f1)")
done

echo "[4/6] start bot (live, batching on): Aave adapter only, scanning the fork's own blocks"
python3 - "$WORK/e2e.toml" "$FORK_BLOCK" "$RPC" <<'PY'
import sys, re
out, fork_block, rpc = sys.argv[1], int(sys.argv[2]), sys.argv[3]
src = open("config/arbitrum.toml").read()
src = src.replace("chain_id = 42161", "chain_id = 31337")
src = re.sub(r'(?m)^sequencer_rpc = .*$', '', src)
src = re.sub(r'(?m)^sequencer_feed = .*$', '', src)
src = re.sub(r'(?ms)^pairs = \[.*?\n\]\n', 'pairs = [["WETH", "USDC"]]\n', src)
for section in ("auto_discovery", "compound_v3", "morpho", "fluid", "uniswap_v4", "ticks", "atlas", "presim"):
    src = re.sub(rf'(?ms)^(\[{section}\]\n(?:#[^\n]*\n)*)enabled = true', r'\1enabled = false', src)
src = re.sub(r'(?m)^scan_rpc = .*$', f'scan_rpc = "{rpc}"', src)
src = re.sub(r'(?m)^sweep_rpc = .*$', f'sweep_rpc = "{rpc}"', src)
src = re.sub(r'(?m)^sweep_rpc_fallbacks = .*$', 'sweep_rpc_fallbacks = []', src)   # public mainnet RPCs would answer without the fork's state
src = re.sub(r'(?m)^sweep_secs = .*$', 'sweep_secs = 5', src)
src = re.sub(r'(?m)^fallback_poll_secs = .*$', 'fallback_poll_secs = 2', src)
src = re.sub(r'(?m)^from_block = 7700000.*$', f'from_block = {fork_block - 5}', src)
src = re.sub(r'(?ms)(\[aave\].*?^scan_step = )\d+', r'\g<1>1000', src)
keep = [b for b in re.split(r'(?m)^(?=\[\[dexes\]\])', src) if not b.startswith("[[dexes]]") or 'name = "uniswap_v3"' in b]
out_src = "".join(keep)
out_src = re.sub(r'(?ms)^(\[batch\]\n(?:#[^\n]*\n)*)enabled = false', r'\1enabled = true', out_src)
out_src = re.sub(r'(?m)^window_ms = \d+', 'window_ms = 300', out_src)
open(out, "w").write(out_src)
PY
! grep -q "sequencer_rpc" "$WORK/e2e.toml" || fail "sequencer broadcast must be removed"
# Run from a scratch dir: the bot loads ./.env, whose extra mainnet endpoints would leak into the fork test.
ROOT="$PWD"
(cd "$WORK" && exec env -i PATH="$PATH" HOME="$HOME" NO_COLOR=1 RUST_LOG=info \
  ARBITRUM_HTTP_URL="$RPC" ARBITRUM_WS_URL="ws://127.0.0.1:$PORT" \
  PRIVATE_KEY="$OWNER_KEY" EXECUTOR_ADDRESS="$EXEC" PROFIT_RECEIVER="$RECEIVER" \
  "$ROOT/target/release/mev-bot" --mode live --config "$WORK/e2e.toml" --data-dir "$WORK/data" > "$WORK/bot.log" 2>&1) &
pids+=($!)
for _ in $(seq 900); do grep -q 'sweep protocol="aave-v3" accounts=[1-9]' "$WORK/bot.log" && break; sleep 1; done
grep -q 'sweep protocol="aave-v3" accounts=[1-9]' "$WORK/bot.log" || fail "bot never indexed the borrowers"
grep -q "^enabled = true" <(sed -n '/^\[batch\]/,/^\[/p' "$WORK/e2e.toml") || fail "batching not enabled in the test config"

echo "[5/6] crash ETH/USD to 70% via a mock price source"
SRC=$(cast call "$AAVE_ORACLE" 'getSourceOfAsset(address)(address)' "$WETH" --rpc-url "$RPC")
PX=$(cast call "$AAVE_ORACLE" 'getAssetPrice(address)(uint256)' "$WETH" --rpc-url "$RPC" | cut -d' ' -f1)
CODE=$(cd contracts && forge inspect test/mocks/MockPriceSource.sol:MockPriceSource deployedBytecode)
cast rpc anvil_setCode "$SRC" "$CODE" --rpc-url "$RPC" >/dev/null
cast rpc anvil_setStorageAt "$SRC" 0x0 "$(cast to-uint256 $((PX * 70 / 100)))" --rpc-url "$RPC" >/dev/null
cast rpc evm_mine --rpc-url "$RPC" >/dev/null
# The bot reacts within seconds, so a position may already be repaid by the time it is checked here:
# each must be underwater now or have its debt already reduced.
for i in "${!BORROWERS[@]}"; do
  DATA=$(cast call "$AAVE_POOL" 'getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)' "${BORROWERS[$i]}" --rpc-url "$RPC")
  HF=$(echo "$DATA" | sed -n 6p | cut -d' ' -f1); D=$(echo "$DATA" | sed -n 2p | cut -d' ' -f1)
  python3 -c "import sys; sys.exit(0 if int('$HF') < 10**18 or int('$D') < int('${DEBT0[$i]}') * 0.9 else 1)" || fail "${BORROWERS[$i]} neither underwater nor repaid after the crash (hf=$HF debt=$D debt_before=${DEBT0[$i]})"
done
CRASH_AT=$(date +%s)

echo "[6/6] wait for the bot to liquidate all $N (up to 240 s)"
DONE=0
for _ in $(seq 240); do
  DONE=0
  for i in "${!BORROWERS[@]}"; do
    D=$(cast call "$AAVE_POOL" 'getUserAccountData(address)(uint256,uint256,uint256,uint256,uint256,uint256)' "${BORROWERS[$i]}" --rpc-url "$RPC" | sed -n 2p | cut -d' ' -f1)
    python3 -c "import sys; sys.exit(0 if int('$D') < int('${DEBT0[$i]}') * 0.9 else 1)" && DONE=$((DONE + 1))
  done
  [[ $DONE -eq $N ]] && break
  sleep 1
done
ELAPSED=$(( $(date +%s) - CRASH_AT ))
EV=$(cat "$WORK/data/arbitrum/events-"*.jsonl 2>/dev/null || true)
BATCHES=$(echo "$EV" | grep -c '"batch_sent"' || true)
SENT=$(echo "$EV" | grep -c '"kind":"sent"' || true)
[[ $DONE -eq $N ]] || fail "only $DONE of $N positions liquidated after ${ELAPSED}s ($(echo "$EV" | grep -o '"kind":"[a-z_]*"' | sort | uniq -c | sort -rn | head -8 | tr '\n' ' '))"
echo "E2E-CRASH PASS: $N of $N positions liquidated in ${ELAPSED}s; batch_sent events: $BATCHES, single sends: $SENT"
