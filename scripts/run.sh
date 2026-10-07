#!/usr/bin/env bash
# Run the bot in podman.
#   scripts/run.sh build            build the image
#   scripts/run.sh shadow           start in shadow mode (simulate only, never sends)
#   scripts/run.sh live             start in live mode (sends trades) — asks for confirmation
#   scripts/run.sh status           container state, health, last summary line
#   scripts/run.sh logs             follow logs
#   scripts/run.sh halt             kill switch: stop sending new trades (process keeps running)
#   scripts/run.sh resume           remove the kill switch
#   scripts/run.sh stop             stop and remove the container
set -euo pipefail
cd "$(dirname "$0")/.."
export UID GID="$(id -g)"
CHAIN=arbitrum
STOP_FILE="data/$CHAIN/STOP"
# podman-compose explicitly: `podman compose` may delegate to docker-compose (and the Docker daemon).
compose() { podman-compose "$@"; }

preflight() {
  [[ -f .env ]] || { echo "missing .env (see docs/planning/README.md)"; exit 1; }
  for k in ARBITRUM_HTTP_URL ARBITRUM_WS_URL; do
    grep -q "^$k=." .env || { echo "$k not set in .env"; exit 1; }
  done
  mkdir -p "data/$CHAIN"
}

need_live_keys() {
  for k in PRIVATE_KEY EXECUTOR_ADDRESS; do
    grep -q "^$k=." .env || { echo "$k not set in .env — live mode needs it"; exit 1; }
  done
}

start() {
  preflight
  # NO_BUILD=1: the image was built elsewhere and loaded (small hosts can't compile the workspace).
  [[ -n ${NO_BUILD:-} ]] || compose build bot >/dev/null
  compose down >/dev/null 2>&1 || true
  EXECUTION_MODE="$1" compose up -d bot
  echo "started in $1 mode — 'scripts/run.sh logs' to follow, 'scripts/run.sh halt' to stop trading"
}

case "${1:-}" in
  build)  compose build bot ;;
  shadow) start shadow ;;
  live)
    need_live_keys
    read -r -p "Start LIVE trading with real funds? [y/N] " ok
    [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    rm -f "$STOP_FILE"
    start live ;;
  status)
    podman ps -a --filter name=mev-bot-arbitrum --format '{{.Names}}  {{.Status}}'
    podman logs --tail 200 mev-bot-arbitrum 2>&1 | grep -E "summary|landed|sent|ERROR|WARN" | tail -5 || true
    [[ -f $STOP_FILE ]] && echo "KILL SWITCH ACTIVE ($STOP_FILE)" || true ;;
  logs)   podman logs -f mev-bot-arbitrum ;;
  halt)   mkdir -p "data/$CHAIN" && touch "$STOP_FILE" && echo "kill switch on — no new trades will be sent" ;;
  resume) rm -f "$STOP_FILE" && echo "kill switch off" ;;
  stop)   compose down ;;
  *) sed -n '2,10p' "$0"; exit 2 ;;
esac
