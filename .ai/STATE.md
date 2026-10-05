# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot` (t3.small, DECISIONS #44), still on the
pre-#27 build. Contabo stopped (rollback only). Deploy: `scripts/deploy.sh live --yes` (defaults to
Ohio, builds locally, gate first). The user merges PRs (merging is blocked for the agent); wait for
all 6 CI checks.
**Executor:** `0xc0B4BA513359979793B9aaa8C74b526aD644AF8D` (no `executeFlashSwap`). Hot wallet ~0.007 ETH.

## Where we are

PR #27 (`feat/localsim`, CI green, awaiting the user's merge), simulation-engine phases 2, 5–8, 13, 15:
- **Live once deployed:** triangular arbs (`mev_search::cycles`); wrapped price feeds (SVR/CAPO)
  resolved via access lists; aggregator answers by direct `eth_call`; Compound backrun prediction.
- **Off (flags):** `presim.local` (REVM worker: exact where complete, ~7 % live coverage on free RPC,
  DECISIONS #45); `executor_flash_swaps` (needs the new executor deployed, #46).
- Findings: `docs/planning/SIM_ENGINE.md`; decisions #45–47.

RPC budget lesson: the host IP shares free limits with the live bot. Probes must not use arb1
(presim primary); never poll at block rate.

## Next

1. User merges #27 → `scripts/deploy.sh live --yes`; watch for `prediction snapshot` warnings
   (the Aave live test couldn't run: public RPCs rate-limit its 53k-account sweep).
2. Deploy the new executor (`scripts/deploy-executor.sh`, needs the user's OK: on-chain spend),
   `setTarget` the lending targets again, update `EXECUTOR_ADDRESS`, set `executor_flash_swaps = true`.
3. Phase 11/12: liquidation exits via the best route (≤2 hops today) incl. flash swaps.
4. Phase 17: historical replay; Phase 18: profiling. Morpho/Fluid prediction (shadow books).
