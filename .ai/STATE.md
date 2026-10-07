# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-07
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot` (sole host; Contabo stopped, rollback only),
build from `main` (see `git log`; deploy tags `deploy-YYYYMMDD-HHMMSS`). Hot wallet ~0.005 ETH (low).
**Executor:** `0x17357E33012e8c05E654a7D1CC7D9dedd6B63acf` (redeployed 2026-10-06: V4 hops, `batch`,
`liquidateViaAave`, flash swaps). `executor_v4 = true`, `executor_batch = true`; `[batch] enabled = false`.
**On:** PGA bidding + per-route tip books, win-rate gate (#54), sim back-off for failed routes (120 s),
V4 (79 hookless pools, `presimulate = true`, `ModifyLiquidity` applied), tick-aware V3/Pancake/V4
pricing (`[ticks]`, window ±800, reload 300 s), triangles (~62k cycles), V4 race detection.
**Observe-only:** Atlas solver (`[atlas] enabled, bid=false`; needs ~0.1 ETH bond, `docs/research/ATLAS.md`).
**Deploy:** `scripts/deploy.sh live --yes` builds from the working tree (stash `docs/knowledge/` first;
run in background, it takes 10-15 min). The user merges PRs and runs `deploy-executor.sh` himself.
**Ops:** `scripts/report.sh` (PnL, AWS spend, health), `scripts/watchdog.sh` (on the host: container,
heartbeat, kill switch, wallet gas, cap events; optional webhook file `data/arbitrum/alert_webhook`),
`scripts/test-e2e-crash.sh` (N borrowers liquidated in one crash, batching on).
**RPC:** keyed Chainstack endpoints in `.env`; arb1 public RPC rate-limits the dev IP (Ohio is fine).

## Facts that decide what to build

- Results so far: ~180 landed sends, 0 wins, ~$4–5 gas+tips; net PnL negative (see `report.sh`).
  Arbitrum orders each 125 ms round by priority fee; we land in the winners' round and lose on tips.
- Real arb edges are $0.03–$0.8 and contested; the thin-pool "edges" were phantoms (DECISIONS #73).
- Calm days have ~no liquidations reachable without the Atlas auction; crash days are the upside.
- AWS: usage ≈ credits (net $0); Chainstack $49/mo is the real fixed cost. The stopped Nitro node's EBS
  volumes live in another account (not visible from profile `mev-bot`): user to confirm they're deleted.

## Open (needs the user)

1. Atlas bond (~0.1 ETH) to bid on auctions; wallet top-up (watchdog alerts below 0.003 ETH).
2. `[batch] enabled`: run `scripts/test-e2e-crash.sh` first, then decide.
3. Fast Feed key; a second host only once there is profit to protect.
4. Stopped-node disks and the `docs/knowledge/` untracked churn.

## Next (engineering)

Watch for the first profitable landed arb or a V4 race with a known winner (`race` events);
tune per-route bids from `route_tips`; consider tick-aware Algebra pools; liquidation census over more lenders.
