# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.
Durable facts and next actions only.

---

**Updated:** 2026-10-03
**Branch:** `feat/mvp-live-arb` — draft PR #5. User merges PRs.
**Goal:** live arbitrage trades on Arbitrum today; repo as reusable building-block crates.

## Done and verified

- `contracts/ArbExecutor.sol`: Balancer V2 flash loan → Uni/Sushi V2, Camelot V2, Uni/Sushi V3 hops.
  7 fork tests pass on live Arbitrum state (incl. capturing a real arb, holding no funds).
- 18 crates, clippy `-D warnings` clean, unit tests pass (dex 3, risk 4, search 2).
- Shadow run vs live Arbitrum (PublicNode): 186 pools discovered, 55 pass $20k liquidity,
  WS events stream. Balancer vault is thin post-2025 exploit: USDC loan cap ~$5k, ARB ~$4k.
- Only 8 pool updates/min via PublicNode WS — expect more with Alchemy/dRPC.

## Diagnosis 2026-10-03 (see DECISIONS #19–21)

- Crash loop on DNS outage fixed (WS reconnect + backoff). Image rebuilt; user restarts with `scripts/run.sh live`.
- Zero trades = coverage: watched majors are efficient; volume is on Uniswap v4, PancakeSwap v3,
  Camelot v3 (Algebra), and long-tail tokens. Tool: `target/release/mev-analyze` (use a getLogs-capable
  RPC, e.g. ARBITRUM_HTTP_URL=https://arb1.arbitrum.io/rpc; Alchemy free caps getLogs at 10 blocks).

## Next

1. User fills `.env` (ARBITRUM_HTTP_URL, ARBITRUM_WS_URL, PRIVATE_KEY of fresh hot wallet, PROFIT_RECEIVER).
2. User runs `scripts/deploy-executor.sh` (re-runs fork tests, asks to confirm, writes EXECUTOR_ADDRESS).
3. `cargo run --release -p mev-bot -- --mode shadow` (now with simulation), then `--mode live`.
4. Majors are efficient → add long-tail pairs/pools; then liquidations (Phase 2 plan).

## Known limits

- V3 modeled within current tick only (sim is source of truth). No triangular routes yet.
- Profit in metrics uses search estimate, not on-chain transfer amount.
