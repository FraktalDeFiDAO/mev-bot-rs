# JEV-LIQ-006 — Pure baseline replay

Owner: Codex continuation of Claude's `feat/liq-baseline-replay` worktree. Depends on
JEV-LIQ-005 (`feat/liq-scheduler`, PR #126). The production engine, executor and mandatory
scheduler are unchanged. The existing `BaselinePolicy` / `RankingPolicy::schedule` supplies
every dispatch decision; this crate is an offline event runner and evidence exporter.

## Interface and ordering

`mev-liq-replay::replay(&ReplayStream, &ReplayConfig, code_revision)` takes caller-supplied
time and capacity and returns `ReplayReport`. It performs no RPC, Jev, signing, database or
clock I/O. The executable loads a JSONL recording and writes JSON. This is the production
baseline policy evaluated with a fixed service-time model, not a second live scheduler.

The format starts with a versioned header, followed by events ordered by `(t_ns, seq)`.
The loader rejects duplicate JSON keys, unknown fields, unsupported chains, bad decimal
U256 amounts, duplicate sequence IDs, unregistered account events, backward blocks without
a reorg, inconsistent eligibility labels and unmatched/invalid gaps. Input is capped at
16 MiB / 100,000 events. Replay additionally caps scheduler calls and check records.

Observed events, clock ticks and check completions wake the policy. Ground-truth state and
labels never create extra dispatch opportunities. A check reads the latest recorded state
**at its start**; a later state change cannot affect that result. Its successful-check age
starts at that read, not the completion of an older response. Newer triggers survive old
completions. Missing recorded state remains unknown and is reported as an unavailable read.
Pending work is represented by the same `InFlight` lease type used by the scheduler.

Oracle/reserve events affect known dependent borrowers and accounts with unknown dependencies.
This format models one dependency asset per account. It does not fabricate token balances;
the baseline therefore uses its normal debt-size fallback for fairness cohorts. Repeated
dirty reasons coalesce by account/reason while preserving their earliest urgency and newest
chain reference. Gaps use the scheduler's existing coverage-gap input.

Reorgs restore recorded ancestor state, invalidate all cached snapshots and in-flight reads,
and require fresh rechecks. Labels from orphaned blocks are counted separately and excluded
from detection denominators. A restored eligible state needs an explicit as-of label.
Block hashes in the replay are deterministic identities derived from block/epoch; they are
**not verified blockchain hashes**. These modeled snapshots cannot be used for trading.

## Evidence and reproduction

Run from this branch/worktree, using the existing rootless Podman toolchain:

```bash
python3 crates/mev-liq-replay/fixtures/generate.py
scripts/dev.sh cargo test -q -p mev-liq-replay --locked
scripts/dev.sh cargo run -q -p mev-liq-replay --locked -- \
  --input crates/mev-liq-replay/fixtures/quiet.jsonl \
  --output docs/audits/evidence/jev-liq-006/quiet.json \
  --code-revision 4149baf881656812ff58629085455f964cb86154
scripts/dev.sh cargo run -q -p mev-liq-replay --locked -- \
  --input crates/mev-liq-replay/fixtures/oracle-shock.jsonl \
  --output docs/audits/evidence/jev-liq-006/oracle-shock.json \
  --code-revision 4149baf881656812ff58629085455f964cb86154
scripts/dev.sh cargo run -q -p mev-liq-replay --locked -- \
  --input crates/mev-liq-replay/fixtures/overload.jsonl \
  --output docs/audits/evidence/jev-liq-006/overload.json \
  --code-revision 4149baf881656812ff58629085455f964cb86154 --concurrency 1 --recheck-ms 1000
```

All three recordings are deterministic **synthetic** fixtures, with generation provenance,
input digest, policy parameters, source block range, seed and code revision repeated in
reports. Report clocks and latencies are modeled nanoseconds, not measured host latency.
The fixture blocks are generated labels, not actual Arbitrum block ranges. No fixture
asserts economic actionability (`actionable=false`). No API credentials are needed.

Reports contain ordered checks, P0–P3 counts, modeled account reads/busy time, mandatory
backlog peak, deadline-miss observations and unique account/tier/deadline misses, coverage
violations, invalidations, eligibility detection counts and nearest-rank p50/p95/p99.
Warm-up excludes episodes starting before `measure_from_ns` from detection metrics;
workload counters describe the entire recording. Episodes ending before a check completes
receive no detection credit. Replay stops at the header duration: later completions remain
pending, and open episodes are right-censored. Deadline misses follow the scheduler's own
deadline semantics; this is not a separately invented completion SLA.

## Limits and next tasks

Observed local gates (2026-10-13, rootless Podman): workspace format and clippy
`--all-targets --locked -- -D warnings` passed; workspace tests passed **707, failed 0,
ignored 28** across 175 test summaries, including 16 replay tests (one property test runs
256 generated permutations). The existing Python CI suite passed 3 tests. Foundry,
network/fork tests and browser tests were not rerun for this offline Rust-only change.
`scripts/graph.sh optimize` completed; generated graph/Obsidian files stay outside this PR.

GitHub CI is blocked independently of this change: scheduler dependency PR #126's
`changes` annotation says jobs could not start because recent account payments failed
or the spending limit needs to increase. Do not merge this stack until GitHub can run
the required checks successfully. No account billing settings were changed.

This establishes deterministic scheduling evidence and modeled coverage behavior. It
does not measure actionable recall, REVM CPU, transaction economics, competitor outcomes,
live RPC costs, live coverage, or Jev benefit. Historical exporter/independent economic
labels are JEV-LIQ-015/016; equal-budget four-arm evaluation is JEV-LIQ-017. The legacy
runner lacks per-account poll timestamps, so a live legacy-versus-baseline comparison
still needs instrumentation and cannot be claimed from these fixtures.

Dependency changes: one new workspace crate; existing Alloy/REVM versions are unchanged.
No infrastructure, persistent schema or trading migration. Rollback removes this offline
crate and its evidence; no bot configuration changes are required. Inputs stay local;
reports contain fixture/recorded borrower addresses and must be reviewed before sharing
real recordings. No network clients or signing permissions are linked by the replay code.

JEV-LIQ-008 and 009 remain unfinished in Claude's separate worktrees. Neither is claimed
complete by task 006. Do not activate Jev or merge through red CI.
