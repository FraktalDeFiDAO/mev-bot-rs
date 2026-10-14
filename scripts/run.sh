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
#   scripts/run.sh advisor-start    build + start the Jev advisor (shadow; needs secrets/typesafe_api_key, mode 0600)
#   scripts/run.sh advisor-stop | advisor-status | advisor-logs
set -euo pipefail
cd "$(dirname "$0")/.."
export UID
GID="$(id -g)"
export GID
export MEV_ENV_FILE="${MEV_ENV_FILE:-.env.shadow}"
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
  if [[ $1 == shadow ]]; then
    # Do not source .env: values are data, and the signing key must never enter this container.
    local shadow_tmp
    shadow_tmp=$(mktemp .env.shadow.XXXXXX)
    chmod 600 "$shadow_tmp"
    awk '/^(ARBITRUM_(HTTP_URL|WS_URL|WS_FALLBACK|SWEEP_URL|PRESIM_URL|FAST_FEED_KEY)(_[0-9]+)?|EXECUTOR_ADDRESS|PROFIT_RECEIVER)=/ {print}' .env > "$shadow_tmp"
    mv "$shadow_tmp" .env.shadow
    MEV_ENV_FILE=.env.shadow
  else
    MEV_ENV_FILE=.env
  fi
  # NO_BUILD=1: the image was built elsewhere and loaded (small hosts can't compile the workspace).
  [[ -n ${NO_BUILD:-} ]] || compose build bot >/dev/null
  compose down >/dev/null 2>&1 || true
  EXECUTION_MODE="$1" compose up -d bot
  echo "started in $1 mode — 'scripts/run.sh logs' to follow, 'scripts/run.sh halt' to stop trading"
}

ADVISOR_CTR=mev-jev-advisor
ADVISOR_KEY=secrets/typesafe_api_key

advisor_start() {
  [[ -f $ADVISOR_KEY && ! -L $ADVISOR_KEY ]] || { echo "missing $ADVISOR_KEY (create: install -m 600 /dev/null $ADVISOR_KEY, then put the key in it)" >&2; exit 1; }
  [[ $(stat -c %a "$ADVISOR_KEY") == 600 ]] || { echo "$ADVISOR_KEY must be mode 0600 (chmod 600 $ADVISOR_KEY)" >&2; exit 1; }
  [[ -s $ADVISOR_KEY ]] || { echo "$ADVISOR_KEY is empty" >&2; exit 1; }
  [[ -n ${JEV_POLICY_VERSION:-} ]] || { echo "set JEV_POLICY_VERSION to the bot's liquidation export policy version" >&2; exit 1; }
  mkdir -p "data/$CHAIN"
  [[ -n ${NO_BUILD:-} ]] || compose --profile jev build advisor >/dev/null
  # Only the advisor service: the bot is never touched.
  compose --profile jev up -d advisor
  echo "advisor started (shadow) — 'scripts/run.sh advisor-logs' to follow"
}

case "${1:-}" in
  advisor-start)  advisor_start ;;
  advisor-stop)   podman stop -t 20 "$ADVISOR_CTR" && podman rm "$ADVISOR_CTR" ;;
  advisor-status)
    podman ps -a --filter name=$ADVISOR_CTR --format '{{.Names}}  {{.Status}}'
    podman logs --tail 200 "$ADVISOR_CTR" 2>&1 | grep -E "metrics|started|ERROR|WARN" | tail -3 || true ;;
  advisor-logs)   podman logs -f "$ADVISOR_CTR" ;;
  build)  MEV_ENV_FILE=.env compose build bot ;;
  shadow) start shadow ;;
  live)
    need_live_keys
    read -r -p "Start LIVE trading with real funds? [y/N] " ok
    [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    # A STOP file means someone (or the watchdog) halted trading: don't clear it silently.
    if [[ -f $STOP_FILE ]]; then
      echo "kill switch is ON ($STOP_FILE, set $(date -r "$STOP_FILE" '+%F %T')). Run '$0 resume' first." >&2
      exit 1
    fi
    start live ;;
  status)
    podman ps -a --filter name=mev-bot-arbitrum --format '{{.Names}}  {{.Status}}'
    podman logs --tail 200 mev-bot-arbitrum 2>&1 | grep -E "summary|landed|sent|ERROR|WARN" | tail -5 || true
    [[ -f $STOP_FILE ]] && echo "KILL SWITCH ACTIVE ($STOP_FILE)" || true ;;
  logs)   podman logs -f mev-bot-arbitrum ;;
  halt)   mkdir -p "data/$CHAIN" && touch "$STOP_FILE" && echo "kill switch on — no new trades will be sent" ;;
  resume)
    if [[ -f $STOP_FILE ]]; then
      echo "kill switch set $(date -r "$STOP_FILE" '+%F %T'); daily caps are kept in data/$CHAIN/risk-state.json"
      read -r -p "Clear the kill switch and allow trading? [y/N] " ok
      [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    fi
    rm -f "$STOP_FILE" && echo "kill switch off" ;;
  stop)   compose down ;;
  *) sed -n '2,12p' "$0"; exit 2 ;;
esac
