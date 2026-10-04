# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.

---

**Updated:** 2026-10-04 (evening)
**Running:** LIVE on AWS us-east-2 `mev-ohio:~/src/mev-bot` (t3.small, DECISIONS #44) since 2026-10-04
22:57 UTC; Contabo stopped (rollback only). Deploy: `scripts/deploy.sh live --yes` (defaults to Ohio,
builds locally). Never run a second live bot.
**Executor:** `0xc0B4BA513359979793B9aaa8C74b526aD644AF8D`. Hot wallet ~0.007 ETH.

## Open PRs (user merges; squash-only)

| PR | What | Base |
|---|---|---|
| #9 | sequencer feed: reconnect when silent (bot went blind 1 h) | main |
| #8 | `mev-liq-audit` missed-liquidation audit (DECISIONS #40) | main |
| #13 | redact RPC API keys from logs/events | main |
| #10 | Aave oracle backrun (DECISIONS #41) | main |
| #11 | live arbs send on the model; gas price never blocks (5 ms) | #10 |
| #12 | feed-driven arbitrage via `eth_simulateV1` (`mev-presim`, DECISIONS #43) | #11 |

Merge order: #9, #8, #13 → rebase #10 onto main (DECISIONS.md conflict) → merge → rebase #11, then
#12 (`git rebase --onto main <old base>`) → merge each → `scripts/deploy.sh live --yes`.

## Findings that drive the plan

- Contested Aave liquidations land in the same block as the oracle update (#40) → backrun (#10).
- All live arbs reverted `NotProfitable`: lost by ~1 block (#42) → 5 ms send path (#11) +
  feed pre-simulation (#12; PublicNode p50 37 ms from the server; ~2/3 of pool updates early).

## Next

1. After deploy: watch backrun/arb outcomes (`landed` events), re-run `mev-liq-audit` weekly and
   after volatile days.
2. Extend `predict` to Compound/Morpho once Aave backruns are measured.
3. Uniswap v4 pools; optimal arb sizing / 3-hop routes.
4. Free The Graph/Arbiscan key to finish the Aave index backfill (only to block 391M).
5. Rotate Alchemy/Chainstack keys after #13 is deployed (old logs contain them).

## Ops notes

- Tests run in the dev container (`scripts/dev.sh`, now with python for the E2E scripts).
- Host `target/` is owned by the dev container's volume mapping; build on the host fails.
- Server watch: SSH + `podman logs -f`; awk/grep must flush per line.
