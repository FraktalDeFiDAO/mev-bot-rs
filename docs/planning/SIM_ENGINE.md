# Simulation engine: audit (Phase 0) and roadmap

Status: Phase 0 complete (2026-10-05). Target design: one opportunity engine (liquidation +
arbitrage strategies) → candidates → REVM as the execution authority → profit → competition → submit.

## Constraints that shape everything

- **No own node; $200 capital; ~$20/mo host** (AWS us-east-2 t3.small, 2 GB RAM, 2 vCPU). Anything
  needing a node or a big box is out until the bot earns.
- **Arbitrum ordering is FCFS + Timeboost**: non-express txs wait ~200 ms, then are ordered by
  arrival. Priority fees do not buy position; the "PGA" lever is the express lane (paid auction).
- **Measured race position (race reports, 2026-10-04/05):** we land in the *same feed batch* as the
  winner, 5–7 positions behind. Winners reach the sequencer ~5–15 ms after the trigger; we take
  18 ms (best, feed presim) to ~120 ms (block-log path). Trigger→send latency decides the race.

## Architecture map (as built)

| Stage | Component (crate) | What it does today |
|---|---|---|
| Observe | `mev-sequencer` | Sequencer feed WS → decoded txs; 20 s stall reconnect; `FeedLog` arrival clock |
| Observe | `mev-ingest` | WS block logs (V2 `Sync`, V3/Algebra/Pancake `Swap`) for watched pools |
| Observe | `mev-presim` | Feed txs → `eth_simulateV1` (free RPCs) → pool logs a block early; calldata watchlist filter |
| State | `mev-state` `PoolStore` | Pool states keyed by address, pair index (`pools_for`, `siblings`), `apply_log` |
| State | `mev-dex` | f64 pool math: V2 reserves; V3/Algebra/Pancake as **virtual reserves of the current tick** |
| State | `mev-lending-*` | Aave/Compound/Morpho/Fluid adapters: event index, health, plan; Aave `predict` snapshot |
| Detect | `mev-search` | `two_pool_arbs` (2-pool cycles, ternary-search size), `best_route` (≤2-hop exits) |
| Detect | `mev-liquidate` | Runner: sweeps, feed-triggered polls, **oracle backrun** (Aave), backoff |
| Simulate | `mev-sim` | `eth_call` of the executor (liquidations; arbs only with `arb_presimulate`) |
| Simulate | `mev-localsim` (branch, WIP) | revm 43 over a self-filling cache; probe vs `eth_simulateV1` |
| Profit/Risk | `mev-engine`, `mev-risk` | Model/sim profit − gas; min profit; daily gas cap, arb sub-budget, lost-race accounting |
| Submit | `mev-exec` | Sign once, broadcast to sequencer + RPC in parallel; nonce `AtomicU64`; receipts |
| Contract | `ArbExecutor.sol` | Balancer V2 flash (0 fee); hops V2/CamelotV2/V3/PancakeV3/Algebra; `liquidate` with owner-allowlisted protocol calls |
| Telemetry | `mev-engine::race`, presim stats | Per-attempt timeline vs trigger feed arrival, winner index, per-endpoint send times |

## Classification

| Component | Verdict | Why (evidence) |
|---|---|---|
| `ArbExecutor` callbacks | **KEEP** | V3-style callbacks authenticated by transient operation context (`expectedPool`, `expectedTokenIn`, `expectedMaxIn`, cleared in callback, checked after swap); flash callback checks vault + `flashActive`; `minProfit` invariant reverts losses. Owner-only entry; protocol calls allowlisted; exact approvals reset to 0. |
| `ArbExecutor` funding | **MODIFY** (Ph 6–8) | Balancer V2 only. No Aave `flashLoanSimple`, no V3 flash swap, no own-capital path. |
| `mev-sequencer` | **KEEP** | Light decode on the read task, stall detection proven live. |
| `mev-presim` (eth_simulateV1) | **KEEP as fallback** | Mean ~70 ms on free RPCs (rate limits); too slow as primary. |
| `mev-localsim` | **MODIFY** | revm path is fast (p50 0.43 ms, p90 0.79 ms on the host) but **0/30 real swaps exact**: time-based staleness. Also rebuilds the EVM per simulation; cache behind a `Mutex`. |
| `PoolStore` | **MODIFY** | Good pair index; no state versions, no PoolId, no route/cycle index. |
| `mev-dex` V3 math | **MODIFY** (filter only) | No tick traversal: wrong for tick-crossing swaps. Acceptable as a filter if REVM confirms. |
| `mev-search` | **MODIFY** | 2-pool only; no triangular/multi-hop cycles; no `PoolId → cycles` index. |
| Lending adapters + runner | **KEEP** | Live; Aave predict is the shadow-book seed. Compound/Morpho predict missing. |
| `mev-engine` | **MODIFY (split)** | 1.2k-line orchestrator mixing detection, sizing, profit, send. Split into strategy/candidate/profit/submit modules as phases land. |
| `mev-exec` nonce | **MODIFY** | `fetch_add`/`fetch_sub` with concurrent sends (backruns may overlap an arb) can leave gaps on partial failure. |
| Arbitrum fee model | **MODIFY** (Ph 15) | Arb path prices gas as constant × gas price; L1 data cost not modeled separately. |
| Tests | **KEEP + ADD** | Unit/property/fork/E2E exist. **No benchmarks, no REVM-vs-chain harness, no arb replay.** |
| Not applicable | — | Go/Rust boundary (all Rust), Diamond facets (none), "Fast Feed" (standard sequencer feed only). |

## The blocking problem: state freshness without a node

Measured: dRPC serves `debug_traceBlockByNumber` + `prestateTracer` `diffMode` in **240–290 ms,
40–190 KB/block**; PublicNode and arb1 do not. Applying every block's diff to the hot cache keeps
REVM's base state **exact** ~1 block behind; the feed overlay (simulated pending txs) covers the
current block. This replaces TTL staleness. Open question: dRPC free-tier sustainability at ~4
blocks/s — measure before depending on it; fallback is per-pool overlays from `PoolStore` + targeted
`eth_getStorageAt` refreshes.

## Phase checklist

Legend: A audited · D designed · I implemented · C compiled · U unit tested · T integration tested ·
R replay tested · B benchmarked · Doc documented.

| Phase | A | D | I | C | U | T | R | B | Doc |
|---|---|---|---|---|---|---|---|---|---|
| 0 Audit + map | ✔ | ✔ | – | – | – | – | – | – | ✔ |
| 1 REVM correctness baseline | ✔ | ✔ | ✔ (probe) | ✔ | ✔ | ✔ (live probe) | | ✔ (p50/p90/p99) | ✔ |
| 2 Hot-state DB: block diffs + snapshot + overlays | ✔ | ✔ | | | | | | | |
| 3 Persistent parallel workers | ✔ | | | | | | | | |
| 4 DEX adapters + full-path REVM | ✔ | | | | | | | | |
| 5 Two-pool / triangular / multi-hop | ✔ | | | | | | | | |
| 6 Funding abstraction | ✔ | | | | | | | | |
| 7 Aave flash-loan arbitrage | ✔ | | | | | | | | |
| 8 DEX-native flash swaps | ✔ | | | | | | | | |
| 9 Aave liquidation index | ✔ | ✔ | ✔ (event index + predict) | ✔ | ✔ | ✔ | ✔ (`mev-liq-audit`) | | ✔ |
| 10 Flash-funded liquidation | ✔ | ✔ | ✔ (Balancer) | ✔ | ✔ | ✔ (fork E2E) | | | ✔ |
| 11 Liquidation route optimizer | ✔ | | partial (≤2 hops) | | | | | | |
| 12 Liquidation + arbitrage routes | ✔ | | | | | | | | |
| 13 Shadow books | ✔ | partial (Aave predict) | partial | | | | | | |
| 14 Dependency invalidation | ✔ | | partial (`affected`) | | | | | | |
| 15 Arbitrum fee / competition | ✔ | | | | | | | | |
| 16 Low-latency feed | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ | | partial (race telemetry) | ✔ |
| 17 Historical replay | ✔ | | partial (liquidations) | | | | | | |
| 18 Profile-driven hardening | ongoing | | | | | | | | |

## Order of work (evidence-driven)

1. **Phase 2**: diff-fed hot state + immutable snapshot + per-simulation overlay; rerun the accuracy
   probe (target: ≥95 % of real pool-moving swaps exact, 0 hot-path RPC). Gate for everything else.
2. **Phase 3**: persistent revm workers (no per-simulation EVM construction), benchmarked.
3. **Phases 4–5** on top: REVM-confirmed two-pool, triangular and multi-hop with a cycle index.
4. **Phases 6–8, 10–12**: funding abstraction (Aave `flashLoanSimple`, V3 flash swap, own capital),
   flash liquidation with optimized exits, combined routes.
5. **Phase 15** fee/competition model, **17** replay, **18** profiling throughout.
