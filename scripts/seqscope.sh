#!/usr/bin/env bash
# Run seqscope (built in the project container) with your files kept in data/seqscope/ (rules, database, recordings).
#
#   scripts/seqscope.sh serve                      the web interface at http://127.0.0.1:7878 (live feed)
#   scripts/seqscope.sh serve --replay rec.jsonl   the same from a recording, no network
#   scripts/seqscope.sh rules template             any seqscope command: run it with --help to see them all
#
# The first call builds it (a couple of minutes); later calls start at once. The container shares the host's network, so
# the interface is reachable from your browser at the printed address. Ctrl-C stops it cleanly.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=/workspace/target/release/seqscope
if [[ "${SEQSCOPE_REBUILD:-}" == 1 ]] || ! scripts/dev.sh test -x "$BIN"; then
  echo "building seqscope (first run, or SEQSCOPE_REBUILD=1)…" >&2
  scripts/dev.sh cargo build -q --release -p seqscope-cli
fi
mkdir -p data/seqscope
exec scripts/dev.sh bash -c 'cd /workspace/data/seqscope && exec "$0" "$@"' "$BIN" "$@"
