# Design: sequencer-first, block-reconciled engine (Phase 1)

Status: **proposal for the Phase 1 gate.** Spec: `docs/tasks/sequencer-first-engine.md`. Invariants:
`docs/tasks/sequencer-first-invariants.md`. Facts: `docs/arbitrum-facts.md`. Audit: `docs/design/audit-sequencer-first.md`.
Interface sketches are Rust-flavoured and illustrative.

## 1. Current vs proposed data flow

**Today (legacy bot, live on `mev-ohio`)**
```
standard feed (per block, tx bodies only) ─► presim: eth_simulateV1 per candidate (RPC, ~40 ms) ─► apply_log (f64, unversioned)
ws eth_subscribe(logs) per block ───────────────────────────────────────────────────────────────► apply_log        │
                                                                                          search (f64) ► eth_call verify ► bid ► sign ► broadcast ×3
60 s refresh / 5 min tick reload: RPC reads overwrite state silently (inside the same event loop)
```
**Proposed**
```
HOT PATH                                                                                    CORRECTNESS PATH (async, may lag)
Fast Feed ×2 (tx + receipt + logs) ─► dedupe/gap ─► classify ─► adapter(logs→delta) ─► versioned store ─publish─► standard feed / node block
        │ fallback per market                                         │ atomic snapshot (Tentative)          │      │
        └─► local revm / node read (marked slower-confidence)         ▼                                       ▼      ▼
                                                      dirty scheduler ─► integer quote ─► size ─► simulate ─► bid ─► sign ─► submit(seq)
                                                                                                          reconcile: read touched markets at block hash,
                                                                                                          compare hashes, StateCorrection, first-divergence replay
```
Shared state is only the versioned market store. Everything on the right of the store reads snapshots; nothing on the hot
path does RPC, disk, or blocking logging (invariant 4).

## 2. Feed ingestion

- `FastFeedClient` (extends `mev-ingest::fast_feed`): bearer upgrade within 5 s (F5), drains faster than arrival (a lag > 2048 closes the
  socket, F6), decodes `FeedMessage` incl. `receipt.logs` and `status`. Two connections (two tickets), dedupe by `tx_hash`,
  per-connection arrival time recorded. Contiguity check on `(block_number, tx_index)`; a gap marks affected markets `Degraded`.
- `SequencerFeedClient` (existing `mev-sequencer`): the free per-block feed, `SoftFinal` source and tx-list cross-check. Block =
  `sequenceNumber + 22207817` (S2).
- Hot-path I/O is two bounded lock-free queues (SPSC per connection → single decode/apply thread). Overflow drops diagnostics, never state
  updates; a dropped state update is a gap and lowers confidence.

## 3. Node integration

A node is only on the **correctness path** (and as a per-market fallback). Requirements: tx list and receipts per block, batched state
reads at a block hash (multicall / `eth_getStorageAt`), `eth_call`, NodeInterface, `debug_traceBlock*` for diagnostics. Source options in §13
(own Nitro node vs the existing Chainstack provider). The hot path never calls it.

## 4. State and version model

```rust
struct MarketVersion { market: MarketId, base_block: u64, block: u64, tx_index: u32, pga_round: u8,
                       tx_hash: B256, status: Status, confidence: Confidence, state_hash: B256, adapter_version: u16 }
enum Status { Tentative, SoftFinal, Reconciled, L1Posted, L1Finalized }      // Hypothetical never enters the store
enum Confidence { Unknown, Degraded, Tentative, SoftFinal, Reconciled }       // ordered; strategies declare a minimum
```
- **Snapshot publish:** per-market immutable `Arc<MarketState>` behind an atomic pointer (`arc_swap`), written by the single apply thread, read lock-free
  by quoting threads. Phase 3 benchmarks it against copy-on-write of a whole-store map and records the choice.
- **Indexes (O(1)):** `address → MarketId`; `market → [RouteId]`; `token → [MarketId]`; `(tokenA, tokenB) → [MarketId]`; `protocol → [MarketId]`.
- **Block journal:** ring buffer (default 4,096 blocks) of `{ block, hash, txs in order, touched markets, per-tx deltas, resulting versions, reconciliation result }`.
  Memory-resident; a bounded async writer persists it for restart (never on the hot path).
- **Confidence rules:** feed gap, decoder failure, unsupported transition, or an unreconciled correction lowers only the affected markets and the routes through them.
- **Opportunity versions (invariant 3):** `Opportunity { routes: [(MarketId, MarketVersion)], … }`; re-checked against the store before simulation and before submit.

## 5. Crates and dependency graph (layering rule: dependencies point down)

```
L5  mev-engine2 (wiring, modes)         mev-bidding (EV)        mev-reconcile (reconciler, replay, corrections)
L4  mev-quote (routes, quote, size)     mev-submit (= mev-exec: lanes, endpoints)   mev-simulate (= mev-sim + localsim)
L3  mev-adapter (trait + v2/v3/algebra/v4)   mev-feed (fast feed + standard feed clients, dedupe, gap)
L2  mev-market (versioned store, journal, indexes, confidence)
L1  mev-amm (integer math: v2, v3 tick/sqrt, algebra, v4, exact rounding)
L0  mev-types, mev-bindings
```
New: `mev-amm`, `mev-market`, `mev-adapter`, `mev-quote`, `mev-bidding`, `mev-reconcile`, `mev-feed`. Reused (modified): `mev-ingest`/`mev-sequencer` (→ `mev-feed`), `mev-exec`, `mev-sim`, `mev-localsim`, `mev-bindings`.
The legacy `mev-dex`/`mev-state`/`mev-search`/`mev-engine` stay untouched until migration step 6.

## 6. Transition strategy per protocol (spec §5)

| Family | Events → state (hot path) | Modeled **fully** | Partially / not | Fallback |
|---|---|---|---|---|
| V2-style (Uniswap V2, Sushi, Camelot V2 volatile) | `Sync(reserve0, reserve1)` gives post-state exactly | reserves | fee (Camelot per-direction fee: read at bootstrap, refresh on admin events) | node read |
| V3-style (Uniswap V3, Pancake V3) | `Swap(sqrtPriceX96, liquidity, tick)` post-state; `Mint`/`Burn` → tick `liquidityNet/Gross`, bitmap, in-range liquidity | price, tick, in-range liquidity, tick table | protocol fee tier changes (`SetFeeProtocol`), tick data outside the bootstrapped window (**not quotable**, `TickStateIncomplete`) | tick fetch (self-heal) |
| Algebra / Camelot V3 | same `Swap` shape; dynamic fee from `Fee` event | price, tick, liquidity | fee (needs event verification in Phase 4) | node read |
| V4 (hookless) | PoolManager `Swap`/`ModifyLiquidity` by pool id | price, liquidity, ticks | hooked pools: excluded (`UnknownHook`) | node read |
| Later: Curve/Balancer | partial events → exact math from cached balances | per adapter | admin-fee handling | local exec |

Reverted txs (`status = 0`) change nothing and are counted per protocol. For routers/aggregators, **logs are authoritative** for which markets changed (selectors are only hints).
Local execution is used only where logs are insufficient; never against state that lacks earlier txs of the block.

## 7. Simulation, opportunity pipeline, bid, submission

- **Pipeline:** dirty market → dependent routes (coalesced, newer version supersedes) → cheap screen on integer spot → size optimizer (closed form where available, bounded integer search) →
  candidate with versions → `Simulator` (local revm against the snapshot; node `eth_call` for Stylus/precompile-dependent routes) → bid → sign → submit.
  Simulation output is `Hypothetical`, never published.
- **Net profit:** exactly spec §7, in `U256`; gas = L2 + L1 component (`gas_used_for_l1`, NodeInterface estimate cached off-path).
- **Bid engine:** `EV(bid) = P(win|bid)·net(bid) − (1−P(win|bid))·revert_cost(bid)`. Day one a conservative heuristic seeded from the dataset we already have
  (155 races; 24 with winner-tip data: median 0.02 gwei, p90 1.2 gwei, max 3.0), then a fitted model as the new log accumulates (every opportunity, winner tip, outcome). Executor fails first on cheap checks to cut `revert_cost`.
- **Submission:** `Submitter` labels endpoints `primary` (sequencer) / `fallback` (providers) and broadcasts to both while the facts say direct is best-effort (R1);
  one in-flight tx per nonce lane, multiple executor EOAs as lanes; all 12 timestamps of spec §8 per opportunity. Conditional submission stays an opt-in experiment (Phase 12).

## 8. Reconciliation, corrections, replay, rollback

- **Per block (standard feed / node):** freeze journal entry → verify number/hash/parent → compare our tx list with the canonical list (lost Tentative txs, feed gaps) →
  batched reads of every touched market at that block hash → compare normalized `state_hash`es → promote to `Reconciled`, or diff/classify/replace/invalidate dependent routes.
- **Replay for first divergence:** from the last `Reconciled` state through the block's txs, using the same adapters, stopping at the first tx/field that disagrees.
- **Taxonomy:** the spec's reason codes verbatim; `StateCorrection` record per spec §9 (predicted/actual, hashes, diff, reason, severity, first divergent tx/field, adapter/decoder versions, self-heal action).
  Reorg and lost-Tentative rollback are **not** corrections (`Reorg` / roll back to last `SoftFinal` via the journal, replay canonical).
- **Recovery:** Fast Feed has no replay (F3): after a disconnect, affected markets go `Degraded`, state is rebuilt from the node and the journal replayed; Fast Feed continues from head.
- **Self-heal:** missing tick → fetch range; unknown pool → discovery queue; code-hash change → quarantine pool; repeated `MathMismatch` → adapter marked suspect, higher required confidence.

## 9. Metrics (async, droppable)

Per spec §10: feed health/lag per connection, gaps/lost txs, per-stage latency histograms, predictions vs corrections by protocol and reason, unknown contracts/events,
opportunities/sims/submissions per second, rounds targeted vs landed, win/revert rate and cost, bid vs winning bid, gross/net PnL, daily fixed costs (Fast Feed tickets), break-even. Daily adapter quality report.

## 10. Testing and benchmarks (maps to spec §11)

Historical replay per adapter against archive state (needs archive access, §13); differential quotes vs on-chain quoters over thousands of random sizes (we already have the tooling pattern:
`crates/mev-discovery/tests/*_live.rs`); Fast Feed fault injection (drop/duplicate/lose/disconnect/out-of-order across 2 connections); simulation parity vs historical arb receipts; executor fork tests;
shadow soak with would-have-won against actual next-round winners; per-stage p50/p99 under replayed peak load.

## 11. Migration plan (legacy bot keeps running)

1. Phases 2-6 build the new stack in new crates, **shadow only**, side by side with the legacy bot (no shared state, no shared keys).
2. A comparator logs legacy vs new opportunity sets and state agreement per block (cheap, async).
3. Gate at Phase 6 (replay passes) → continue to Phases 7-10 in shadow → gate before any signing path.
4. The new engine runs `EXECUTION_MODE=shadow` (throwaway key) until the human decides otherwise; the legacy live bot is untouched and remains the production path.
5. Only if the new path demonstrates would-have-won rates and PnL in shadow does the human switch; legacy `mev-dex/state/search/engine` are removed afterwards, not before.
6. Liquidations stay on the legacy stack (out of this spec); they can later consume the same market store for DEX routes.

## 12. Risks and unknowns I could not verify (honest list)

- Fast Feed receipt/log JSON keys and whether it really leads the standard feed per tx (needs a ticket).
- PGA activation date (docs say "some time after the vote"); ordering behavior must be re-checked before Phase 7.
- Whether Fast Feed + exact local math beats co-located competitors at all: our evidence is 0/155 wins and a timing loss of ~9 queue places in 42 % of the races with tip data. The design removes known latency sources; it does not prove we can win.
- Integer rewrite scope: every pricing/search/bid path (≈ 140 `f64` sites, audit §2) moves to `U256` in new crates; legacy keeps `f64` until retired (invariant 1 applies to the new engine only).

## 13. Decisions needed from the human at this gate

1. **Fast Feed cost vs budget.** ≥ 17 USDG per ticket-day (F4/F8): ≈ $510/month for one connection, ≈ $1,020 for two (spec wants two). Project budget is $200 capital / ~$0 infra. Options: (a) fund tickets only for **experiments and shadow soaks** (a day at a time, $17 each); (b) fund continuously only once shadow shows it pays; (c) run Phases 2-6 against the standard feed + node only (loses the logs-in-receipt advantage; needs local execution, slower, and doubtful).
2. **Experiment 0 (recommended before the big build):** one ticket (17 USDG), run the existing Fast Feed client in measure-only mode next to the standard feed, hash-match every tx, and record `fast_feed_arrival − standard_feed_arrival` per tx plus receipt/log shape. If the lead is ≳ 100 ms median, the premise holds and the rebuild is justified; if not, we save weeks. The bot cannot buy tickets itself (no funding of wallets): this needs your action.
3. **Node for the correctness path and replay tests.** Spec assumes a local Nitro node. Building one is paused (cost, ~10 TiB). Alternatives: the existing Chainstack plan (async path tolerates latency; archive access and request quota unverified), or a short-lived node for the replay tests only.
4. **Scope of the invariants file.** Stored at `docs/tasks/sequencer-first-invariants.md`; I scoped it to the new engine and listed legacy violations (audit §2). Confirm, or tell me the live bot must meet them too (would mean rewriting it).
5. **Throwaway shadow key and a separate host/port** for the new engine's shadow runs (I will not create production keys, fund wallets, or change `EXECUTION_MODE`).

## 14. Next (after approval)

Phase 2: `mev-feed` (Fast Feed receipt/log decoding, two-connection dedupe, gap/lost-tx detection, fault-injection tests against a mock feed server).
Start with a recorded-message harness so no ticket is needed for CI; use a ticket only for experiment 0 and shadow soaks.
