#!/usr/bin/env bash
# The gate every commit and CI run must pass: format, lint (warnings = errors), unit tests.
#   scripts/check.sh            Rust only (fast)
#   scripts/check.sh --all      + Foundry fork tests (needs network)
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"
cargo fmt --all --check
# Invariant 8: no stubs in production code (tests may use them).
if grep -rnE 'todo!\(|unimplemented!\(|panic!\("TODO' crates --include='*.rs' | grep -v '/tests/' | grep -v '^crates/[^:]*:[0-9]*:\s*//' | grep .; then
  echo "check: stubs found in production code (invariant 8)" >&2; exit 1
fi
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test -q --workspace
if command -v forge >/dev/null; then
  # The shadow virtual executor (mev-sim) must be the contract in contracts/src.
  if [[ "$(cd contracts && forge inspect ArbExecutor bytecode | tail -1)" != "$(tr -d '[:space:]' < crates/mev-sim/src/arb_executor.creation.hex)" ]]; then
    echo "check: crates/mev-sim/src/arb_executor.creation.hex is stale; regenerate it with" >&2
    echo "  (cd contracts && forge inspect ArbExecutor bytecode | tail -1) > crates/mev-sim/src/arb_executor.creation.hex" >&2
    exit 1
  fi
fi
if [[ ${1:-} == --all ]]; then
  (cd contracts && forge fmt --check && ARBITRUM_HTTP_URL="${ARBITRUM_FORK_URL:-https://arb1.arbitrum.io/rpc}" forge test -q)
fi
echo "check: OK"
