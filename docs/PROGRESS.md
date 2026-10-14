# Progress: sequencer-first engine

Spec `docs/tasks/sequencer-first-engine.md` · invariants `docs/tasks/sequencer-first-invariants.md` · facts `docs/arbitrum-facts.md`.
Read this file first in every session. Update at the end of every phase.

## Jev continuation (2026-10-13): JEV-LIQ-006

Baseline replay implemented on `feat/liq-baseline-replay`, stacked on scheduler PR #126.
Source commit `4149baf`; design and reproduction commands in
`docs/design/jev-liq-006-baseline-replay.md`, recorded synthetic baseline evidence in
`docs/audits/evidence/jev-liq-006/`. The same production `BaselinePolicy` handles dispatch;
the offline runner adds strict event loading, caller-supplied time, in-flight read/reorg
handling and machine-readable ordering/coverage metrics. No live engine/executor changes.

Verified: format, workspace clippy, 707 workspace Rust tests (0 failed, 28 ignored),
16 final replay tests including 256 permutation cases, and 3 Python tests. Graph optimized.
Synthetic shock detected 24/24 eligible labels; modeled p50/p95/p99 20/40/40 ms.
Overload explicitly reported 50 unique mandatory deadline misses. These are model results,
not historical economic recall, live latency or profitability claims.

GitHub CI jobs are blocked before startup by account payments/spending limits (PR #126
annotation); retain a draft and do not merge through red checks. Next independent work:
complete Claude's existing JEV-LIQ-008 outbox and JEV-LIQ-009 client worktrees, then task 010.
Task 015/016 still needs historical eligibility/economic labels for comparative evaluation.
This work does not mark a sequencer-first phase complete or authorize deployment/live Jev.

## Status

| Phase | State | Evidence |
|---|---|---|
| 1 Audit + design doc | **done, awaiting human approval (GATE)** | `docs/design/audit-sequencer-first.md`, `docs/design/sequencer-first.md`, `docs/arbitrum-facts.md` |
| 2-14 | not started | blocked on the Phase 1 gate |

## Decoder workstream (owner go-ahead 2026-10-08, ahead of the Phase 1 gate)

- Goal: decode every swap, liquidity change and liquidation seen on the sequencer feed, with coverage proved against chain receipts. See DECISIONS #89 (decoder, shadow), #90 (feed recorder), #91 (chain outcomes + coverage audit).
- Done: calldata decoder for the three non-public contracts that make up 94% of pool movers (A and B end at their price limit; C is a single-hop exact input), shadow comparison with the node simulation (93% exact), feed recorder, chain outcome sampler, `scripts/analyze-outcomes.py`.
- Limit stated plainly: a feed transaction's *success* is not knowable from the feed; chain state (block logs) stays the source of truth and decoded effects stay tentative.
- Next: read the coverage report, then add decoders in descending order of uncovered volume (public routers, aggregators, position managers, V4, lending liquidations).

## Phase 1: what was done

- Saved the spec and the standing invariants under `docs/tasks/`; linked the invariants from the repo `CLAUDE.md` without replacing its project rules.
- Re-verified Arbitrum behavior against docs.arbitrum.io on 2026-10-07 (PGA, Fast Feed, Use the Fast Feed, Read the sequencer feed, RPC methods,
  NodeInterface, precompiles, Stylus activation, node providers) plus the `ArbSys.sol` interface source. Every fact has a source and status in `docs/arbitrum-facts.md`.
- Audited the existing bot (12 areas with verdicts; explicit file:line findings for every list item in spec §13).
- Wrote the design doc (flows, state model, crate graph, per-protocol transitions, bid engine, reconciliation, migration plan).

## Compliance workstream (owner: invariants bind the live bot)

- **Wave 1 shipped in PR "compliance wave 1" (DECISIONS #78):** async metrics, background maintenance, guarded refresh + `state_correction` events, opportunity version stamps, `EXECUTION_MODE`, stub check.
  Verified by unit tests (metrics burst/drop, version bump semantics, refresh vs newer event), `scripts/check.sh`, and `scripts/test-e2e-crash.sh` (5/5 in 4 s).
- **Wave 2 (next, no new money/hardware needed):** pool busy-marking review (#6), per-pool confidence from read failures (#7 interim), correction reason classification.
- **Experiment 0 tooling ready (DECISIONS #80, `docs/runbooks/fast-feed-experiment.md`):** `mev-feedlead`, `scripts/fast-feed-ticket.sh`, Fast Feed client fixed for binary frames and covered by a mock-server test. Waiting on the operator to buy one ticket: a FIRST-TIME buyer can buy only in the second half of a round (grandfather period), i.e. 2026-10-08 05:00-17:00 UTC; the ticket is live 17:00 UTC that day for 24 h. Optional vault + keeper automation for renewals (DECISIONS #81, runbook path B).
- **Blocked on human decisions:** Fast Feed ticket (experiment 0), node for replay/reconciliation, shadow key + host. Phases 2+ proceed with recorded fixtures where they can.

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

## Audit remediation (2026-10-11, in progress)

The full audit and its evidence are in `docs/audits/2026-10-11-complete-audit.md`. The user authorized fixing all findings. Local implementation and verified limitations are tracked in `docs/audits/2026-10-11-remediation.md`; this work does not mark a sequencer-first phase complete. Integer strategy math, Atlas escrow settlement and hot-path purity remain open. No production deployment, funding, production signing or key access has been performed.
