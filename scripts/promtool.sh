#!/usr/bin/env bash
# Run promtool from a pinned Prometheus image against the repo (read-only mount, no network needed after pull).
#   scripts/promtool.sh check rules infra/monitoring/copper-liq.rules.yml
set -euo pipefail
cd "$(dirname "$0")/.."
IMAGE="${PROMTOOL_IMAGE:-docker.io/prom/prometheus:v3.5.0}"
exec podman run --rm --network=none --userns=keep-id \
  -v "$PWD":/work:ro,Z -w /work \
  --entrypoint /bin/promtool "$IMAGE" "$@"
