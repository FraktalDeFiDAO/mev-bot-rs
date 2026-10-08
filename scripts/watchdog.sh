#!/usr/bin/env bash
# Health check for the bot, meant to run ON the production host (cron/systemd timer or by hand):
#   scripts/watchdog.sh             prints "ok" or one ALERT line per problem; exit 1 on any alert
# Alerts are appended to data/arbitrum/ALERTS.log. Optional push: put a webhook URL in
# data/arbitrum/alert_webhook (a file, never committed); each new alert is POSTed to it as JSON {"text": ...}.
# Checks: container up + healthy, heartbeat fresh, kill switch, hot wallet gas balance, disk, cap/mode events; with FF_VAULT
# set, also the Fast Feed ticket vault (ticket bought for the next round, funds, renewals, keeper gas).
set -uo pipefail
cd "$(dirname "$0")/.."
D=data/arbitrum
NAME=mev-bot-arbitrum
MIN_ETH="${MIN_ETH:-0.003}"        # alert below this hot-wallet balance
MAX_HEARTBEAT_AGE="${MAX_HEARTBEAT_AGE:-180}"
RPC="${WATCHDOG_RPC:-https://arb1.arbitrum.io/rpc}"
alerts=()

status="$(podman ps -a --filter "name=$NAME" --format '{{.Status}}' 2>/dev/null | head -1)"
case "$status" in
  Up*healthy*) ;;
  "") alerts+=("container $NAME not found") ;;
  *) alerts+=("container not healthy: $status") ;;
esac

if [[ -f $D/heartbeat ]]; then
  age=$(( $(date +%s) - $(stat -c %Y "$D/heartbeat") ))
  (( age <= MAX_HEARTBEAT_AGE )) || alerts+=("heartbeat stale: ${age}s")
else
  alerts+=("no heartbeat file")
fi

[[ -f $D/STOP ]] && alerts+=("kill switch is ON ($D/STOP): not sending")

addr="$(podman logs "$NAME" 2>&1 | grep -oE 'hot wallet address=0x[0-9a-fA-F]{40}' | tail -1 | cut -d= -f2)"
if [[ -n $addr ]]; then
  hex="$(curl -s -m 10 -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBalance\",\"params\":[\"$addr\",\"latest\"]}" "$RPC" \
    | jq -r '.result // empty')"
  if [[ -n $hex ]]; then
    low="$(python3 -c "print(1 if int('$hex',16)/1e18 < $MIN_ETH else 0)")"
    eth="$(python3 -c "print('%.5f' % (int('$hex',16)/1e18))")"
    [[ $low == 1 ]] && alerts+=("hot wallet low: $eth ETH (< $MIN_ETH) $addr")
  else
    alerts+=("could not read hot wallet balance")
  fi
fi

disk="$(df --output=pcent / | tail -1 | tr -dc 0-9)"
(( disk < ${MAX_DISK_PCT:-85} )) || alerts+=("disk ${disk}% full (builds fail near 100%; podman image prune -f)")

# Fast Feed ticket vault (optional): set FF_VAULT (and FF_KEEPER_ADDRESS for the gas check). Losing the seat is expensive
# (nobody has ever come back after skipping a round), so alert early.
if [[ -n "${FF_VAULT:-}" ]]; then
  while IFS= read -r a; do [[ -n "$a" ]] && alerts+=("$a"); done < <(python3 - "$RPC" "$FF_VAULT" "${FF_KEEPER_ADDRESS:-}" <<'PY'
import json, sys, time, urllib.request
rpc, vault, keeper = sys.argv[1], sys.argv[2], sys.argv[3]
def call(to, data):
    r = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{"to": to, "data": data}, "latest"]}).encode(), {"content-type": "application/json", "user-agent": "watchdog"})
    return json.load(urllib.request.urlopen(r, timeout=20))["result"]
def words(h): h = h[2:]; return [int(h[i:i + 64], 16) for i in range(0, len(h), 64)]
try:
    rnd, price, held, gf, usdg, inside, left, budget, can = words(call(vault, "0x200d2ed2"))
    per_round = words(call(vault, "0x6e7687c2"))[0]
    end = words(call("0x451DD62C26b753d07d6104aaC6865AaE73a6A598", "0xc02e580e"))[0]
    hours = (end - time.time()) / 3600
    if per_round and held < per_round and hours < 2:
        print(f"Fast Feed vault: only {held} of {per_round} ticket(s) bought for the next round and {hours:.1f} h left in round {rnd}")
    if per_round and usdg < price * max(per_round - held, 1) and held < per_round:
        print(f"Fast Feed vault: {usdg/1e6:.2f} USDG cannot cover the next ticket ({price/1e6:.2f} USDG)")
    if left <= 1:
        print(f"Fast Feed vault: {left} renewal(s) left")
    if budget < price:
        print(f"Fast Feed vault: budget {budget/1e6:.2f} USDG is below one ticket")
    if keeper:
        r = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_getBalance", "params": [keeper, "latest"]}).encode(), {"content-type": "application/json", "user-agent": "watchdog"})
        eth = int(json.load(urllib.request.urlopen(r, timeout=20))["result"], 16) / 1e18
        if eth < 0.0005:
            print(f"Fast Feed keeper gas low: {eth:.5f} ETH")
except Exception as e:
    print(f"Fast Feed vault check failed: {e}")
PY
)
fi

today="$D/events-$(date -u +%F).jsonl"
if [[ -f $today ]]; then
  # Only events from the last hour: a cap hit earlier in the day (the arb strategy runs again
  # after midnight UTC, or after an operator restart) is not a current problem.
  since="$(date -u -d '1 hour ago' +%FT%T)"
  n="$(grep -E '"kind":"(cap_reached|mode_switch)"' "$today" | python3 -c "
import sys, json
since = '$since'
print(sum(1 for l in sys.stdin if json.loads(l).get('ts', '')[:19] >= since))" 2>/dev/null || echo 0)"
  (( n == 0 )) || alerts+=("$n cap_reached/mode_switch event(s) in the last hour (bot may be in shadow)")
fi

if (( ${#alerts[@]} == 0 )); then
  echo "ok ${eth:+(wallet $eth ETH)}"
  exit 0
fi
ts="$(date -u +%FT%TZ)"
for a in "${alerts[@]}"; do
  echo "ALERT $a"
  echo "$ts $a" >> "$D/ALERTS.log"
  [[ -s $D/alert_webhook ]] && curl -s -m 10 -H 'content-type: application/json' \
    -d "$(jq -n --arg t "mev-bot: $a" '{text:$t}')" "$(cat "$D/alert_webhook")" >/dev/null || true
done
exit 1
