# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot` (t3.small, DECISIONS #44), PR #27 build.
Contabo stopped (rollback only). Deploy: `scripts/deploy.sh live --yes` (defaults to
Ohio, builds locally, gate first). The user merges PRs (merging is blocked for the agent); wait for
all 6 CI checks.
**Executor:** `0x5932C0887173B7E9C4258EaCaB2bAc395137CB29` (2026-10-05, has `executeFlashSwap`, 8 lending targets allowed; old `0xc0B4…AF8D` retired). Hot wallet ~0.006 ETH.

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

1. #27 live (deploy-20261005-045515). Next PR `feat/cycle-ranking`: fair cycle cap + flash swaps on.
   Watch the first flash-swap sends (`kind`/revert reasons) after it deploys.
2. `deploy-executor.sh` needs a real terminal (its prompt gets EOF under `!`/no TTY).
3. Phase 11/12: liquidation exits via the best route (≤2 hops today) incl. flash swaps.
4. Phase 17: historical replay; Phase 18: profiling. Morpho/Fluid prediction (shadow books).
