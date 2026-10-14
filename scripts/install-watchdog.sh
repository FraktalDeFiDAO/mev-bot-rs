#!/usr/bin/env bash
# Install the watchdog as a systemd USER timer on the production host (run there, once):
#   scripts/install-watchdog.sh
# Installs the watchdog (every 3 min) and the RAM/CPU sampler scripts/resources.sh (every minute).
# Alerts go to data/arbitrum/ALERTS.log (and the webhook file, if present). Idempotent.
set -euo pipefail
cd "$(dirname "$0")/.."
U="$HOME/.config/systemd/user"
mkdir -p "$U"
# The units name %h/src/mev-bot; point them at this checkout wherever it lives (e.g. ~/src/mev-bot-v3).
repo="$(pwd)"
for f in mev-watchdog.service mev-watchdog.timer mev-resources.service mev-resources.timer; do
  sed "s#%h/src/mev-bot#$repo#g" "infra/watchdog/$f" > "$U/$f"
done
systemctl --user daemon-reload
systemctl --user enable --now mev-watchdog.timer mev-resources.timer
loginctl enable-linger "$USER" 2>/dev/null || echo "note: run 'sudo loginctl enable-linger $USER' so the timer survives logout"
systemctl --user list-timers mev-watchdog.timer mev-resources.timer --no-pager
