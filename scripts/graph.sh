#!/usr/bin/env bash
# Keep the knowledge graph and its Obsidian projection current.
#   scripts/graph.sh update    AST re-extract changed code (no LLM cost)
#   scripts/graph.sh obsidian  project graph -> docs/knowledge/ (Obsidian notes)
#   scripts/graph.sh reflect   fold saved query outcomes into reflections/LESSONS.md
#   scripts/graph.sh optimize  update + reflect + obsidian (run after code changes)
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-optimize}" in
  update)   graphify update . ;;
  obsidian) node scripts/graphify-obsidian.mjs ;;
  reflect)  graphify reflect --graph graphify-out/graph.json ;;
  optimize) "$0" update && "$0" reflect && "$0" obsidian ;;
  *) echo "usage: $0 [update|obsidian|reflect|optimize]" >&2; exit 2 ;;
esac
