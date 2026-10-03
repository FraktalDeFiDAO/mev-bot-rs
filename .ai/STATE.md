# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-03 (evening)
**Branch:** `feat/liquidations` (stacked on #6 → #5). User merges PRs.
**Executor:** `0xc0B4BA513359979793B9aaa8C74b526aD644AF8D` (Arbitrum) — 8 targets allowlisted.

## Done and verified

- Liquidation architecture: `LiquidationAdapter` trait (`mev-lending`), runner (`mev-liquidate`),
  4 live adapters (Aave v3, Compound III, Morpho, Fluid), 5 status crates (Silo, Radiant, Spark,
  Dolomite, Gearbox) — one crate per venue from `docs/table-1791056656798.csv`.
- Sequencer feed (`mev-sequencer`) drives price-feed-triggered re-checks; forwarder-aware matching.
- Live shadow: all adapters start, 35 feeds watched, ~1 trigger/min, Aave 38 at-risk accounts.
- Tests: unit + property (Rust), contract fuzz/invariant (mocks) + 21 fork tests, E2E on anvil
  (bot trades live on a fork, +0.0103 WETH). See `docs/planning/TESTING.md`. Gate: `scripts/check.sh`.

## Next

1. User restarts the bot: `scripts/run.sh live` (image rebuilt with all of the above).
2. Faster history backfill: free Arbiscan or The Graph key (arb1 rate-limits log scans).
3. Arbitrage: Uniswap v4 support; gas reduction (inventory mode, packed calldata).
4. Promote Dolomite/Gearbox after measuring 30-day flow; Silo when Arbitrum v2 markets carry debt.
