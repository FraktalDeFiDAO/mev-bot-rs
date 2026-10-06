# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot`, build 9d952c0 (main through #33). It
still sends **zero-tip** transactions: PGA bidding lives on branch
`claude/arbitrum-timeboost-successor-cihu0f` (not merged, no PR yet, nothing deployed from it).
**Built, not enabled:** batching (`ArbExecutor.batch`, `[batch]`, needs executor redeploy + `executor_batch = true`) and the Atlas solver (`[atlas]`, runbook `docs/research/ATLAS.md`, observing since 2026-10-06).
**Executor:** `0xb793f66e4e3D1fFfd7c095E3AC2D863BfBB336A2` (flash swaps + `liquidateViaAave`; 8
lending targets). PR #34 (enable `executor_aave_flash`) open. Hot wallet ~0.006 ETH.
**Deploy:** `scripts/deploy.sh live --yes` builds from the working tree: don't switch branches while
it runs. The user merges PRs and runs `deploy-executor.sh` in a real terminal (it prompts).
**RPC:** keyed Chainstack endpoints in `.env` (`ARBITRUM_SWEEP_URL_n`, `_PRESIM_URL_n`,
`_WS_FALLBACK_n`).

## Ordering: Priority Gas Auction (DECISIONS #59)

Arbitrum One orders each 125 ms round (2 per 250 ms block) by effective priority fee, ties by
arrival; Timeboost is gone. Verified on-chain (tip ordering 3.4:1 end of September vs 0.9:1 mid
September). Our 40 reverted arbs tipped 0; txs ahead of them tipped 0.001–0.26 gwei. The branch adds:
- `mev_risk::bid`: `BidPolicy` (≤ 30 % of net, ≤ $0.25/tx, never net < 0), `TipBook` (competition
  from race-report winner tips), `TipBudget` ($0.50/min, $2/day).
- `mev-exec`: `Fees` on every tx (explicit `maxPriorityFeePerGas`, `maxFee ≥ tip`).
- `mev-engine`: bid at submission for arbs, backruns, simulated liquidations; on-chain `minProfit`
  includes the tip; metrics (bids placed/skipped/won/lost, tip $, sends within/over one round).
- `mev_ingest::fast_feed`: paid feed client (off; `ARBITRUM_FAST_FEED_KEY`), merged and deduped.
- `mev_sim::pga` + `pga_replay` test; `mev-pnl` (net PnL per strategy after tips).

## User goal

Profit enough for own RPC nodes (~$800/mo), $200/mo tools, living costs. Status: no profit yet.
Calm days have ~no liquidations; crash days are large (DECISIONS #51).

## Next

1. User: review the PGA branch; PR when asked. Then shadow-run it (default mode stays shadow),
   compare bid/skip counts, then live.
2. Tune from data: win rate vs tip (`winner_tip_wei` in race reports); round latency (sends take
   ~150–250 ms from trigger: usually over one round).
3. #34 merge → deploy (Aave-funded liquidations).
4. Open: activation date, minimum priority fee and replacement rules aren't in the primary sources.
5. Audit items #54–#57 (win-rate gating, off-loop awaits, shorter in-flight wait, per-venue EV).
