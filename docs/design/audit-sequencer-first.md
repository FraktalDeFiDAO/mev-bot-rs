# Audit of the existing bot against the sequencer-first spec (Phase 1)

Date 2026-10-07, repo at `main` (`a399a9e`). Scope: `docs/tasks/sequencer-first-engine.md` §13.
Verdicts: **keep** / **modify** / **replace**. File references are `path:line` at this commit.
Evidence for behavior claims comes from the production event logs (Oct 4–7, 155 race reports, 160 sends)
and the live `rpc usage` / `presim` log lines, not from reading code alone.

## 1. Verdict per area

| Area | Where | What it does today | Verdict |
|---|---|---|---|
| Architecture | `crates/mev-engine/src/lib.rs` (2,252 lines) | One `Engine` task: a single `tokio::select!` loop (`:606-690`) handles block logs, presim results, tx completions, liquidation events, resync and summary. Everything runs through `&mut self` on that task, including awaited RPC calls. | **modify**: split into the spec's hot path and correctness path; keep the crate layering rule (`docs/planning/CRATES.md`). |
| Block/event ingestion | `crates/mev-ingest/src/lib.rs:19-30` | WS `eth_subscribe logs` on pool addresses, event filter = V2 `Sync`, V3 `Swap`, Pancake `Swap`, V4 `Swap`, V4 `ModifyLiquidity` only. Arrives when the **block** is published (≈ 250 ms granularity). | **modify**: stays as the `SoftFinal` source, not the primary path; add the missing events (§2). |
| RPC usage | `crates/mev-rpc`, `mev-engine`, `mev-presim`, `mev-discovery` | Live: ≈ 150-260 `eth_simulateV1`/min (presim), 20-2,200 `eth_call`/min (liquidation sweeps), plus state refresh, tick reload, pre-send verification. | **replace** as primary data path (spec §14); keep as the correctness-path reader. |
| Sequencer feed | `crates/mev-sequencer/src/lib.rs:109-200` | Free standard feed, decodes `l2Msg` into signed txs (`SequencedTx`). Has **no receipts or logs** (facts S4), so every effect must be re-derived. `FeedLog` (`:159`) keeps arrival times for race analysis. | **keep**; becomes the `SoftFinal` / tx-list cross-check feed. |
| Fast Feed | `crates/mev-ingest/src/fast_feed.rs` (346 lines) | Client written to the documented protocol (bearer upgrade, JSON-in-binary-frame, `GapDetector`). Default off. The receipt is kept as raw `serde_json::Value` (`:44`): **logs are not decoded and never reach the state store**; the engine only uses it as another tx trigger (`engine/lib.rs:157,389`). | **modify**: this is the seed of the new hot path; add receipt/log decoding, 2-connection dedupe, the `(block, tx_index)` contiguity check. |
| Simulation / REVM | `crates/mev-localsim` (741+323+139 lines), `mev-presim`, `mev-sim` | `localsim`: revm worker fed by per-block prestate diffs, **off** since DECISIONS #45 (≈ 7 % of feed txs complete locally: first-sight state keys need RPC). No Arbitrum precompiles (`localsim/lib.rs:138` only skips them), no Stylus. `presim`: `eth_simulateV1` per candidate. `mev-sim`: executor `eth_call`. | **modify**: localsim becomes the §5.3 fallback (needs ArbSys/ArbGasInfo/NodeInterface shims and a Stylus guard); presim becomes the fallback for unmodeled txs, not the default. |
| Market cache | `crates/mev-state/src/lib.rs`, `mev-dex/src/lib.rs` | `PoolStore`: `HashMap<Address, Pool>`, mutated in place via `&mut`. `PoolState` is `f64` (`mev-dex/lib.rs:15,18`). No version, status, confidence or journal. | **replace** (spec §4): versioned, integer, snapshot-published store. |
| State versioning | none | `apply_log` (`mev-state/lib.rs:186`) applies absolute state with no `(block, tx_index)` check. | **replace**. |
| Route discovery | `mev-discovery`, `mev-search/cycles` | Factory scan at start (`discover_pools`), V4 registry, auto-discovery from logs, `CycleIndex::build` over all pools (`engine/lib.rs:1084-1087`, rebuilt every resync). Per-update search is indexed (good). | **keep** discovery; **modify** the index (pool → routes is there; add version/coalescing). |
| Execution / submission | `crates/mev-exec/src/lib.rs`, `mev-exec/src/nonce.rs` | Signs, broadcasts to all endpoints in parallel (`:219-230`, sequencer + RPCs), tracks `first_accept` and receipt. `NonceManager` take/release/reset. | **modify**: label endpoints (primary vs fallback), per-lane nonces, per-stage timestamps (§8). |
| Gas / priority logic | `crates/mev-risk/src/bid.rs`, `engine/lib.rs:993-1060` | `BidPolicy` + `TipBook` (quantiles of observed winner tips), per-route books (#61), caps per tx/min/day. It is a heuristic budget policy, **not** an EV model: no `P(win\|bid)`, no `revert_cost`. | **modify** → `BidEngine` (§7). |
| Executor gas | `contracts/src/ArbExecutor.sol`, `mev-sim/lib.rs:83-85` | Estimated 330 k gas (2 hops) to 445 k (3 hops), Balancer flash loan; `minProfit` on-chain. Heavy next to a gas-golfed competitor, and gas is a stated competitive lever (spec §1). | **modify** (Phase 10). |
| Reconciliation | none systematic | Only side effects: the 60 s `refresh` silently overwrites state from RPC (`discovery/lib.rs:294`), the 5 min tick reload, and `heal_route` after a failed simulation (`engine/lib.rs:907`) which logs `state_heal` drift. No `StateCorrection`, no reason codes, no first-divergence. | **replace** (§9). |
| Reorg / rollback | none | Searching `reorg|rollback|tx_index` in `mev-state` and the engine finds nothing relevant. A pending feed tx that never lands is never rolled back. | **replace**. |

## 2. Explicit findings (file:line)

**Remote RPC in the hot path**
- `mev-presim/src/lib.rs:287-300`: one `eth_simulateV1` task per candidate feed tx (≈ 38-41 ms mean, live).
- `engine/lib.rs:1265` (`simulate_after`, trigger + arb) and `:1280` (`simulate_fast`, an `eth_call`) before every send.
- `engine/lib.rs:1303` → `heal_route` (`:907`): `load_pools` RPC after a failed simulation.
- **Inside the single event loop:** `resync` → `refresh(...).await` and `rebuild_cycles()` (`:664-669`), `tick_reload` → `refresh_ticks(...).await` (`:671-676`), reconnect refresh (`:682`). While these await (hundreds of ms), no new event is processed.

**Full market scans:** `rebuild_cycles` (`:1084`) over ~65-73 k cycles and `watchlist(&store)` (`:2073`) every 60 s, inline; `discovery::refresh` (`:294`) reloads every pool; `scan_once` (`:688`) is diagnostic-only (not hot).

**DB / disk in the hot path:** no DB, but `Metrics::event` (`mev-metrics/lib.rs:84-97`) takes a `Mutex`, `writeln!`s and **`flush()`es per event**, and the engine calls it from 30 sites, several on the send path (e.g. `sim_revert`, `bid`, `sent`).

**Data races / ordering:** no memory races in the engine (single task), but `FeedLog` and the feed `TipBook` are `Arc<Mutex<…>>` shared with feed tasks. The real problem is **ordering**: presim spawns a task per tx (`presim/lib.rs:287`) and forwards results as they finish (`:310`), so two txs on one pool can apply out of sequencer order; `apply_log` (`mev-state/lib.rs:186`) accepts any absolute state, so a late result can regress a pool.

**State that cannot be reconstructed:** `PoolStore` is memory-only, f64, no journal, no snapshot; tick maps are loaded and replaced wholesale (`discovery::refresh_ticks`). After a restart or a corrupt update there is no way to say which tx a pool's state reflects.

**Likely drift sources (ranked, from data):**
1. Pending-tx effects applied into the shared store as if final (presim, `engine/lib.rs:~660`), never rolled back; heal drift of 3-77 bps was partly this.
2. Events not subscribed: V3/Pancake `Mint`/`Burn` (active liquidity and tick `liquidityNet` change silently until the 5 min tick reload), Algebra fee events (`mev-ingest/lib.rs:19-26`).
3. `refresh` overwriting event-applied state with an RPC read from a rotating, possibly lagging endpoint (`discovery/lib.rs:294-303`, `mev-rpc` rotating provider).
4. Hop-level exactness assumptions: 14 of 14 model-only sends reverted for ≈ $2 in 80 minutes (DECISIONS #75).

**Unsupported transitions:** V3 liquidity changes, Algebra dynamic fee, V4 hooked pools (deliberately excluded), fee-on-transfer/rebasing tokens (not detected), Curve/Balancer-style pools (no adapter), Stylus-dependent pools (no detection).

**Float math in financial paths (violates invariant 1):** `PoolState` (`mev-dex/lib.rs:15,18`), all pricing and tick walking (`mev-dex`, 39 `f64` uses), size search (`mev-search`, 25), `Opportunity.est_profit`/`est_profit_usd` (`mev-types/lib.rs:100,102`), bid policy (`mev-risk/bid.rs`, 30), engine profit/gas arithmetic (33). Only `amount_in` and executor calldata are `U256`. The on-chain `minProfit` is the only integer guard.

**Stale-opportunity windows:** `SIM_OK_TTL` 300 s (`engine/lib.rs:105`), `ROUTE_COOLDOWN` 30 min (`:96`), `SIM_BAD_TTL` 120 s (`:111`), tick reload 300 s, pool refresh 60 s. Opportunities carry no state versions (invariant 3 not met).

**Wasted simulations:** presim: 64-79 % of simulated feed txs have no pool effect (`no_pool_effect` 78-167 of 99-260 candidates per minute); 16,945 simulation reverts across 4,301 distinct routes in 80 minutes before the back-off (#70); `exact`/`unvetted` heuristics decide who is simulated, not state versions.

**Submission latency sources (measured):** trigger→signed 32-162 ms (presim RPC ≈ 40 ms inside it; plus the new pre-send verification ≈ 40 ms); signed→first accept 100-250 ms (`first_accept` median 202 ms presim, 209 ms block-triggered); broadcast fan-out waits on the first accept across three endpoints including public RPCs (`mev-exec/lib.rs:219-230`); block-triggered sends start ≥ one block after the swap (≈ 70 % of sends so far); the public sequencer endpoint has no SLA (facts R1).

## 3. What the data says about the spec's premise

The 155 races on record: **0 wins**. In every race with an identified winner (71) it reached the standard feed in the same message as ours, in the block after the trigger. Of 24 with tip data, 14 were lost on tip (winners paid ≥ us, up to 3 gwei), 10 on timing (we paid more, sat ≈ 9 places behind). The standard feed is **per block**: a competitor on the Fast Feed sees each tx per round, up to one block earlier than we do. That is the best-supported explanation for the "timing" losses and is the first thing to test (design doc §13, experiment 0) before the larger rebuild.

## 4. Keep / modify / replace summary

- **Keep:** `mev-bindings`, `mev-config`, `mev-sequencer` (feed decode), `mev-discovery` (registry, V4), contracts' structure, nonce basics, the whole liquidation stack (out of this spec's scope), event/race telemetry.
- **Modify:** `mev-ingest::fast_feed` (decode receipts/logs, 2 connections), `mev-localsim` (Arbitrum shims, Stylus guard), `mev-exec` (lanes, labels, timestamps), `mev-risk::bid` (EV), `ArbExecutor` (gas), `mev-engine` (split paths).
- **Replace:** `mev-state`/`mev-dex` state and math (versioned, integer), presim-as-primary, `refresh` as reconciliation, synchronous metrics.

## 6. Compliance matrix: the live bot against the eight invariants (owner decision: they bind the live bot too)

| # | Invariant | Live bot today | Remediation | Blocked by |
|---|---|---|---|---|
| 1 | Integer math only | **Violated**: `f64` in `mev-dex`, `mev-state`, `mev-search`, `mev-risk::bid`, engine profit/gas math, `Opportunity.est_profit*` (≈ 140 sites, audit §2). On-chain `minProfit` is the only integer guard. | New `mev-amm` + `mev-quote` (U256, exact rounding); retire the f64 stack. Not patchable in place. | Phases 4, 7, 8 |
| 2 | Chain state wins, every overwrite recorded as `StateCorrection` | **Partial**: chain wins (refresh/heal overwrite), but overwrites are silent (`refresh` `discovery/lib.rs:294`, tick reload) except `state_heal` on failed sims. No reason codes. | **Quick win:** emit a `StateCorrection`-shaped event (market, predicted vs actual, reason) on every overwrite, async. Full taxonomy + first-divergence in Phase 6. | none for the quick win |
| 3 | Opportunity carries exact state versions; drop if superseded | **Violated**: no versions anywhere (`apply_log` `mev-state/lib.rs:186`). | **Interim quick win:** per-pool update counter in `PoolStore`, stamped on each `Opportunity`, re-checked before simulation and before send. Real versions come with `mev-market` (Phase 3). | none for the interim |
| 4 | Hot path purity (no RPC, no disk/DB, no blocking log, no global scans between feed receipt and submit) | **Violated four ways**: pre-send `eth_call`/`eth_simulateV1` (`engine/lib.rs:1265,1280`), presim RPC per candidate (`presim/lib.rs:287`), synchronous flush per metric event (`metrics/lib.rs:84-97`, 30 call sites), awaited `refresh`/`refresh_ticks`/`rebuild_cycles` inside the event loop (`engine/lib.rs:664-682`). | **Quick wins:** (a) metrics through a bounded droppable channel + writer task; (b) refresh, tick reload and cycle rebuild in spawned tasks that swap results in. **Hard part:** the RPC verification and presim are what protect us from phantom sends (DECISIONS #75, #77); removing them without a local exact simulator re-opens ≈ $1.50/h of phantom losses. | the RPC part needs local exact state (own node or Fast Feed logs + exact integer quotes), i.e. Phases 2-5, 7-9 |
| 5 | `EXECUTION_MODE` defaults to `shadow`; no live set by the agent | **Compliant in effect**: `compose.yaml:17` defaults `MEV_MODE` to `shadow`; live needs `scripts/run.sh live` and a typed confirmation, performed by the human. Name differs. | Accept `EXECUTION_MODE` as the variable (keep `MEV_MODE` as an alias). | none |
| 6 | Own transactions take the normal path, no special-casing | **Partial**: our txs arrive via the feed/logs like any other, but `claim_pools` (`engine/lib.rs:825,1371,1413`) marks pools busy while a send is in flight, and per-route cooldowns/gates key on our own outcomes. | Review: keep busy-marking only as an *opportunity-level* guard, not as state; own txs update state only through ingestion. | Phase 3 design |
| 7 | Degraded state is contagious | **Violated**: no confidence concept. | `Confidence` per market in `mev-market`; strategies declare a minimum. Interim: a pool whose last update failed or whose feed gap is open is excluded from search. | Phase 3 |
| 8 | No stubs reported as done | **Compliant**: no `todo!`/`unimplemented!`/TODO panics in non-test code (checked 2026-10-07). | Keep the check in `scripts/check.sh`. | none |

**Consequence worth deciding.** Invariant 4 forbids exactly the pre-send RPC verification that stopped the phantom-send losses. Full compliance therefore *requires* a local, exact way to price and verify (Fast Feed logs plus integer AMM math, or a local node with REVM) before the RPC checks can be removed. Until then the live bot is knowingly non-compliant on #4 (and #1, #3, #7); the quick wins reduce the gap without re-opening losses. If you prefer strict compliance now, the alternative is to run live bot in shadow until the local path exists.

### Compliance progress

| Date | Change | Invariants moved |
|---|---|---|
| 2026-10-07 | Wave 1 (DECISIONS #78): async metrics writer, background refresh/tick reload/cycle rebuild, guarded refresh apply + `state_correction` events, per-pool versions on opportunities, `EXECUTION_MODE`, stub check in `scripts/check.sh` | #2 partial → mostly compliant (reason codes still `Unknown`); #3 violated → interim (versions stamped and re-checked, not `(block, tx_index)`); #4 two of four causes removed (metrics I/O, awaited maintenance in the loop); #5 compliant under the right name; #8 now enforced |
| next | Wave 2: pool busy-marking review (#6), confidence per pool excluded from search when its last read failed (#7 interim), reason-code classification of corrections | #6, #7 |
| later | New stack: integer AMM math, `mev-market`, Fast Feed log-driven state, local exact simulation | #1, #3 full, #4 RPC part, #7 full |
