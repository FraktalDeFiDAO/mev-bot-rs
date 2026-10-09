#!/usr/bin/env bash
# The seqscope web interface, tested in a real browser. No network and no input needed.
#
#   scripts/test-seqscope-ui.sh            logic tests (node) + browser tests (headless Chromium) against a server that
#                                          replays the recorded corpus; screenshots land in $SHOTS (default /tmp/seqscope-ui/shots)
#
# Needs on the host: node >= 22 and chromium. The server and its build run in the project container (Podman), like every
# other build here. The server is started with the interface files built into the binary (what users get), is stopped
# with SIGTERM at the end (which also checks it shuts down cleanly), and any rules the tests save are restored.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${SEQSCOPE_PORT:-7899}"
export SEQSCOPE_URL="http://127.0.0.1:$PORT"
export SHOTS="${SHOTS:-/tmp/seqscope-ui/shots}"
command -v node >/dev/null || { echo "node is needed for the interface tests"; exit 1; }
command -v "${CHROMIUM:-chromium}" >/dev/null || { echo "chromium is needed for the interface tests (or set CHROMIUM)"; exit 1; }

echo "== interface logic (node)"
node --test crates/seqscope-server/ui/test/logic.test.js 2>&1 | grep -E "^(✖|ℹ (tests|pass|fail))" || true
node --test crates/seqscope-server/ui/test/logic.test.js >/dev/null 2>&1 || { echo "LOGIC TESTS FAILED"; exit 1; }

echo "== build"
scripts/dev.sh cargo build -q --release -p seqscope-cli

START="$(mktemp -p . .seqscope-ui-start.XXXXXX)"
LOG="$(mktemp)"
SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then kill -TERM "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; fi
  rm -f "$START"
}
trap cleanup EXIT
cat > "$START" <<SCRIPT
set -e
mkdir -p /tmp/ui && cd /tmp/ui
S=/workspace/target/release/seqscope
\$S rules template dex-swaps > seqscope.toml
printf '\n[[profile]]\nname = "gaps"\nmode = "first"\n\n[[profile.rule]]\nname = "identified but unread"\nwhen = "kind == undecoded_known"\n\n[[profile.rule]]\nname = "unidentified"\nwhen = "kind == unknown && len >= 4"\nsample = 4\n' >> seqscope.toml
exec \$S serve --replay /workspace/crates/seqscope-core/tests/fixtures/corpus.jsonl --speed 40 --rules seqscope.toml --db ui.db --bind 127.0.0.1:$PORT
SCRIPT

echo "== server (replaying the corpus) on :$PORT"
scripts/dev.sh bash "/workspace/$(basename "$START")" > "$LOG" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 120); do curl -sf "$SEQSCOPE_URL/api/health" >/dev/null && break; sleep 0.5; done
curl -sf "$SEQSCOPE_URL/api/health" >/dev/null || { echo "the server did not start:"; cat "$LOG"; exit 1; }
sleep 3   # let the replay put some rows in the database

echo "== interface in a real browser"
RC=0
node --test crates/seqscope-server/ui/test/browser.test.js 2>&1 | grep -E "^(✔|✖)|ℹ (tests|pass|fail)|actual:|Error|timed out" || true
node --test crates/seqscope-server/ui/test/browser.test.js >/dev/null 2>&1 || RC=1

echo "== stop (SIGTERM): the server must flush and exit cleanly"
kill -TERM "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""
grep -E "^stored [0-9]+ rows \(0 dropped, 0 failed\)" "$LOG" || { echo "the server did not report a clean stop:"; tail -5 "$LOG"; RC=1; }

if [[ $RC == 0 ]]; then echo "INTERFACE SUITE: OK (screenshots in $SHOTS)"; else echo "INTERFACE SUITE: FAILED"; fi
exit $RC
