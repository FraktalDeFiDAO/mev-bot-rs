#!/usr/bin/env bash
# Install the watchdog as a systemd USER timer on the production host (run there, once):
#   scripts/install-watchdog.sh
# Alerts go to data/arbitrum/ALERTS.log (and the webhook file, if present). Idempotent.
set -euo pipefail
cd "$(dirname "$0")/.."
U="$HOME/.config/systemd/user"
mkdir -p "$U"
cp infra/watchdog/mev-watchdog.service infra/watchdog/mev-watchdog.timer "$U/"
systemctl --user daemon-reload
systemctl --user enable --now mev-watchdog.timer
loginctl enable-linger "$USER" 2>/dev/null || echo "note: run 'sudo loginctl enable-linger $USER' so the timer survives logout"
systemctl --user list-timers mev-watchdog.timer --no-pager
