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
