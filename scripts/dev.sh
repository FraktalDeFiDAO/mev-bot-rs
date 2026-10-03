#!/usr/bin/env bash
# Run a dev command inside the toolchain container (Rust + Foundry), repo bind-mounted.
#   scripts/dev.sh cargo test -q -p mev-dex
#   scripts/dev.sh forge test            (run from contracts via: scripts/dev.sh bash -c 'cd contracts && forge test')
#   scripts/dev.sh                       (interactive shell)
set -euo pipefail
cd "$(dirname "$0")/.."
IMAGE=localhost/mev-bot-dev:latest
if ! podman image exists "$IMAGE"; then
  podman build -t "$IMAGE" -f Containerfile.dev --build-arg APP_UID="$(id -u)" --build-arg APP_GID="$(id -g)" .
fi
tty=(); [[ -t 0 ]] && tty=(-it)
exec podman run --rm "${tty[@]}" --userns=keep-id --network=host \
  -v "$PWD":/workspace:Z \
  -v mev-bot-cargo-registry:/home/dev/.cargo/registry \
  -v mev-bot-target:/workspace/target \
  -e CARGO_TARGET_DIR=/workspace/target \
  --env-file <(grep -E '^ARBITRUM_(HTTP|WS)_URL=' .env 2>/dev/null || true) \
  -w /workspace "$IMAGE" "${@:-bash}"
