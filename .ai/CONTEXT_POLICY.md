# Context policy

Canonical retrieval policy for every AI client in this repo. `CLAUDE.md` is a thin adapter
pointing here. Adapted from `~/projects/AI_Chatbot/.ai/CONTEXT_POLICY.md`.

## The rule

Send the model the smallest context that answers the question. Every file read is re-sent on
every later turn, so the real cost of a read is its size × the turns that follow.

## Retrieval ladder

Stop at the first tier that resolves the question. Escalate one tier at a time.

| Tier | Action | Cost |
|---|---|---|
| 0 | Already in context, or a prior result | free |
| 1 | `graphify query "<q>" --budget N` | ~0.5–4k |
| 2 | `graphify explain "<node>"` / `path "A" "B"` / `affected "X"` | ~0.5k |
| 3 | `docs/knowledge/*.md` community note or `Files.md`; `docs/planning/README.md` for plans | ~0.3–4k |
| 4 | A symbol: `rg -n "fn name" crates/` then read that range | ~0.2k |
| 5 | Partial file — the enclosing block only (`sed -n 'A,Bp'`) | varies |
| 6 | Whole file — only when structure defeats partial reads | high |
| 7 | Several whole files — only for genuine cross-crate reasoning | high |
| 8 | Repo or vault dump | **prohibited** |

## Budgets (`graphify query --budget N`)

| Need | N |
|---|---|
| Single fact | 300–500 |
| Normal question | 500–1000 |
| Routine implementation | 800–1500 |
| Cross-crate reasoning | 1500–2500 |
| Architecture / debugging | 2000–4000 (say why if more) |

## Command output

Tool output enters context too. Keep it short:

| Instead of | Use |
|---|---|
| `cargo build` | `cargo check -q --message-format=short` |
| `cargo test` | `cargo test -q -p <crate>` (one crate, not the workspace) |
| `cargo clippy` | `cargo clippy -q --message-format=short -- -D warnings` |
| `forge test -vvvv` | `forge test` first; add `-vvv --match-test <name>` only for the failing test |
| full logs | `… 2>&1 \| tail -40` or `rg` for the error |

## Writing back

A durable fact found while working goes into a file once (graph, `docs/planning/`,
`DECISIONS.md`), not into scrollback. Record whether a graph answer worked:
`graphify save-result --question … --answer … --outcome useful|dead_end|corrected`, then
`scripts/graph.sh reflect` so dead ends are not re-derived.

## Sessions

Context cost grows quadratically with session length. At any natural breakpoint, rewrite
`.ai/STATE.md` and start a fresh session instead of letting one run long.

## Models

Mechanical work (edits, running gates, searches, scaffolding) → Sonnet subagents.
Reserve Opus for strategy, contract security, and hard debugging.
