# Task: Sequencer-first, block-reconciled MEV engine for Arbitrum One

Place this file at `docs/tasks/sequencer-first-engine.md`. `CLAUDE.md` holds the
standing invariants (stored in this repo at `docs/tasks/sequencer-first-invariants.md`, linked from
`CLAUDE.md`); this file holds the build spec.

---

## 0. How to work on this task

This is a multi-week build. Do not try to finish it in one session.

1. Work phase by phase (section 12). Each phase has a Definition of Done (DoD).
2. **Gates:** stop and report to the human after Phase 1 (audit + design doc),
   after Phase 6 (state engine passes historical replay), and before Phase 10
   (anything that builds or signs transactions). Do not proceed past a gate
   without approval. Between gates, proceed autonomously.
3. After every phase, update `docs/PROGRESS.md`. If a session ends mid-phase, the
   next session must be able to resume from that file alone.
4. If a requirement here conflicts with verified current Arbitrum behavior, the
   verified behavior wins. Record the conflict in `docs/PROGRESS.md` and adapt
   the design; do not build against a wrong assumption.
5. If you find yourself unable to meet a DoD, say so plainly with the evidence.
   Partial-but-honest beats complete-looking.

---

## 1. Arbitrum One ground truth (verified 2026-10; re-verify before building)

Fetch and read the current versions of: PGA intro, Fast Feed intro, Use the Fast
Feed, Read the sequencer feed, RPC methods (Arbitrum vs Ethereum), and NodeInterface
/ precompile references, all under `docs.arbitrum.io`. Record findings in
`docs/arbitrum-facts.md`. As of writing:

**Ordering (PGA).** Blocks are nominally 250ms with two PGA rounds of ~125ms
each. Transactions that reach the Sequencer during a round wait for the next
round; at round start they are ordered by
`final_priority = effective_priority_fee_per_gas + accumulated_boost`, where
`effective_priority_fee_per_gas = min(maxPriorityFeePerGas, maxFeePerGas − baseFee)`.
Boost only applies to transactions that have waited more than one round. Ties
break by arrival time measured at the Sequencer. A round in progress cannot be
interrupted.

Consequences you must design around:
- An opportunity revealed in round R can only be contested in round R+1 or later.
  The latency budget is: Fast Feed message receipt → our tx *arriving at the
  Sequencer* before the next round starts.
- Priority is per unit gas, but you pay it on every unit of gas used. A lower-gas
  executor contract can outbid a competitor at lower total cost. Gas efficiency
  of the on-chain executor is a first-class competitive lever.
- `maxFeePerGas` must leave headroom above base fee or the effective priority is
  silently capped.

**Fast Feed.** Paid, authenticated WebSocket (ticketed per 24h round, paid in USDG,
one connection per ticket). Publishes each transaction *after the Sequencer has
ordered and executed it*, before the block reaches the standard feed. Each
message includes `timestamp_ms`, `pga_round`, a tentative `block_number`,
`tx_index`, `raw_tx`, `tx_hash`, and a partial receipt with `status`, `gas_used`,
`gas_used_for_l1`, `effective_gas_price`, `base_fee`, and **event logs**. It does
not publish state diffs, has no replay or history, and messages can be lost if
the Sequencer fails mid-block. A Nitro node cannot follow the chain from it.

Consequences:
- Because logs and revert status are included, **most state transitions can be
  derived from logs without local execution**. Local execution is a fallback, not
  the default (see section 5).
- Gap detection on the Fast Feed is by `(block_number, tx_index)` contiguity, not
  by a sequence number. Cross-check every block's tx count against the standard
  feed.
- On disconnect, everything missed is gone. Recovery comes from the standard
  feed / local node, and affected markets drop to `Degraded` until reconciled.
- For redundancy, budget for at least two tickets/connections, dedupe by
  `tx_hash`, and record per-connection arrival latency.
- Ticket cost is a fixed daily operating cost. Include it in PnL and in the
  break-even report.

**Standard sequencer feed.** Free, per-block `BroadcastFeedMessage` with
`sequenceNumber`; L2 block = sequence number + 22207817 on Arbitrum One (verify).
This is the soft-finality source. Run a local feed relay and a local Nitro node
that follows it.

**Conditional submission.** Arbitrum supports `eth_sendRawTransactionConditional`
with `knownAccounts` (storage root or slot → value) and block/timestamp bounds.
Arbitrum's implementation checks conditions against recent past state (roughly
the prior 1–2 seconds), and the endpoint is rate-limited. That makes it a weak
guarantee at 125ms granularity. Treat it as an optional, measured experiment,
never as the primary stale-execution guard. The primary guard is the on-chain
executor's `minProfit` check plus fail-fast reverts.

**Arbitrum EVM differences that break naive simulation.**
- `block.number` in Solidity returns an approximate L1 block number; L2 block
  number comes from `ArbSys.arbBlockNumber()` (precompile `0x64`).
- Arbitrum precompiles (ArbSys, ArbGasInfo, NodeInterface at `0xc8`, etc.) do not
  exist in stock REVM. Implement the ones touched by your routes or route those
  calls to the local Nitro node.
- Stylus (WASM) contracts cannot be executed by REVM. Detect them by code prefix
  (verify the exact prefix) and mark any market that depends on one as
  "node-simulation only."
- Gas used includes an L1 data component (`gas_used_for_l1`). Total cost is
  `effective_gas_price × gas_used`. Estimate the L1 component with NodeInterface
  `gasEstimateL1Component`, cached and refreshed off the hot path.
- Reverted transactions are still included and still pay for gas consumed.

---

## 2. Objective and success condition

Build a state machine where:

1. A transaction is ordered and executed by the Sequencer.
2. We receive it on the Fast Feed (fallback: standard feed).
3. We derive the resulting market state deltas in exact Sequencer order and
   atomically publish new `Tentative` market snapshots.
4. Only routes depending on changed markets are re-quoted, locally, in integer math.
5. Profitable candidates are size-optimized, simulated, bid, signed, and sent
   directly to the Sequencer in time for the next PGA round.
6. When the block appears on the standard feed / local node, every touched market
   is reconciled against node state. Mismatches are corrected, classified,
   recorded, and the first divergent transaction is identified.
7. At any moment the system can answer, for any market: which block and tx index
   its state reflects, its status, when it was last reconciled, the cause of its
   last correction, and which opportunities used it.

Latency and correctness are both required. The hot path delivers speed; the
correctness path delivers truth. Neither may degrade the other.

---

## 3. Architecture

Two explicit pipelines, sharing only the versioned market-state store.

**Hot path** (single-digit-millisecond target, measured not guessed):
Fast Feed client → decoder/classifier → log-driven state transition (fallback:
local execution) → atomic snapshot publish → dirty-route scheduler → integer
quote engine → size optimizer → simulation → bid → sign → direct Sequencer submit.

**Correctness path** (asynchronous, can lag by a few blocks without harm):
standard feed / local node block → touched-market set → batched state reads at
that block hash → normalized-hash compare → structural diff on mismatch →
classification → correction + route invalidation → replay to find first
divergence → persistence + metrics.

Components (one responsibility each; names are suggestions):
`FastFeedClient`, `SequencerFeedClient`, `NodeStateClient`, `TxClassifier`,
`ProtocolAdapter` (one per AMM family), `MarketStateStore`, `RouteIndex`,
`DirtyScheduler`, `QuoteEngine`, `SizeOptimizer`, `Simulator`, `BidEngine`,
`NonceManager`, `Submitter`, `BlockJournal`, `Reconciler`, `ReplayEngine`,
`CorrectionStore`, `Metrics`.

Strategy code never touches WebSocket parsing, RPC clients, or persistence directly.

---

## 4. State model

**Versioning.** Every market snapshot carries:
`market_id, base_block, block_number, tx_index, pga_round, tx_hash, status,
confidence, state_hash, adapter_version`.

**Status** uses the vocabulary in `CLAUDE.md` (`Tentative`, `SoftFinal`,
`Reconciled`, `L1Posted`, `L1Finalized`, `Hypothetical`). Never conflate them.

**Confidence** per market: `Unknown`, `Degraded`, `Tentative`, `SoftFinal`,
`Reconciled`. Each strategy declares the minimum confidence it accepts. A feed
gap, decoder failure, unsupported transition, or unreconciled correction lowers
confidence only for the affected markets and routes, not globally.

**Snapshots.** Readers never see partial mutation. Use copy-on-write or a
compact per-market representation with atomic pointer swap. Benchmark both and
record the choice.

**Indexes.** O(1) lookup from pool address → market. Maintain market → routes,
token → markets, pair → markets, protocol → markets.

**Block journal.** Rolling window (configurable, default a few thousand blocks)
of: tx list in order, touched markets, per-tx deltas, resulting versions,
reconciliation result. Required for replay, rollback, and first-divergence.

**Tentative rollback.** If a Fast Feed transaction never appears in the produced
block (Sequencer failure mid-block), roll back to the last `SoftFinal` snapshot
for affected markets, replay the actual block, and invalidate dependent
opportunities. This is the common "reorg" case on Arbitrum; deeper L2 reorgs are
rare but must still be handled via the journal (find common ancestor, roll back,
replay canonical, reconcile, record as `Reorg`, never as a normal correction).

---

## 5. State transitions

Order of preference for deriving a market's post-transaction state:

1. **Logs from the Fast Feed receipt** where the protocol's events fully
   determine post-state. Examples to verify per adapter: Uniswap V2-style `Sync`
   (reserves); Uniswap V3-style `Swap` (post `sqrtPriceX96`, `tick`, in-range
   `liquidity`) plus `Mint`/`Burn` for tick `liquidityNet`/`liquidityGross` and
   bitmap; Algebra/Camelot fee-change events for dynamic fees.
2. **Deterministic math from logs + cached state** where events are partial
   (e.g. Curve-style `TokenExchange` lacks post-balances; compute them exactly
   from the pool's math and cached balances, including admin-fee handling).
3. **Local execution** (REVM against the current snapshot, in exact Sequencer
   order) when logs and math are insufficient or the adapter flags uncertainty.
   Never execute tx N against state that does not include txs < N.
4. **Node fallback** (local Nitro `eth_call`/state read) for Stylus contracts or
   unsupported precompiles. Mark these markets as slower-confidence.

Reverted transactions (`status = 0`) change no market state; skip them, but count
them per protocol (useful competition signal).

Each adapter declares, per event and per state field, whether it models that
transition **fully**, **partially**, or **not at all**. Only fully modeled fields
may be held to the 100% replay-agreement requirement; partially modeled fields
force a node read before any submission that depends on them.

**Classification and discovery.** Combine destination address, code hash,
factory membership, router registry, selector hints, and emitted logs. Selectors
are hints, never identity. For aggregators and routers (Universal Router, 1inch,
0x, Odos, Paraswap, multicalls), the emitted pool logs, not the outer calldata, are
authoritative for which pools changed. Unknown contracts that emit known pool
events, or pools from known factories not yet in the registry, are queued for
discovery off the hot path; the hot path never blocks on discovery.

---

## 6. Pricing and opportunity search

- Quote exact-in and exact-out from cached state with each protocol's exact
  integer rounding. Spot price is for screening only; decisions use
  amount-specific executable output including fees and price impact.
- Concentrated liquidity quotes must walk initialized ticks and update liquidity
  on every crossing. A pool whose tick data is incomplete in the required range is
  not quotable.
- Pipeline: dirty market → dependent routes (coalesced; newer version supersedes
  queued older work) → cheap screen → size optimization (closed form where the
  AMM allows, bounded numerical search otherwise) → candidate.
- Route types: 2-pool cyclic, triangular, bounded multi-hop, flash-swap funded,
  flash-loan funded. Never hard-code one AMM family into the scanner.
- **Differential test** every adapter's quote against the on-chain quoter or
  `eth_call` at the same block across thousands of random sizes. Any mismatch in a
  fully modeled path fails CI.

---

## 7. Simulation, profit, and bidding

**Simulation** runs only on candidates that pass screening, against the latest
snapshot, after verifying every dependency version is still current. It must
produce: success/revert, exact output, gas used (L2 + L1 component), and touched
storage. Simulation output is `Hypothetical` and never enters the shared cache.

**Net profit:**
```
net = gross_out − amount_in − flash_fee
      − (base_fee + priority_fee) × gas_used_total
      − safety_margin
```
Ticket cost and infrastructure cost are tracked as daily fixed costs, not per trade.

**Bid engine.** Model the round as a competition, not a fixed formula:
```
EV(bid) = P(win | bid) × net(bid) − (1 − P(win | bid)) × revert_cost(bid)
```
`revert_cost` is the gas paid when we lose and revert, so the executor must fail
as early and cheaply as possible (check pool state and `minProfit` before doing
expensive work). Choose the bid that maximizes EV, capped so that `net > 0` and
`EV > min_required_ev`. Estimate `P(win | bid)` from history: record, for every
opportunity we saw, the winning tx's priority fee (from the Fast Feed and
receipts), whether we competed, and the outcome. Start with a conservative
heuristic and replace it with the fitted model once enough data exists. Persist
all bid decisions for offline analysis.

**On-chain executor contract** (Phase 10, behind the gate): owner-restricted,
`minProfit` enforced on-chain, no token approvals left dangling, fail-fast
ordering of checks, gas-golfed hot path. Get it reviewed before any live use.

---

## 8. Submission

- `Submitter` sends directly to the Arbitrum One Sequencer endpoint (verify the
  current URL). Public/provider RPCs are an explicit, labeled fallback only.
- Measure RTT to the Sequencer from the deployment host and report it; arrival
  time at the Sequencer decides ties and round eligibility.
- `NonceManager` handles concurrent candidates: one in-flight tx per nonce lane,
  explicit replacement/cancellation policy, and recovery from unknown outcomes.
  Consider multiple executor EOAs as independent lanes.
- Conditional submission is an opt-in per strategy, measured against plain
  submission (acceptance rate, latency, revert rate). Keep it only if it wins.
- Track per opportunity: `fast_feed_received, decoded, state_published,
  quoted, simulated, signed, submit_started, submit_acked, landed_block,
  landed_tx_index, landed_pga_round, outcome`. Report the round we targeted vs
  the round we landed in; "missed round" is a key metric.

---

## 9. Reconciliation and correction (correctness path)

On each block from the standard feed / local node:

1. Freeze the journal entry for that block.
2. Verify block identity (number + hash; parent linkage).
3. Verify our tx list for the block matches the canonical tx list (detects lost
   Tentative txs and Fast Feed gaps).
4. For every touched market, read authoritative state at that block hash from
   the local node, batched (multicall / batched storage reads).
5. Compare normalized state hashes (exclude volatile metadata). On match: promote
   to `Reconciled`, record success.
6. On mismatch: structural diff → classify → replace with node state →
   invalidate dependent routes and pending opportunities → replay from the last
   reconciled state through the block's txs to find the **first divergent
   transaction and field** → persist a `StateCorrection` → self-heal action.

Three depths: every block (touched markets, cheap fields), on mismatch (storage,
logs, full protocol state, trace if needed), periodic audit (random sample of
active and inactive markets, full state).

**Correction record** fields: timestamp, block number/hash, tx index, tx hash,
market, protocol, pool, predicted vs actual state and hashes, diff, reason,
severity, first divergent tx/field, adapter and decoder versions, self-heal
action taken, resolution.

**Reason codes:** `Unknown, FastFeedGap, FastFeedTxLost, SequencerFeedGap,
OutOfOrder, UnsupportedEvent, UnsupportedRouter, UnknownInternalCall,
MissingLog, DecoderBug, MathMismatch, TickStateIncomplete, TokenBehavior
(fee-on-transfer/rebasing), DynamicFee, ProtocolUpgrade, UnknownHook,
StylusContract, PrecompileDependency, Reorg, RaceCondition, IncompleteBootstrap,
ExternalStateDependency`.

**Self-heal mapping** (examples): missing tick → fetch tick range; unknown pool →
discovery; protocol upgrade (code hash changed) → quarantine adapter for that
pool; feed gap → recover from node and replay; repeated `MathMismatch` on a
protocol → mark that adapter suspect and raise its required confidence.

Alert when corrections per block or per protocol exceed thresholds. Corrections
are a safety net; a steady rate means a model bug to fix.

---

## 10. Observability and persistence

**Metrics:** feed health and lag (both feeds, per connection), Fast Feed gaps and
lost txs, per-stage latency histograms (section 8 timestamps), predictions vs
corrections globally and per protocol, corrections by reason, unknown
contracts/events, opportunities/sec, simulations/sec, submissions/sec, rounds
targeted vs landed, win rate, revert rate and revert cost, bid vs winning bid,
gross and net PnL, daily fixed costs, break-even.

**Persistence** (never on the hot path): tokens, markets, factories, routers,
code fingerprints, adapter metadata, discovery records, correction records, bid
decisions and outcomes. Persistence or metrics outages must not stop the hot
path; anything that compromises state correctness must lower confidence.

**Adapter quality report**, generated daily: per protocol, predictions,
corrections, accuracy, top reasons.

---

## 11. Testing requirements

- **Historical replay (required per adapter):** bootstrap state at block N−1 from
  an archive node, replay block N's txs through the state engine using receipts,
  compare every touched market against archive state at N. Run over large active
  ranges. 100% agreement on fully modeled fields, or the test fails.
- **Differential quote tests** against on-chain quoters (section 6).
- **Fast Feed fault injection:** dropped messages, duplicate messages, lost
  Tentative txs, disconnect mid-block, out-of-order delivery across two
  connections. Verify confidence drops, recovery, and correct rollback.
- **Simulation parity:** for a sample of historical arbitrage txs (ours or
  others'), simulated output and gas must match the receipt.
- **Executor contract tests** on a fork: profit path, `minProfit` revert, fail-fast
  revert gas cost, access control.
- **Shadow mode soak:** run live against real feeds in `shadow` for an extended
  period; report would-have-won rate by comparing our hypothetical tx against the
  actual next-round winners.
- **Benchmarks:** per-stage latency p50/p99 under replayed peak load.

---

## 12. Phases and Definitions of Done

| # | Phase | Definition of Done |
|---|---|---|
| 1 | Audit + design doc (section 13) | Report and design doc written; arbitrum-facts.md populated with sources. **GATE.** |
| 2 | Feed clients + local relay/node wiring | Both feeds ingest; dedupe across 2 Fast Feed connections; gap and lost-tx detection proven by fault-injection tests. |
| 3 | Versioned state store, journal, confidence | Atomic snapshots under concurrent read benchmark; journal supports rollback and replay in tests. |
| 4 | Adapters: log-driven transitions (start with V2-style and V3-style) | Historical replay passes 100% on fully modeled fields over a large block range. |
| 5 | Local-execution fallback + Arbitrum precompile/Stylus handling | Replay passes for txs routed to fallback; Stylus markets correctly flagged. |
| 6 | Reconciler, correction records, first-divergence replay | Injected model bugs are detected, classified, and pinned to the correct first tx. **GATE.** |
| 7 | Route index, dirty scheduler, quote engine | Differential quote tests pass; coalescing proven; no global scans in profiler. |
| 8 | Size optimizer + candidate generation | Optimizer matches brute-force optimum within 1 wei-unit tolerance on test pools. |
| 9 | Simulator for candidates | Simulation parity tests pass, including L1 gas component. |
| 10 | Executor contract, signer, nonce manager, submitter (shadow) | Fork tests pass; shadow mode produces signed-but-unsent txs with full timestamps. **GATE before any live mode.** |
| 11 | Bid engine + outcome logging | Heuristic bidder in place; winning-bid dataset accumulating; EV logged per decision. |
| 12 | Conditional submission experiment | Measured comparison report; kept or dropped on evidence. |
| 13 | Persistence, metrics, dashboard, adapter quality report | All section 10 metrics visible; hot path unaffected by DB outage test. |
| 14 | Benchmark, profile, remediate | Per-stage p50/p99 reported; top three bottlenecks fixed or documented. |

Add further protocol adapters (Camelot/Algebra, Curve, Balancer, etc.) by
repeating Phase 4's DoD for each.

---

## 13. Phase 1 deliverables

**Audit report** on the existing repo. For each item, cite file paths and give a
one-line verdict (keep / modify / replace): architecture, block and event
ingestion, RPC usage, any sequencer feed usage, simulation/REVM usage, market
cache, state versioning, route discovery, execution and submission path,
gas/priority logic, reconciliation, reorg handling.

Explicitly list, with file:line where possible: remote RPC in the hot path, full
market scans, DB access in the hot path, data races, state that cannot be
reconstructed, missing ordering guarantees, likely drift sources, unsupported
transitions, float math in financial paths, stale-opportunity windows, wasted
simulations, submission latency sources.

**Design doc** (`docs/design/sequencer-first.md`): current vs proposed data flow,
feed ingestion, node integration, state and version model, store and indexes,
dependency graph, transition strategy per protocol, simulation design, opportunity
pipeline, bid engine, submission, reconciliation, correction taxonomy, replay,
rollback/reorg, recovery, metrics, benchmarks, and a migration plan that keeps
the existing bot runnable until the new path passes its gates.

Keep both documents concrete and short enough to review in one sitting. Then stop
at the gate.

---

## 14. Do not

- Use block polling or remote RPC as the primary market-data path.
- Treat selectors or outer calldata as authoritative for which pools changed.
- Locally execute a transaction whose effect is already fully determined by its logs.
- Simulate every route; only screened candidates reach the simulator.
- Bid more than the opportunity's EV supports, or ignore revert cost.
- Treat a Fast Feed message as a stored block, or a soft-final block as L1-final.
- Patch a reorg or lost-Tentative rollback as an ordinary correction.
- Submit through generic public RPC except as a labeled fallback.
- Change `EXECUTION_MODE`, touch production keys, or deploy contracts to mainnet.
