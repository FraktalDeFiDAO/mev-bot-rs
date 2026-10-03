#!/usr/bin/env bash
# End-to-end test: the real mev-bot binary trades live against a local anvil fork of Arbitrum.
#
#   1. anvil forks Arbitrum with chain id 31337 (signed txs are invalid on mainnet) on :8547
#   2. deploy ArbExecutor to the fork with anvil's well-known dev key
#   3. start mev-bot in LIVE mode against the fork (no sequencer broadcast, no feed, no liquidation)
#   4. create a real price gap: dump WETH into Sushi WETH/USDC on the fork
#   5. assert the bot saw it, simulated it, sent a tx, it landed successfully, and profit arrived
#
# Needs: foundry (anvil/cast/forge), a release build, and a public Arbitrum RPC for fork state.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"
# The fork must serve historical ("archive") state: the pinned block ages within seconds.
# PublicNode refuses archive reads; Alchemy (incl. free tier) and arb1.arbitrum.io serve them.
if [[ -z ${E2E_FORK_URL:-} && -f .env ]]; then
  E2E_FORK_URL=$(grep -E '^ARBITRUM_HTTP_URL=' .env | head -1 | cut -d= -f2-)
fi
FORK_URL="${E2E_FORK_URL:-https://arb1.arbitrum.io/rpc}"
PORT=8547
RPC="http://127.0.0.1:$PORT"
WORK=$(mktemp -d)
# anvil dev account 0, derived from anvil's public test mnemonic (never hardcode keys), fork-only.
DEV_KEY=$(cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index 0)
DEV=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
RECEIVER=0x000000000000000000000000000000000000bEEF
WETH=0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
USDC=0xaf88d065e77c8cC2239327C5EDb3A432268e5831
SWAP_ROUTER02=0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45   # Uniswap V3 SwapRouter02 (Arbitrum)
VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$WORK"; }
trap cleanup EXIT
fail() { echo "E2E FAIL: $*"; echo "--- bot log tail:"; tail -30 "$WORK/bot.log" 2>/dev/null || true; exit 1; }

echo "[1/5] anvil fork (chain id 31337)"
anvil --fork-url "$FORK_URL" --chain-id 31337 --port "$PORT" --silent &
pids+=($!)
for _ in $(seq 60); do cast block-number --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 1; done
[[ $(cast chain-id --rpc-url "$RPC") == 31337 ]] || fail "anvil not up with chain id 31337"

echo "[2/5] deploy executor"
DEPLOY_OUT=$(cd contracts && forge create src/ArbExecutor.sol:ArbExecutor --rpc-url "$RPC" --private-key "$DEV_KEY" \
  --broadcast --constructor-args "$VAULT" "$WETH" "$RECEIVER" 2>&1) || fail "forge create: $(echo "$DEPLOY_OUT" | tail -3)"
EXEC=$(echo "$DEPLOY_OUT" | awk '/Deployed to:/ {print $3}')
[[ -n $EXEC ]] || fail "deploy failed"
echo "      executor $EXEC"

echo "[3/5] start bot (live) against the fork"
python3 - "$WORK/e2e.toml" <<'PY'
import sys, re
src = open("config/arbitrum.toml").read()
# Fork-only safety + speed: chain 31337, no sequencer broadcast/feed, no liquidation scans,
# no chain-wide auto-discovery, WETH/USDC on Uniswap V3 fee tiers only, low profit floor.
src = src.replace("chain_id = 42161", "chain_id = 31337")
src = re.sub(r'(?m)^sequencer_rpc = .*$', '', src)
src = re.sub(r'(?m)^sequencer_feed = .*$', '', src)
src = re.sub(r'(?ms)^pairs = \[.*?\n\]\n', 'pairs = [["WETH", "USDC"]]\n', src)
for section in ("auto_discovery", "liquidation", "aave", "compound_v3", "morpho", "fluid"):
    src = re.sub(rf'(?ms)^\[{section}\]\nenabled = true', f'[{section}]\nenabled = false', src)
src = src.replace("min_profit_usd = 0.05", "min_profit_usd = 0.01")
keep = []
for block in re.split(r'(?m)^(?=\[\[dexes\]\])', src):
    if block.startswith("[[dexes]]") and 'name = "uniswap_v3"' not in block:
        continue
    keep.append(block)
open(sys.argv[1], "w").write("".join(keep))
PY
grep -q "chain_id = 31337" "$WORK/e2e.toml" || fail "config transform"
! grep -q "sequencer_rpc" "$WORK/e2e.toml" || fail "sequencer broadcast must be removed for the fork"
env -i PATH="$PATH" HOME="$HOME" NO_COLOR=1 RUST_LOG=info \
  ARBITRUM_HTTP_URL="$RPC" ARBITRUM_WS_URL="ws://127.0.0.1:$PORT" \
  PRIVATE_KEY="$DEV_KEY" EXECUTOR_ADDRESS="$EXEC" PROFIT_RECEIVER="$RECEIVER" \
  ./target/release/mev-bot --mode live --config "$WORK/e2e.toml" --data-dir "$WORK/data" > "$WORK/bot.log" 2>&1 &
pids+=($!)
for _ in $(seq 180); do grep -q "subscribed to pool events" "$WORK/bot.log" && break; sleep 1; done
grep -q "subscribed to pool events" "$WORK/bot.log" || fail "bot did not start"

echo "[4/5] create a price gap: dump 300 WETH into Uniswap V3 WETH/USDC 0.05% (a watched pool)"
bal() { cast call "$1" 'balanceOf(address)(uint256)' "$RECEIVER" --rpc-url "$RPC" | cut -d' ' -f1; }
# Profit arrives in the loan token, which the bot picks (WETH or USDC here).
BEFORE_USDC=$(bal "$USDC"); BEFORE_WETH=$(bal "$WETH")
cast send "$WETH" 'deposit()' --value 300ether --private-key "$DEV_KEY" --rpc-url "$RPC" >/dev/null
cast send "$WETH" 'approve(address,uint256)' "$SWAP_ROUTER02" 300ether --private-key "$DEV_KEY" --rpc-url "$RPC" >/dev/null
cast send "$SWAP_ROUTER02" 'exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))' \
  "($WETH,$USDC,500,$DEV,300000000000000000000,0,0)" --private-key "$DEV_KEY" --rpc-url "$RPC" >/dev/null \
  || fail "dump swap failed"

echo "[5/5] wait for the bot to trade"
for _ in $(seq 90); do grep -q '"landed"' "$WORK/data/arbitrum/events-"*.jsonl 2>/dev/null && break; sleep 1; done
EV=$(cat "$WORK/data/arbitrum/events-"*.jsonl 2>/dev/null || true)
echo "$EV" | grep -q '"sim_ok"' || fail "no profitable simulation recorded"
echo "$EV" | grep -q '"sent"' || fail "bot did not send a transaction"
echo "$EV" | grep '"landed"' | grep -q '"success":true' || fail "tx did not land successfully"
AFTER_USDC=$(bal "$USDC"); AFTER_WETH=$(bal "$WETH")
GAIN=$(python3 -c "print(f'USDC +{($AFTER_USDC-$BEFORE_USDC)/1e6:.4f}, WETH +{($AFTER_WETH-$BEFORE_WETH)/1e18:.6f}')")
python3 -c "import sys; sys.exit(0 if $AFTER_USDC>$BEFORE_USDC or $AFTER_WETH>$BEFORE_WETH else 1)" || fail "receiver got no profit ($GAIN)"
grep -q "stale nonce; resyncing" "$WORK/bot.log" && echo "      (stale-nonce recovery exercised: setup txs used the bot's wallet)"
echo "E2E PASS: bot detected the gap, simulated, sent, landed; receiver $GAIN"
