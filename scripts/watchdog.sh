#!/usr/bin/env bash
# Health check for the bot, meant to run ON the production host (cron/systemd timer or by hand):
#   scripts/watchdog.sh             prints "ok" or one ALERT line per problem; exit 1 on any alert
# Alerts are appended to data/arbitrum/ALERTS.log. Optional push: put a webhook URL in
# data/arbitrum/alert_webhook (a file, never committed); each new alert is POSTed to it as JSON {"text": ...}.
# Checks: container up + healthy, heartbeat fresh, kill switch, hot wallet gas balance, cap/mode events.
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

today="$D/events-$(date -u +%F).jsonl"
if [[ -f $today ]]; then
  n="$(grep -cE '"kind":"(cap_reached|mode_switch)"' "$today" || true)"
  (( n == 0 )) || alerts+=("$n cap_reached/mode_switch event(s) today (bot may be in shadow)")
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
