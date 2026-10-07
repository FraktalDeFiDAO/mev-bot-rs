# Progress: sequencer-first engine

Spec `docs/tasks/sequencer-first-engine.md` · invariants `docs/tasks/sequencer-first-invariants.md` · facts `docs/arbitrum-facts.md`.
Read this file first in every session. Update at the end of every phase.

## Status

| Phase | State | Evidence |
|---|---|---|
| 1 Audit + design doc | **done, awaiting human approval (GATE)** | `docs/design/audit-sequencer-first.md`, `docs/design/sequencer-first.md`, `docs/arbitrum-facts.md` |
| 2-14 | not started | blocked on the Phase 1 gate |

## Phase 1: what was done

- Saved the spec and the standing invariants under `docs/tasks/`; linked the invariants from the repo `CLAUDE.md` without replacing its project rules.
- Re-verified Arbitrum behavior against docs.arbitrum.io on 2026-10-07 (PGA, Fast Feed, Use the Fast Feed, Read the sequencer feed, RPC methods,
  NodeInterface, precompiles, Stylus activation, node providers) plus the `ArbSys.sol` interface source. Every fact has a source and status in `docs/arbitrum-facts.md`.
- Audited the existing bot (12 areas with verdicts; explicit file:line findings for every list item in spec §13).
- Wrote the design doc (flows, state model, crate graph, per-protocol transitions, bid engine, reconciliation, migration plan).

## Conflicts between the spec and verified facts (recorded, design adapted)

1. **Fast Feed cost vs project budget:** ≥ 17 USDG per ticket-day (≈ $510/month per connection; spec wants two). Project budget is $200 / ~$0 infra. Needs a human decision.
2. **"Submit directly to the Sequencer":** the public sequencer endpoint is documented as best-effort, no SLA, with providers advised as primary. Design broadcasts to both and labels them.
3. **Conditional submission claims** (checks against the prior 1-2 s, rate-limited): not in the docs; bounds are an **L1** block-number window. Left as a measured experiment (Phase 12).
4. **"A round in progress cannot be interrupted":** docs describe early block finalization when a block fills. Treat 125 ms as an upper bound.
5. **PGA activation date** is not stated ("some time after the vote"); the repo's on-chain observation says tip-ordered since late September. Re-check before Phase 7.
6. **Invariants vs the existing live bot.** DECIDED by the owner 2026-10-07 ("all live"): the invariants bind the live bot too. Compliance matrix: audit §6 (compliant: #5, #8; partial: #2, #6; violated: #1, #3, #4, #7). Tension: invariant 4 forbids the RPC pre-send verification that stops phantom sends; full compliance needs the local exact path first.

## Open items for the human (Phase 1 gate)

See `docs/design/sequencer-first.md` §13. **Decided:** invariants scope (they bind the live bot too).
**Still open:** Fast Feed ticket policy and **experiment 0** (one ticket, measure the Fast Feed lead over the standard feed), the node for the correctness path
and replay tests, a throwaway shadow key and host for shadow runs, and whether to start the live-bot compliance quick wins now
(audit §6: async metrics, off-loop refresh, `StateCorrection` events, opportunity version stamps, `EXECUTION_MODE` alias).

## What the next phase needs (Phase 2: feed clients)

A recorded Fast Feed message sample (needs one ticket, or hand-written fixtures from the documented schema) to pin the receipt/log shape;
decision on dedupe/two-connection budget; a place to run shadow (separate port/host from the live bot).

## Verification log

- Docs fetched 2026-10-07; quotes in `docs/arbitrum-facts.md` come from those fetches, not from memory.
- Repo evidence: production event files on `mev-ohio` (Oct 4-7), live `rpc usage` and `presim` log lines, `rg` over the tree at `a399a9e`.
- Nothing was built, signed or sent in this phase; no keys touched; `EXECUTION_MODE` untouched.
