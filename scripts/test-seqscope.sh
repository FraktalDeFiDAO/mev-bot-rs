#!/usr/bin/env bash
# The seqscope test suite, every layer, in the project container (no host toolchain, no keys, no input needed).
#
#   scripts/test-seqscope.sh            offline: format, lints, unit + property + corpus + CLI end-to-end tests,
#                                       and a performance guard (rule engine under 20 us/tx with 100 rules)
#   scripts/test-seqscope.sh --live     the above, then a smoke test against the live sequencer feed: record 30 s, run
#                                       every template over the recording, run the collector 20 s and check its
#                                       invariants (nothing dropped or failed, every row pending, counts add up)
#   scripts/test-seqscope.sh --ui       also the web interface in a real browser (scripts/test-seqscope-ui.sh: needs node and chromium)
#   scripts/test-seqscope.sh --all      everything
#   UPDATE_GOLDEN=1 scripts/test-seqscope.sh    regenerate crates/seqscope-core/tests/fixtures/corpus_golden.json after a
#                                       deliberate decoder change (review the diff before committing)
#
# Layers (what each one protects):
#   mev-decode unit + calls tests ... wrapper unwrapping (Multicall3, multicalls, accounts, ERC-4337 v0.6/v0.7)
#   seqscope-core unit + engine ...... rule language, compiler errors, engine semantics, file format
#   seqscope-core corpus ............. 2,049 real feed transactions: invariants, accuracy against the chain, golden
#                                      summary, mutation fuzzing, engine properties
#   seqscope-store ................... round trip, filters, read-only SQL, backpressure accounting, persistence, pruning
#   seqscope-cli e2e ................. the real binary: every offline command, exit codes, deterministic output
#   perf guard ....................... the 20 us/tx target of the spec
set -euo pipefail
cd "$(dirname "$0")/.."
LIVE=0; UI=0
for a in "$@"; do case "$a" in --live) LIVE=1 ;; --ui) UI=1 ;; --all) LIVE=1; UI=1 ;; esac; done

scripts/dev.sh bash -c '
set -euo pipefail
export UPDATE_GOLDEN="'"${UPDATE_GOLDEN:-}"'"
[ -z "$UPDATE_GOLDEN" ] && unset UPDATE_GOLDEN
step() { printf "\n== %s\n" "$*"; }

step "format and lints"
cargo fmt --all --check
cargo clippy -q --message-format=short -p mev-decode -p seqscope-core -p seqscope-store -p seqscope-server -p seqscope-cli --all-targets -- -D warnings

step "tests"
cargo test -q -p mev-decode -p seqscope-core -p seqscope-store -p seqscope-server -p seqscope-cli

step "performance guard (release): rule engine, 100 rules, decoding included"
OUT=$(cargo run -q --release -p seqscope-core --example bench 2>&1)
echo "$OUT" | grep "with 100 rules"
echo "$OUT" | grep "with 100 rules" | awk "{ if (\$(NF-6)+0 > 20.0) { print \"TOO SLOW: \" \$0; bad=1 } } END { exit bad }"

step "load guard (release): 10,000 tx/s for 5 s must drop nothing"
LOAD=$(cargo run -q --release -p seqscope-store --example load 5 10000 2>&1)
echo "$LOAD"
echo "$LOAD" | grep -q "dropped 0  failed 0" || { echo "LOAD GUARD FAILED"; exit 1; }
echo "$LOAD" | grep -q "accounted:.*(ok)" || { echo "ROWS NOT ACCOUNTED FOR"; exit 1; }

echo; echo "OFFLINE SUITE: OK"
if [ "'"$LIVE"'" = 1 ]; then
  step "live smoke (needs network): record, templates, collector"
  cargo build -q --release -p seqscope-cli
  S=/workspace/target/release/seqscope
  mkdir -p /tmp/live && cd /tmp/live && rm -f *.db* *.jsonl *.toml
  $S record -o rec.jsonl --secs 30 2>&1 | tail -1
  N=$(wc -l < rec.jsonl); [ "$N" -gt 100 ] || { echo "recorded only $N transactions"; exit 1; }
  for t in $($S rules template | awk "NF>1 && \$1 !~ /^seqscope/ {print \$1}" | head -8); do
    $S rules template "$t" > "$t.toml"
    $S rules check "$t.toml" > /dev/null
    $S replay rec.jsonl -r "$t.toml" --db "$t.db" 2>&1 | tail -1 | sed "s/^/  $t: /"
  done
  # Invariants over every database the templates produced.
  for db in *.db; do
    BAD=$($S sql --db "$db" "SELECT count(*) FROM rows WHERE status != '"'"'pending'"'"' OR kind NOT IN ('"'"'swap'"'"','"'"'liquidity'"'"','"'"'liquidation'"'"','"'"'undecoded_known'"'"','"'"'unknown'"'"')" --json | python3 -c "import sys,json; print(json.load(sys.stdin)[\"rows\"][0][0])")
    [ "$BAD" = 0 ] || { echo "$db: $BAD malformed rows"; exit 1; }
  done
  echo "  database invariants hold"
  $S rules test -f rec.jsonl -w "kind != unknown" --samples 1 | head -2
  echo "  collector 20 s:"
  cp dex-swaps.toml seqscope.toml
  timeout 25 $S run --every 10 2>&1 | tee run.log | tail -4 || true
  grep -q "dropped 0  failed 0" run.log || { echo "COLLECTOR DROPPED OR FAILED ROWS"; exit 1; }
  grep -q "stored" run.log || { echo "collector did not shut down cleanly"; exit 1; }
  echo "LIVE SMOKE: OK"
fi
'

if [[ "$UI" == 1 ]]; then
  echo; echo "== web interface"
  scripts/test-seqscope-ui.sh
fi
