# Decision Log

Newest last. Each entry: decision, reason, and what would change it.

## 2026-10-03 — Initial planning

| # | Decision | Reason | Revisit when |
|---|---|---|---|
| 1 | Strategies: **arbitrage and liquidations only**, no sandwiching | Sandwiching harms users and is mostly impossible on L2s without a public mempool | — |
| 2 | **Measure before building** (Phase 0 analytics + go/no-go gates) | Most MEV bots lose money by competing in saturated cells | — |
| 3 | Primary strategy: **liquidations on long-tail lending protocols**; secondary: long-tail DEX arb | Few competitors, 5–15% bonuses, least latency-sensitive (we start without a node) | Phase 0 data says otherwise |
| 4 | Avoid blue-chip DEX arb (WETH/USDC etc.) | Saturated by professional low-latency teams | We own colocated infra and data shows an edge |
| 5 | Starting chain: **Arbitrum** (provisional) | Free, unmetered sequencer feed = fastest data without a node | Phase 0 ranking |
| 6 | Language: **Rust** off-chain; Solidity (Foundry) for the executor contract only | User requirement; latency | — |
| 7 | **Every component in its own crate**, one Cargo workspace, strict downward layering | User requirement; isolation and testability | — |
| 8 | Starting capital **$200**: $100 gas float, $10–20 deploy, $50 VPS reserve, $30–40 buffer | User constraint | Profits arrive |
| 9 | Flash loans provide all trade size; free sources first (Balancer, Morpho), Aave as fallback | Capital-free trading; contract reverts unprofitable trades | — |
| 10 | **No own node until profitable**: Oracle Cloud free VPS in `us-ashburn-1` + free sequencer feed/Flashblocks + free RPC tiers (dRPC, Alchemy, Chainstack) | Any node costs $77–365/mo, too much for $200 | 30-day profit ≥ 3× node cost and latency shown to lose trades |
| 11 | Host in **US East (Ashburn/Virginia area)** | Sequencers believed to be in US East; ~70 ms edge vs Frankfurt | Phase 0 latency measurements |
| 12 | Upgrade order: paid RPC (~$50) → node | Most profit per dollar first | — |
| 13 | Preferred Stage 3 node: **RedSwitches dedicated node, Manassas VA** (pre-synced node + root access, bot on the same box, from $199/mo) | Only provider found offering a pre-built node plus a server we can deploy on | Better offer found; get exact quote first |
| 14 | Alternatives kept: Chainstack Dedicated on AWS US East (managed, region choice); Argusnodes (Base, from $199, endpoint only); OVH Rise/Advance self-run (from ~$77) | Backup options | — |
| 15 | Planning docs live in `docs/planning/` | User requirement | — |

## 2026-10-03 — MVP deployment

| # | Decision | Reason | Revisit when |
|---|---|---|---|
| 16 | `ArbExecutor` deployed on Arbitrum at `0xCc807Fb920c13145D5757D4C7DDBAa3f7Ce2E6AD` (owner = receiver = `0xe7322F17Ed69B9236f5103f8F62B302Cbf984447`, Balancer V2 vault lender) from branch `feat/mvp-live-arb` | User wanted live trades same day; fork tests passed immediately before deploy | Any contract change → new deployment + new row |
| 17 | Hot wallet is a reused wallet (218 prior txs), not a fresh one; receiver = hot wallet | User's choice | Move to fresh hot wallet + separate cold receiver when scaling |
| 18 | First shadow run with sim: 55 major pools, ~12 updates/min, 0 opportunities > $0.05 | Major pairs are efficiently arbitraged; Uniswap v4 / Pancake V3 / Algebra pools not covered yet | Next: widen pool coverage (long-tail pairs, lower liquidity floor, more DEX types) |
| 19 | **Fixed crash loop**: engine replaces a dead WS provider and bootstrap retries with backoff (1s→60s) | 5-min host DNS outage → alloy gave up reconnecting → process exited → 205 podman restarts in ~90s | — |
| 20 | **Diagnosis: no trades = coverage, not bugs.** Watched pools emit 15 events/min (matches what the bot receives); every watched pair sits 0.3–129 bps *below* break-even after fees. All of Arbitrum: ~87 swaps/min; we see ~17%. Unwatched volume: Uniswap v4 PoolManager (~12%), PancakeSwap v3 (~11%), Camelot v3/Algebra (e.g. WETH/USDC `0xb1026b8e…`), long-tail tokens (RAIN, CHIP, ATH, AIDOGE) | `mev-analyze` + chain-wide Swap scan, 2026-10-03 | Re-run `mev-analyze` after each coverage change |
| 21 | Next: (a) Camelot v3 (Algebra) + PancakeSwap v3 support — contract callbacks + redeploy; (b) auto-discover long-tail pairs from Swap events instead of a fixed pair list; (c) Uniswap v4 (PoolManager unlock flow) | Ordered by volume gained per effort | — |
