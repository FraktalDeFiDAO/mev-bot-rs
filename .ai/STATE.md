# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot`, build a5768b0 (#31). Deploy:
`scripts/deploy.sh live --yes` builds from the local working tree: don't switch branches while it
runs. The user merges PRs and runs `deploy-executor.sh` in a real terminal (it prompts).
**Executor:** `0x5932C0887173B7E9C4258EaCaB2bAc395137CB29` (flash swaps; no `liquidateViaAave`
yet). Hot wallet ~0.006 ETH.
**RPC:** keyed Chainstack endpoints in `.env` (`ARBITRUM_SWEEP_URL_n`, `ARBITRUM_PRESIM_URL_n`,
`ARBITRUM_WS_FALLBACK_n`); dRPC back in sweep fallbacks. Sweeps OK again since 12:22 UTC.

## User goal

Profitable enough for own RPC nodes (~$800/mo), $200/mo tool plans, and living costs. Status: no
profit yet. Evidence (DECISIONS #51): calm days have ~no liquidations (8, $42 debt in 48 h); crash
days are large (2025-10-10/11: 907 Aave liquidations, ~$17.5 M debt, 22 liquidators; about half
landed 1-4 blocks after the oracle report, so a fast re-check can compete). Arbitrage is contested
by express-lane bots (~200 ms Timeboost head start); reverted routes cool down 30 min.

## Next

1. PR #32 (open): `liquidateViaAave` + profit-funded gas budget. After merge: user deploys the new
   executor + allow-targets (8 lending targets, see DECISIONS #46 era commands) and copies `.env`;
   then set `executor_aave_flash = true` and redeploy.
2. Crash replay (Phase 17): replay 2025-10-10/11 through the bot's sizing/routing to estimate capture.
3. Long-tail arbitrage: widen pool discovery to uncontested pairs.
4. Watch `~/revert-traces.jsonl` on Ohio (diagnostic `trace_reverts.py`) for revert causes.
