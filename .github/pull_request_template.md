## What & why

<!-- One or two sentences. Link the issue: Closes #123 -->

## Crates / areas touched

- [ ] `contracts/` (executor — needs fork tests + security review)
- [ ] `mev-exec` / `mev-risk` (moves funds — needs extra review)
- [ ] Other crates:
- [ ] Docs / planning only

## Verification

- [ ] `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -q -p <crate>` for each touched crate
- [ ] `forge test` (if contracts changed)
- [ ] Shadow-mode or fork run (if strategy/exec logic changed): result →

## Risk

- [ ] No secrets, private keys, or RPC API keys in the diff
- [ ] Gas/loss limits unchanged, or change explained here
- [ ] `scripts/graph.sh optimize` run; `.ai/STATE.md` / `DECISIONS.md` updated if relevant
