# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot` (t3.small, DECISIONS #44). Contabo stopped
(rollback only). Deploy: `scripts/deploy.sh live --yes` (defaults to Ohio, builds locally, gate first).
Merge PRs only after all 6 CI checks report and pass.
**Executor:** `0xc0B4BA513359979793B9aaa8C74b526aD644AF8D`. Hot wallet ~0.007 ETH.

## Where we are

Simulation-engine program (`docs/planning/SIM_ENGINE.md`): Phase 0 audit done; **Phase 2 in progress**
on branch `feat/localsim` (not merged, not live):
- `mev-localsim`: revm 43 over a state cache fed by **dRPC per-block prestate diffs**
  (`debug_traceBlockByNumber`, diffMode; 0 rate limits, p50 ~170 ms, lag 0–1 block).
- Accuracy (probe on Ohio): with complete state and truth on the same block, **124/124 real
  pool-moving swaps exact**, local sim p50 0.4 ms. First sight vs latest: 2/12 — the gap is the 0–1
  block diff lag. ~2% false movers (unclassified).

Race telemetry (live): we land in the same feed batch as winners, 5–7 positions behind; trigger→send
18 ms (best, presim) to ~120 ms (block logs); free-RPC eth_simulateV1 averages ~70 ms.

## Next

1. Phase 2 step 2: speculative overlay (simulate every feed tx locally in sequencer order on top of
   the last applied block; reset when its diff lands) + pre-warm watched pools/routers; re-measure
   first-sight accuracy vs latest (target ≥95 %).
2. Classify the ~2% false movers (timestamps, Arbitrum precompiles).
3. Phase 3: persistent revm workers; then wire local sim into the arb path (presim as fallback).
4. Liquidations: extend oracle backrun to Compound/Morpho.
