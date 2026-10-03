#!/usr/bin/env bash
# Live read-only integration tests: adapters decode real Arbitrum contracts (catches ABI drift).
#   MEV_IT_RPC      eth_call RPC (default: PublicNode)
#   MEV_IT_LOG_RPC  getLogs RPC   (default: arb1.arbitrum.io)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test -q -p mev-lending-aave-v3 -p mev-lending-compound-v3 -p mev-lending-morpho -p mev-lending-fluid \
  --test live -- --ignored --test-threads=1
