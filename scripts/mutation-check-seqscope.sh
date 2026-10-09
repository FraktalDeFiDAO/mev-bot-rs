#!/usr/bin/env bash
# Does the test suite notice when the code is wrong? Plants known bugs one at a time, runs the relevant tests, restores
# the file, and fails if any bug goes unnoticed. Run after changing the tests or the code they guard (a few minutes).
#
#   scripts/mutation-check-seqscope.sh
set -uo pipefail
cd "$(dirname "$0")/.."
missed=0
try() { # name file sed-expr test-command (run in the container)
  mutate "$1" "$2" "$3" "scripts/dev.sh bash -c '$4'"
}
try_host() { # name file sed-expr test-command (run on the host: the interface tests need node and chromium)
  mutate "$1" "$2" "$3" "$4"
}
mutate() { # name file sed-expr full-command
  local name="$1" file="$2" expr="$3" cmd="$4"
  cp "$file" /tmp/mut.bak
  sed -i "$expr" "$file"
  if cmp -s "$file" /tmp/mut.bak; then echo "STALE  [$name]: the mutation no longer applies; update this script"; missed=1; return; fi
  local out; out=$(bash -c "$cmd" 2>&1 | grep -E "FAILED|panicked|✖" | head -1 | cut -c1-120)
  cp /tmp/mut.bak "$file"
  if [[ -n "$out" ]]; then echo "caught [$name]"; else echo "MISSED [$name]"; missed=1; fi
}
try "wrappers disabled"              crates/mev-decode/src/calls.rs        's/const MAX_DEPTH: usize = 6;/const MAX_DEPTH: usize = 0;/'                                              'cargo test -q -p seqscope-core --test corpus 2>&1'
try "V4 amount from the wrong word"  crates/mev-protocol-uniswap-router/src/lib.rs 's/160 => s + 96,/160 => s + 128,/'                                                         'cargo test -q -p mev-protocol-uniswap-router 2>&1'
try "sampling ignored"               crates/seqscope-core/src/engine.rs    's/if r.sample > 1$/if false \&\& r.sample > 1/'                                                          'cargo test -q -p seqscope-core 2>&1'
try "drop action stores"             crates/seqscope-core/src/engine.rs    's/ActionKind::Drop => break,/ActionKind::Drop => rows.push(make_row(\&ctx, \&p.name, r)),/'            'cargo test -q -p seqscope-core 2>&1'
try "first mode never stops"         crates/seqscope-core/src/engine.rs    's/if p.mode == Mode::First {/if p.mode == Mode::Never {/;s/pub enum Mode {/pub enum Mode {\n    Never,/'     'cargo test -q -p seqscope-core 2>&1'
try "missing-field != matches"       crates/seqscope-core/src/compile.rs   's/Node::Cmp(f, op, l) => f.read(c).is_some_and(|v| compare(v, \*op, l)),/Node::Cmp(f, op, l) => f.read(c).map_or(*op == Op::Ne, |v| compare(v, *op, l)),/' 'cargo test -q -p seqscope-core 2>&1'
try "store drops without counting"   crates/seqscope-store/src/lib.rs      's/self.stats.dropped.fetch_add(1, Ordering::Relaxed);//'                                                 'cargo test -q -p seqscope-store 2>&1'
try "CLI exits 0 on a bad rules file" crates/seqscope-cli/src/main.rs      's/bail!("{} rule(s) have problems", errs.len())/return Ok(())/'                                         'cargo test -q -p seqscope-cli 2>&1'
try "rule file ignores unknown keys" crates/seqscope-core/src/engine.rs    's/#\[serde(deny_unknown_fields)\]//'                                                                     'cargo test -q -p seqscope-core 2>&1'
try "SIGTERM ignored by the collector" crates/seqscope-cli/src/main.rs 's/_ = term.recv() => {}/_ = std::future::pending::<()>() => {}/' 'cargo test -q -p seqscope-cli --test e2e collector 2>&1'
try "server answers foreign hosts"     crates/seqscope-server/src/lib.rs     's/if !st.allow_remote \&\& !is_loopback_host(host) {/if false {/'                                'cargo test -q -p seqscope-server --test api 2>&1'
try "server accepts unmarked changes"  crates/seqscope-server/src/lib.rs     's/h.get("x-seqscope").is_none()/false/'                                                                  'cargo test -q -p seqscope-server --test api 2>&1'
try "bad rules are saved anyway"       crates/seqscope-server/src/lib.rs     's/let (_, engine) = validate(text)?;/let engine = Engine::compile(\&RuleSet::default()).unwrap();/'    'cargo test -q -p seqscope-server --test api 2>&1'
try_host "UI joins conditions with OR"  crates/seqscope-server/ui/app.js     "s/const sep = joiner === 'or' ? ' || ' : ' \&\& ';/const sep = ' || ';/"                                  'node --test crates/seqscope-server/ui/test/logic.test.js 2>&1'
try_host "UI never shows the status"    crates/seqscope-server/ui/app.js     's/      renderStatus();/      \/\/ renderStatus();/'                                                         'scripts/test-seqscope-ui.sh 2>&1'
if (( missed )); then echo "MUTATION CHECK: FAILED"; exit 1; fi
echo "MUTATION CHECK: every planted bug was caught"
