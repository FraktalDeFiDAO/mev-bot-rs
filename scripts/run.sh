#!/usr/bin/env bash
# Run the bot in podman.
#   scripts/run.sh build            build the image
#   scripts/run.sh shadow           start in shadow mode (simulate only, never sends)
#   scripts/run.sh live             start in live mode (sends trades) — asks for confirmation
#   scripts/run.sh status           container state, health, last summary line
#   scripts/run.sh logs             follow logs
#   scripts/run.sh resources [N]    last N RAM/CPU samples (host + containers) from scripts/resources.sh
#   scripts/run.sh liq [WINDOW]     liquidation pipeline view: tiers, freshness, rechecks, lowest HFs
#   scripts/run.sh halt             kill switch: stop sending new trades (process keeps running)
#   scripts/run.sh resume           remove the kill switch
#   scripts/run.sh stop             stop and remove the container
#   scripts/run.sh advisor-start    build + start the Jev advisor (shadow; needs secrets/typesafe_api_key, mode 0600)
#   scripts/run.sh advisor-stop | advisor-status | advisor-logs
#   scripts/run.sh base-shadow      start the Base bot (shadow only, no .env) next to Arbitrum; public RPCs,
#                                   plus keyed BASE_* RPC URLs from secrets/base-rpc.env when present (mode 0600)
#   scripts/run.sh base-live        start the Base bot in live mode (sends trades) — asks for confirmation;
#                                   takes only PRIVATE_KEY, BASE_EXECUTOR_ADDRESS and PROFIT_RECEIVER from .env
#   scripts/run.sh base-halt | base-resume   Base kill switch (data/base/STOP)
#   scripts/run.sh base-stop | base-status | base-logs
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
ADVISOR_IMAGE=localhost/copper-jev-advisor:dev
ADVISOR_KEY=secrets/typesafe_api_key

advisor_start() {
  [[ -f $ADVISOR_KEY && ! -L $ADVISOR_KEY ]] || { echo "missing $ADVISOR_KEY (create: install -m 600 /dev/null $ADVISOR_KEY, then put the key in it)" >&2; exit 1; }
  [[ $(stat -c %a "$ADVISOR_KEY") == 600 ]] || { echo "$ADVISOR_KEY must be mode 0600 (chmod 600 $ADVISOR_KEY)" >&2; exit 1; }
  [[ -s $ADVISOR_KEY ]] || { echo "$ADVISOR_KEY is empty" >&2; exit 1; }
  [[ -n ${JEV_POLICY_VERSION:-} ]] || { echo "set JEV_POLICY_VERSION to the bot's liquidation export policy version" >&2; exit 1; }
  mkdir -p "data/$CHAIN"
  # Plain `podman run` with the same settings as compose.yaml's `advisor` service: podman-compose
  # 1.0.x (the server's) has no profiles and may recreate other services on `up`, and the bot
  # must never be touched. Build only when the image is missing (deploys stream it in).
  podman image exists "$ADVISOR_IMAGE" || compose --profile jev build advisor >/dev/null
  podman rm -f "$ADVISOR_CTR" >/dev/null 2>&1 || true
  podman run -d --name "$ADVISOR_CTR" \
    --userns keep-id --user "$(id -u):$(id -g)" \
    --read-only --tmpfs /tmp:rw,nosuid,nodev,size=16m \
    --security-opt no-new-privileges:true --cap-drop ALL --pids-limit 128 --memory 512m \
    --restart unless-stopped --stop-timeout 20 \
    --health-cmd "/usr/local/bin/copper-jev-advisor check" --health-interval 60s \
    --health-timeout 10s --health-retries 3 --health-start-period 30s \
    -e RUST_LOG="${RUST_LOG:-info}" -e JEV_MODE=shadow \
    -e JEV_MODEL="${JEV_MODEL:-jev-1.13.0}" -e JEV_POLICY_VERSION="$JEV_POLICY_VERSION" \
    -e JEV_MAX_INFLIGHT="${JEV_MAX_INFLIGHT:-4}" -e JEV_TIMEOUT_MS="${JEV_TIMEOUT_MS:-1500}" \
    -e JEV_BUDGET_PER_HOUR="${JEV_BUDGET_PER_HOUR:-200}" -e ADVISOR_SHUTDOWN_GRACE_SECONDS=10 \
    -e SNAPSHOT_DB="/var/lib/mev-bot/$CHAIN/liq-state.sqlite" \
    -e ADVISOR_DB="/var/lib/mev-bot/$CHAIN/advisor.sqlite" \
    -e TYPESAFE_API_KEY_FILE=/run/secrets/typesafe_api_key \
    -v "$PWD/data:/var/lib/mev-bot:Z" \
    -v "$PWD/$ADVISOR_KEY:/run/secrets/typesafe_api_key:ro,Z" \
    "$ADVISOR_IMAGE" >/dev/null
  echo "advisor started (shadow) — 'scripts/run.sh advisor-logs' to follow"
}

BASE_CTR=mev-bot-base
BOT_IMAGE=localhost/mev-bot:dev
BASE_RPC_ENV=secrets/base-rpc.env

# Base shadow (DECISIONS #175): public endpoints from config/base.toml, no signer; `base-live` adds
# only the signer's three variables (base_live_env). Keyed
# RPC URLs are optional and come from their own file, never .env (which holds the signing key): only
# BASE_{HTTP,WS,SWEEP,PRESIM}_URL* lines are allowed, so nothing else can reach the container.
base_rpc_env() {
  env_args=()
  [[ -f $BASE_RPC_ENV ]] || return 0
  [[ $(stat -c %a "$BASE_RPC_ENV") == 600 ]] || { echo "$BASE_RPC_ENV must be mode 0600" >&2; exit 1; }
  if grep -vE '^\s*(#.*)?$' "$BASE_RPC_ENV" | grep -qvE '^BASE_(HTTP_URL|WS_URL|WS_FALLBACK|SWEEP_URL|PRESIM_URL)(_[1-9])?='; then
    echo "$BASE_RPC_ENV: only BASE_HTTP_URL / BASE_WS_URL / BASE_WS_FALLBACK / BASE_SWEEP_URL / BASE_PRESIM_URL[_N] lines allowed" >&2
    exit 1
  fi
  env_args=(--env-file "$BASE_RPC_ENV")
}

BASE_STOP_FILE=data/base/STOP
BASE_LIVE_ENV=.env.base-live

# Live mode's signing env: only the three variables Base needs, never the rest of .env (Arbitrum's
# keyed URLs and settings stay out). Values are copied as data, not sourced.
base_live_env() {
  [[ -f .env ]] || { echo "missing .env" >&2; exit 1; }
  for k in PRIVATE_KEY BASE_EXECUTOR_ADDRESS PROFIT_RECEIVER; do
    grep -q "^$k=." .env || { echo "$k not set in .env — Base live mode needs it" >&2; exit 1; }
  done
  local tmp
  tmp=$(mktemp "$BASE_LIVE_ENV.XXXXXX")
  chmod 600 "$tmp"
  awk '/^(PRIVATE_KEY|BASE_EXECUTOR_ADDRESS|PROFIT_RECEIVER)=/ {print}' .env > "$tmp"
  mv "$tmp" "$BASE_LIVE_ENV"
  env_args+=(--env-file "$BASE_LIVE_ENV")
}

base_start() {
  local mode=${1:-shadow}
  mkdir -p data/base
  local env_args
  base_rpc_env
  if [[ $mode == live ]]; then base_live_env; fi
  podman image exists "$BOT_IMAGE" || compose build bot >/dev/null
  podman rm -f "$BASE_CTR" >/dev/null 2>&1 || true
  podman run -d --name "$BASE_CTR" --network host "${env_args[@]}" \
    --userns keep-id --user "$(id -u):$(id -g)" \
    --read-only --tmpfs /tmp:rw,nosuid,nodev,size=16m \
    --security-opt no-new-privileges:true --cap-drop ALL --pids-limit 128 --memory 2g \
    --restart always \
    --health-cmd 'test $(( $(date +%s) - $(cat /var/lib/mev-bot/base/heartbeat 2>/dev/null || echo 0) )) -lt 180' \
    --health-interval 60s --health-timeout 5s --health-retries 3 --health-start-period 120s \
    -e RUST_LOG="${RUST_LOG:-info}" -e NO_COLOR=1 \
    -v "$PWD/data:/var/lib/mev-bot:Z" -v "$PWD/config:/app/config:ro,Z" \
    "$BOT_IMAGE" --mode "$mode" --config /app/config/base.toml >/dev/null
  echo "Base bot started ($mode) — 'scripts/run.sh base-logs' to follow, 'scripts/run.sh base-halt' to stop trading"
}

case "${1:-}" in
  base-shadow)    base_start shadow ;;
  base-live)
    read -r -p "Start LIVE trading on Base with real funds? [y/N] " ok
    [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    # A STOP file means someone (or the watchdog) halted trading: don't clear it silently.
    if [[ -f $BASE_STOP_FILE ]]; then
      echo "kill switch is ON ($BASE_STOP_FILE, set $(date -r "$BASE_STOP_FILE" '+%F %T')). Run '$0 base-resume' first." >&2
      exit 1
    fi
    base_start live ;;
  base-halt)      mkdir -p data/base && touch "$BASE_STOP_FILE" && echo "Base kill switch on — no new trades will be sent" ;;
  base-resume)
    if [[ -f $BASE_STOP_FILE ]]; then
      read -r -p "Clear the Base kill switch and allow trading? [y/N] " ok
      [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    fi
    rm -f "$BASE_STOP_FILE" && echo "Base kill switch off" ;;
  base-stop)      podman stop -t 20 "$BASE_CTR" && podman rm "$BASE_CTR" ;;
  base-status)
    podman ps -a --filter name=$BASE_CTR --format '{{.Names}}  {{.Status}}'
    podman logs --tail 300 "$BASE_CTR" 2>&1 | grep -E "sweep protocol|summary|ERROR|WARN" | grep -v "WS connection" | tail -6 || true ;;
  base-logs)      podman logs -f "$BASE_CTR" ;;
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
  resources) scripts/resources.sh show "${2:-20}" ;;
  liq)    scripts/liq-view.sh "${2:-15m}" ;;
  halt)   mkdir -p "data/$CHAIN" && touch "$STOP_FILE" && echo "kill switch on — no new trades will be sent" ;;
  resume)
    if [[ -f $STOP_FILE ]]; then
      echo "kill switch set $(date -r "$STOP_FILE" '+%F %T'); daily caps are kept in data/$CHAIN/risk-state.json"
      read -r -p "Clear the kill switch and allow trading? [y/N] " ok
      [[ $ok == y || $ok == Y ]] || { echo "aborted"; exit 1; }
    fi
    rm -f "$STOP_FILE" && echo "kill switch off" ;;
  stop)   compose down ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
