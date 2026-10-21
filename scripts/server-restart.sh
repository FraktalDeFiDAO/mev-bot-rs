#!/usr/bin/env bash
# Server side of scripts/deploy.sh: save the old container log, rebuild, restart, and restart the Base shadow bot
# on the new image if it was running. deploy.sh starts this detached (setsid nohup) and polls its log, so a dropped
# SSH session cannot kill it halfway: on 2026-10-21 a drop during the ~15 min build left the bot stopped.
#   scripts/server-restart.sh shadow|live [no_build]     (prints "deploy-done rc=N" last)
set -uo pipefail
cd "$(dirname "$0")/.."
mode=$1 no_build=${2:-}
mkdir -p data/logs
podman image prune -f >/dev/null 2>&1 || true   # untagged images from earlier builds filled the disk (2026-10-07)
if podman container exists mev-bot-arbitrum; then
  f=data/logs/mev-bot-arbitrum-$(date -u +%Y%m%d-%H%M%S).log.gz
  podman logs mev-bot-arbitrum 2>&1 | gzip > "$f" && echo "   saved $f"
  ls -1t data/logs/mev-bot-arbitrum-*.log.gz | tail -n +21 | xargs -r rm -f
fi
base_running=$(podman container exists mev-bot-base && echo 1 || true)
echo y | NO_BUILD=$no_build scripts/run.sh "$mode" | tail -1
rc=${PIPESTATUS[1]}
if [[ $rc == 0 && -n $base_running ]]; then
  echo "   restarting the Base shadow bot on the new image"
  scripts/run.sh base-stop >/dev/null 2>&1 || true
  scripts/run.sh base-shadow | tail -1 || rc=$?
fi
podman image prune -f >/dev/null 2>&1 || true
echo "deploy-done rc=$rc"
exit "$rc"
