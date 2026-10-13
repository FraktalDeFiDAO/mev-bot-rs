# JEV-LIQ-006 baseline evidence

Generated 2026-10-13 from source commit `4149baf881656812ff58629085455f964cb86154`
using the tested executable in the rootless Podman dev container. All input recordings
are **synthetic**; this demonstrates scheduler/model behavior, not live performance,
historical liquidation recall or profitability. No economic actionability labels are asserted.

| Recording | Completed modeled checks | Peak mandatory backlog | Unique mandatory deadline misses | Eligible labels detected |
|---|---:|---:|---:|---:|
| [Quiet](quiet.json), 16 accounts / 600 s | 64 | 0 | 0 | no labels |
| [Oracle shock](oracle-shock.json), 48 accounts / 120 s | 240 | 32 | 0 | 24 / 24 |
| [Overload](overload.json), 80 accounts / 60 s | 60 | 79 | 50 | no labels |

Oracle-shock detection latency is **modeled** p50 20 ms, p95/p99 40 ms under a configured
20 ms account-read service time, concurrency 16 and 100 ms policy ticks. The overload
recording deliberately uses concurrency 1 and 1,000 ms reads: its missed deadlines and
coverage violations are recorded, not hidden behind a coverage claim.

Each JSON contains its canonical input hash, full scheduler parameters, ordered checks,
source range, generation provenance and code revision. Busy time and account reads are
model counters. Open episodes and outstanding reads are right-censored at replay end.
The input format has synthetic block identities and one modeled dependency asset per
account; see [task design](../../../design/jev-liq-006-baseline-replay.md) for limits and
exact reproduction commands. [verification.json](verification.json) records observed
local checks and the unrelated GitHub billing/spending-limit runner blocker.

`manifest.json` contains SHA-256 hashes of all three inputs and reports. The CLI integration
test checks repeated runs produce byte-identical JSON; the replay property test runs 256
input permutations through the actual `BaselinePolicy`.
