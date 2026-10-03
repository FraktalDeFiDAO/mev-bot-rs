# L2 MEV Bot — Profit-First Plan

Related docs: [CRATES.md](CRATES.md) (workspace layout) · [INFRASTRUCTURE.md](INFRASTRUCTURE.md) (servers, RPC, costs)

## Guiding principle

Don't decide the chain or strategy based on opinion; measure it. Most MEV bots lose money because
they compete where competition is heaviest (blue-chip DEX arbitrage). This plan measures first, and
the go/no-go gates stop spending if the data says there is no edge.

With **$200 starting capital**, the plan is shaped by one rule: **infrastructure must cost ~$0
until the bot has paid for it.** Flash loans provide trade size; our $200 only pays gas and a cheap VPS.

**Profit equation (per chain × strategy):**

```
net = Σ captured opportunity value
    − gas on winning txs
    − gas on failed/reverted txs
    − flash-loan fees
    − infra cost (VPS, RPC, later nodes)
```

Target cell: **meaningful opportunity volume, few competitors, low latency sensitivity.**
Low latency sensitivity matters doubly for us: without our own node we're slower than pros, so we
must pick opportunities where being ~100ms late still wins.

## Constraints

| Constraint | Value |
|---|---|
| Starting capital | **$200** — gas float + VPS, nothing else |
| Owned hardware | None. Rented VPS / free tiers only until profitable |
| Language | **Rust** for everything off-chain; Solidity (Foundry) only for the on-chain executor |
| Code layout | **Every component in its own crate** in one Cargo workspace (see CRATES.md) |
| Strategies | Arbitrage and liquidations only — no sandwiching |

### Capital allocation

| Bucket | Amount | Purpose |
|---|---|---|
| Gas float (hot wallet) | $100 | Live tx gas on 1 chain (L2 txs ≈ $0.001–0.05 each) |
| Contract deployment | $10–20 | Executor deploy + test txs |
| VPS reserve | $50 | Up to ~6 months of a small VPS if the free tier isn't available |
| Buffer | $30–40 | Unexpected costs; never touched for trading |

No capital is needed for trade size: flash loans fund every trade, and the executor reverts if a
trade isn't profitable, so a failed attempt only costs gas.

## Strategy ranking (hypothesis to be validated in Phase 0)

1. **Liquidations on long-tail lending protocols** — primary.
   Liquidation bonuses are 5–15%, flash loans are actually useful here (large size), and many
   protocols (Aave/Compound forks, Morpho markets, Euler v2, Silo, Moonwell, smaller chains)
   have only 1–3 active liquidators. Least latency-sensitive strategy, which suits a node-less bot.
2. **Long-tail DEX arbitrage** — secondary.
   New pools, Uniswap v4 hook pools, Curve/Balancer-style math, new DEXs on smaller L2s.
   Shares the pool state and simulator with liquidations.
3. **Blue-chip DEX arbitrage (WETH/USDC etc.)** — avoid. Saturated, and we can't win on latency.

**Starting chain: Arbitrum**, provisionally. Its sequencer feed is free and unmetered, giving us
the fastest transaction stream without a node or paid RPC. Phase 0 can override this.

## Phase 0 — Opportunity research (1–2 weeks, $0)

Goal: a ranked table of (chain, protocol/strategy) → monthly profit available, competitor count.

**Liquidations**
- Index `LiquidationCall` / `Liquidate` / `Absorb` events per lending protocol on Arbitrum, Base,
  Optimism, and smaller L2s (Unichain, Linea, Scroll, Mode, etc.).
- Per protocol, measure: monthly liquidation volume, bonus %, unique liquidator addresses, top
  liquidator's share, delay between a position becoming liquidatable and being liquidated.
- Long delays + few liquidators = slow competitors = our opening.

**Arbitrage**
- Identify historical atomic arbs: txs whose swaps form a cycle ending in the start token with
  profit to the sender. Group by bot address and by pool.
- Per chain and pool tier, measure: monthly arb profit, competing bots, top bot's share, spam ratio.

**Tooling:** Dune free tier for quick queries, cross-checked with the `mev-analytics` crate reading
logs through free RPC tiers. The analytics crate reuses `mev-rpc` and `mev-types`, so Phase 0 code
becomes production code.

**Gate 0:** proceed only if at least one cell shows capturable net profit > $100/month at a 10–20%
capture rate. (Low bar on purpose: with ~$0 infra, small edges are worth taking first.)

## Phase 1 — Executor contract (1 week, ~$15 gas to deploy)

Foundry project in `contracts/`, Rust bindings generated into the `mev-bindings` crate.

- Flash-loan sources in cost order: **Balancer V2/V3 vault and Morpho (free)** → Uniswap v3/v4 flash
  → Aave v3 (fee) as fallback.
- Generic route executor: compact encoded swaps (Uniswap v2/v3/v4, Aerodrome/Velodrome, Curve,
  Balancer, Camelot) plus liquidation calls.
- `require(balanceAfter >= balanceBefore + minProfit)` — revert if not profitable.
- Security: owner-only entry, callbacks verify `msg.sender` is the expected lender/pool and that
  we initiated the call, no lingering approvals, profit sweep function.
- Packed calldata (not ABI structs) — on L2 the L1 data fee dominates gas cost.
- Fork tests (anvil) against real liquidations and arbs found in Phase 0. Test on testnet before
  mainnet deploy to avoid wasting gas.

## Phase 2 — Off-chain engine (3–5 weeks)

Each component is its own crate — full layout and dependency rules in [CRATES.md](CRATES.md).

Key profit levers:
- **Oracle-update backrunning:** most L2 liquidations follow an oracle price update. Spot the oracle
  tx in the Arbitrum sequencer feed, simulate new prices, and submit the liquidation immediately.
- **Pre-computed liquidation queue:** positions sorted by distance to liquidation, so we already
  know who to liquidate when a price moves.
- **Simulate everything locally in revm** before sending. With a $100 gas float we can't spam.
- **RPC budget manager:** spread reads across several free RPC tiers and prefer WebSocket log
  subscriptions over polling, to stay inside free quotas (see INFRASTRUCTURE.md).
- **Gas bidding by expected value** on OP Stack chains; on Arbitrum (FCFS) compete on latency.

## Phase 3 — Shadow mode (1–2 weeks, $0)

Run the full engine live but **don't send transactions**. Log every opportunity, simulated profit,
whether a competitor took it, and how much earlier they were than us.

**Gate 1:** simulated capture rate × average profit must clear expected gas + VPS cost with margin.
If competitors consistently beat us by more than our latency, move to a slower-competition cell.

## Phase 4 — Live, small (2–4 weeks)

- Hot wallet holds the $100 gas float only; profits swept to a cold wallet daily.
- Hard limits: max gas per hour/day (start: $3/day), max consecutive failures → auto pause.
- Track per-strategy PnL, win rate, latency to inclusion, reasons for loss.

**Gate 2:** 2 consecutive weeks of positive net PnL before spending anything on scaling.

## Phase 5 — Scale (funded only by profit)

Reinvest in the order that buys the most profit per dollar:
1. Paid RPC tier ($0 → ~$50/mo) once free quotas are the bottleneck.
2. Own L2 node near the sequencer once 30-day profit ≥ 3× the node's monthly cost
   (see the upgrade ladder in INFRASTRUCTURE.md).
3. More chains and protocols from the Phase 0 ranking — same crates, new config + contract deploy.
4. Re-run Phase 0 analytics monthly; edges decay as competitors arrive.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Contract drained via callback | Strict sender checks, initiator check, no approvals left, fork-test attacks |
| Gas float burned on failed txs | Sim-before-send, failure-rate kill switch, daily gas cap |
| Free RPC quota exhausted / rate-limited | Multi-provider rotation, WS subscriptions, quota tracking in `mev-rpc` |
| Free VPS reclaimed or unavailable | Stateless deploy script; state rebuilds from chain; $50 VPS reserve |
| Toxic/honeypot tokens in long-tail arb | Token allowlist + transfer-tax/blacklist detection in sim |
| Edge disappears | Monthly re-measurement, multi-chain diversification |
| Hot wallet key leak | Dedicated wallet, minimal balance, daily sweep, keys outside repo |

## Next action

Scaffold the Cargo workspace (CRATES.md) and start Phase 0 with `mev-analytics` for Arbitrum,
Base, and Optimism.
