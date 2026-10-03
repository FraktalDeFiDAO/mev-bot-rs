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
| 22 | Executor redeployed with PancakeV3 + Algebra callbacks: `0xb41b46dd9f070Ef57A8c80fa2b27E0a425c6bD85` (old `0xCc807F…` retired) | New pool types carry the only positive edges | — |
| 23 | **No-trade root cause is economic, verified on-chain.** Model ≈ simulation (est $0.0045 vs sim $0.0041). With 133 pools, transient opportunities after swaps are $0.001–$0.014 (max seen), gas per arb tx is $0.016–$0.027 (320k–550k gas). Every trade would net negative, so the risk guard correctly sends nothing. 3/17 sims reverted `NotProfitable` = faster bots closed the gap. | `mev-scan` + 4-min low-floor shadow run, 2026-10-03 | Gas per trade < typical opportunity, or larger opportunities |
| 24 | Wallet `0xe7322F…` sent 2 txs not from the bot between deploys (nonces 219–220) | Shared hot wallet breaks bot nonce tracking | Use a dedicated hot wallet |
| 25 | Liquidations are the primary strategy (Aave v3 Arbitrum: 149 liquidations / $527k repaid / ~$26–40k bonus pool in 30 days, 8 competitors; Morpho $868/30d; Fluid 98 events/30d, 5 competitors) | Arbitrage opportunities were smaller than gas (#23) | Re-measure monthly with `mev-analyze`-style scans |
| 26 | Protocol-neutral executor: `liquidate(debt, collateral, flash, minProfit, ProtocolCall[], Hop[])` with owner-allowlisted targets/spenders, exact reset approvals, native-ETH collateral wrapped to WETH. Deployed `0xc0B4BA513359979793B9aaa8C74b526aD644AF8D`; allowlisted: Aave pool, 4 Comets, Morpho, 2 Fluid vaults | New venues need no contract change | Contract change → redeploy + re-allowlist |
| 27 | Venue status on Arbitrum (2026-10-03): Live = Aave v3, Compound III, Morpho, Fluid; Inactive = Silo v2 (~$0.09 debt), Radiant (halted), Spark (no SparkLend); AuditPending = Dolomite, Gearbox | Verified on-chain / official deployments | When a venue's debt or our verification changes |
| 28 | At-risk polling is event-driven from the sequencer feed (price-feed tx → 1.8 s re-poll burst) with a 30 s fallback, replacing 3 s polling | Speed (~0.9 s head start) and ~10× fewer paid-RPC calls | — |
| 29 | Historical log scans only on arb1.arbitrum.io (others truncate wide getLogs or refuse archive); it rate-limits this IP, so backfill is slow | Correctness over speed | A free Arbiscan / The Graph key, or Alchemy PAYG, would make backfill fast |
| 30 | RPC usage is metered per provider (tower layer in `mev-rpc`; minute summary + `rpc_usage` events). Measured 2026-10-03 shadow run: paid `fast` = 3–5 calls/min steady state (98 at startup), `scan` (arb1, free) ≈ 50/min while back-filling, `sweep` (PublicNode, free) ≈ 134 per 5-min sweep. ≈ 5–6M Alchemy CU/month — within the free 30M | Data efficiency until we run our own node; real-time data comes from the free sequencer feed | Paid calls/min trend up, or a node is added (then point `fast` at it) |
| 31 | Gas price cached 10 s in the engine (was one paid call per simulation) | Measured waste removed | — |
