# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-05
**Running:** SHADOW on Contabo `administrator@194.163.145.241:~/src/mev-bot-v3`, build 28e718e
(deploy-20261005-181023). Moved back 2026-10-05 so the AWS vCPU quota (8) goes to the Nitro node
(`i-0245433f99336c72b`, r7i.2xlarge); AWS bot `i-0acc917f9a4d5f83b` is stopped (quota increase to 16
pending, case 179121121800173). Deploy with
`SERVER=administrator@194.163.145.241 REMOTE_DIR=/home/administrator/src/mev-bot-v3 scripts/deploy.sh shadow`.
Contabo is shared (gmx bots). Latency to the sequencer ~110 ms vs ~ms from Ohio. Not live yet.
**Servers:** full inventory in `config/servers.toml`. Primary RPC = Chainstack $49 plan (`ARBITRUM_WS_URL`/`_HTTP_URL` in each host's `.env`; old primaries moved to `_2`).
**Executor:** `0xb793f66e4e3D1fFfd7c095E3AC2D863BfBB336A2`. Hot wallet ~0.006 ETH.
**Deploy:** start instance (user's call), then `scripts/deploy.sh live --yes` (builds from the working
tree; don't switch branches). The user runs `deploy-executor.sh` in a real terminal.
**RPC:** keyed Chainstack endpoints in `.env` (`ARBITRUM_SWEEP_URL_n`, `_PRESIM_URL_n`, `_WS_FALLBACK_n`).

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

1. Start the instance, deploy main in shadow, compare bid/skip counts, then live.
2. Tune from data: win rate vs tip (`winner_tip_wei` in race reports); round latency (sends take
   ~150–250 ms from trigger: usually over one round).
3. #34 merge → deploy (Aave-funded liquidations).
4. Open: activation date, minimum priority fee and replacement rules aren't in the primary sources.
5. Audit items #54–#57 (win-rate gating, off-loop awaits, shorter in-flight wait, per-venue EV).
