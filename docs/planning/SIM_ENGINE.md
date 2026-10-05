# Simulation engine: audit (Phase 0) and roadmap

Status: Phase 0 complete; Phase 2 in progress (2026-10-05). Target design: one opportunity engine (liquidation +
arbitrage strategies) → candidates → REVM as the execution authority → profit → competition → submit.

## Constraints that shape everything

- **No own node; $200 capital; ~$20/mo host** (AWS us-east-2 t3.small, 2 GB RAM, 2 vCPU). Anything
  needing a node or a big box is out until the bot earns.
- **Arbitrum ordering (superseded; verify):** this doc was written assuming FCFS + Timeboost
  (non-express txs wait ~200 ms, then ordered by arrival; priority fees buy nothing). The Arbitrum
  AIP replaces that with Priority Gas Auctions: per-tx `maxPriorityFeePerGas` priority, ties by
  arrival, 125 ms rounds, no express lane. Go-live date is unconfirmed (DECISIONS #49,
  AUDIT_2026-10-05.md). Until confirmed, treat the "latency, not gas bidding" conclusions below as
  provisional.
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
| 2 Hot-state DB: block diffs + snapshot + overlays | ✔ | ✔ | ✔ (diff-fed cache, pending overlay, worker) | ✔ | ✔ | ✔ (exact where complete; live coverage ~7 % → off, DECISIONS #45) | | ✔ (diff p50 ~170 ms, sim p50 0.4 ms) | ✔ |
| 3 Persistent parallel workers | ✔ | ✔ | partial (one persistent worker owns cache + overlay; no per-sim DB setup) | ✔ | ✔ | ✔ | | ✔ (0.4 ms/tx, one core suffices at feed rate) | ✔ |
| 4 DEX adapters + full-path REVM | ✔ | | | | | | | | |
| 5 Two-pool / triangular / multi-hop | ✔ | ✔ | ✔ (two-pool + triangular `CycleIndex`, pool-indexed) | ✔ | ✔ | | | | ✔ |
| 6 Funding abstraction | ✔ | ✔ | ✔ (`arb_calldata`: flash swap or Balancer per route; `fundable`) | ✔ | ✔ | | | | ✔ |
| 7 Aave flash-loan arbitrage | ✔ | ✔ (rejected: 5 bps fee > typical margin; flash swaps cover the depth) | – | | | | | | ✔ |
| 8 DEX-native flash swaps | ✔ | ✔ | ✔ (`executeFlashSwap`) | ✔ | ✔ (fuzz + invariant suite) | ✔ (fork: real arb captured, guards) | | | ✔ |
| 9 Aave liquidation index | ✔ | ✔ | ✔ (event index + predict) | ✔ | ✔ | ✔ | ✔ (`mev-liq-audit`) | | ✔ |
| 10 Flash-funded liquidation | ✔ | ✔ | ✔ (Balancer) | ✔ | ✔ | ✔ (fork E2E) | | | ✔ |
| 11 Liquidation route optimizer | ✔ | | partial (≤2 hops) | | | | | | |
| 12 Liquidation + arbitrage routes | ✔ | | | | | | | | |
| 13 Shadow books | ✔ | ✔ (Aave + Compound predict) | ✔ (Aave, Compound; Morpho/Fluid poll) | ✔ | ✔ | ✔ (live: Compound 0 @ today, 861 @ −40 %) | | | ✔ |
| 14 Dependency invalidation | ✔ | | partial (`affected`) | | | | | | |
| 15 Arbitrum fee / competition | ✔ | ✔ | ✔ (calibrated L2 gas; L1 share measured negligible) | ✔ | | ✔ (receipts) | | | ✔ |
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

## Phase 2 findings (measured on the us-east-2 host)

| Run | Result |
|---|---|
| dRPC block-diff follower, 6 min | 1,273/1,278 blocks, 0 rate limits, 5 "unknown block" (retried), p50 159–236 ms, lag p50 0 / p90 1 / max 4 blocks, ~64 changes/block |
| TTL cache (20 blocks) vs latest | 0/30 real swaps exact, 27 false movers |
| Diff-fed, re-simulated after fill, truth **same block** | 853 compared, 98 % exact, **124/124 real swaps exact**, ~2 % false movers |
| Diff-fed, first sight, truth **latest** | 2/12 real swaps exact (0–1 block lag) |
| Local simulate latency | p50 0.36–0.43 ms, p90 0.69–0.72 ms, p99 0.94–1.7 ms |

revm execution is correct on complete, same-block state. Remaining gap: freshness (the last block)
and completeness (first sight). Step 2: speculative overlay of feed txs + pre-warming.

## Phase 5: triangular arbitrage

`mev_search::cycles::CycleIndex` enumerates directed triangles `L → X → Y → L` (three distinct
pools, `L` a loanable token) once at startup and on each resync, capped at 20 000, indexed by pool.
A pool update prices only the cycles containing it: marginal-rate screen (product > 1 after fees),
then the same ternary size search as two-pool. The engine takes the best of two-pool and triangular
per update; the executor already runs any hop chain. Gas is priced per hop (below).

## Phase 13: shadow books, wrapped price feeds

Compound v3 predicts absorbs from a pending Chainlink report like Aave does. Two oracle defects
found and fixed on the way (they affected every adapter):

- **Wrapped feeds were not watched.** SVR proxies, CAPO adapters and custom wrappers revert on
  every aggregator getter, so their underlying aggregators never triggered anything. The resolver
  now follows `eth_createAccessList(latestRoundData)` to the contracts the feed reads.
- **Baseline answers were unreadable.** Access-controlled aggregators only serve reads where
  `msg.sender == tx.origin`; Multicall3 calls failed silently. Answers are read by direct
  `eth_call` (`mev_oracle::latest_answers`).

Feeds are not assumed proportional to their aggregator (wrappers rescale, compose or invert). Each
(aggregator, Comet, asset) link is +1 when price and answer differ by a power of ten, otherwise
learned from two sweeps where the answer moved ≥ 0.05 %; unknown links are never repriced.

## Phase 15: fee model (measured 2026-10-05)

A real two-hop arbitrage: 314 399 L2 gas, of which 570 for L1 data (548 calldata bytes, ~1 gas per
byte, 0.2 %), at 0.0201 gwei. The per-hop estimate (`arb_gas_estimate`: 100k + 115k × hops) is
calibrated on live receipts; the L1 component is below its error and isn't modelled per trade.
Under Timeboost, priority fees did not reorder non-express transactions (FCFS after the delay), so
the competition budget was latency, not gas bidding. Under PGA a tip does set priority (ties still
go to the earlier arrival), so the budget becomes latency plus a tip bid (DECISIONS #49–#50).

## Phase 2 result: off on free infrastructure (2026-10-05)

Measured on live traffic (worker vs mined receipts): the worker keeps up (dRPC diffs ~210 blocks/min,
lag 0-3, overlay rebuild ~0.5 s/min) and its complete predictions match the chain in 2-4 ms, but only
~7 % of feed txs complete locally and almost no pool-moving ones. Pool movers touch first-sight
state (new users' balances and allowances, newly crossed ticks); each such key costs an RPC round,
which is the latency of the RPC presim it would replace. Defects found and fixed on the way:
per-field fills got rate-limited (78 % failed) → one `eth_getProof` per address; head polling at
block rate tipped arb1 into 429 for the host → head from the feed's sequence number; unconfirmed
pending txs could make every block rebuild re-run hundreds of txs → 3 s expiry. Revisit with an own
node (DECISIONS #45).

## Phase 8: flash swaps

`executeFlashSwap(amount, minProfit, hops)`: the first hop's V3-style pool pays out first; the rest
of the route runs inside its swap callback and the pool is repaid from the proceeds (guard: a
transient `flashPool`, only the pool we started, once; inner V3 hops keep the `expectedPool` guard).
Fork tests capture a real arbitrage with nothing left in the executor. Balancer's Arbitrum balances
(~$14k USDC, 95 WETH, 3 WBTC) capped Balancer-funded loans; flash swaps lift that for V3-first
routes. Aave `flashLoanSimple` was not added: its 5 bps fee exceeds typical arbitrage margins.
