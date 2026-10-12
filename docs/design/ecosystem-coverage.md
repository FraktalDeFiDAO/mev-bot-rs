# Arbitrum ecosystem coverage: what we watch, what we miss, and the order to close it

Measured 2026-10-12 from DeFiLlama (`api.llama.fi/overview/dexs/arbitrum`, `/protocols`; Arbitrum-only figures) against
`config/arbitrum.toml`, `docs/design/watched-protocols.md` and the shadow run on `mev-ohio`. Re-run the two queries to
refresh; the table below is a snapshot, not a feed.

## 1. Exchanges, ranked by 24 h Arbitrum volume (about $141 M in total)

| Venue | 24 h vol | Share | State today | Next step |
|---|---|---|---|---|
| Uniswap V3 | $82.5 M | 58.5% | **watched** (logs + tick maps, integer-exact, verified vs QuoterV2) | none |
| Uniswap V4 | $17.8 M | 12.6% | **watched**, hookless pools, exact, verified vs V4 Quoter; new pools via `Initialize` | hooked pools: not modelled (a hook can change amounts) |
| Metric V1 | $13.5 M | 9.6% | probably not a pool venue ($0 TVL, listed as `DEX`): likely a front end whose volume settles on other pools; **unverified** | trace one swap to see which pools it settles on |
| Fluid DEX | $9.9 M | 7.0% | **recorded**, not priced: all 23 pools' `Swap` events (verified: 2,847 swaps / 3 days on 17 pools), pool list re-read hourly. No price cache and no exact quote yet; the executor has no Fluid DEX hop, so it cannot trade there | exact `swapIn` math (~2,600 lines of Solidity across the core modules, packed storage, center-price and range shifts) and an executor hop |
| Pendle V2 | $4.0 M | 2.9% | **not watched** (PT/YT/SY AMM) | oracle-anchored; arb vs spot is multi-leg, defer |
| PancakeSwap V3 | $2.9 M | 2.0% | **watched** | none |
| WOOFi | $2.2 M | 1.6% | **not watched** (oracle-priced PMM) | record events; arb needs its price oracle, defer |
| Camelot V3 (Algebra) | $1.9 M | 1.4% | **watched**, priced as a certain lower bound at max fee (verified vs Camelot Quoter) | exact fee needs the oracle timepoints, see remediation doc |
| Curve | $1.7 M | 1.2% | **not watched** (TVL $8 M, stableswap-ng / tricrypto) | `TokenExchange` events, `get_dy` quote; moderate |
| GMX V2 swaps | $1.3 M | 0.9% | **not watched** (oracle-priced) | record; not arbable by pool math |
| SushiSwap V2/V3 | $0.9 M | 0.6% | **watched** | none |
| Joe / LFJ Liquidity Book | $0.3 M | 0.2% | **not watched** (bin math) | low volume, later |
| Balancer V2/V3, Maverick V2, Ramses, Zyberswap, DeltaSwap, Swapr, Solidly V3 | each < $0.3 M | < 0.6% total | **not watched** (a Balancer decoder exists for intents only) | batch later; generic factory watch records them as they appear |
| Camelot V2, Uniswap V2 | ~$0.1 M | < 0.1% | **watched** | none |

Watched today: about **75%** of volume (Uniswap V3 + V4, Pancake V3, Camelot V3/V2, Sushi, Uniswap V2). Of the rest, Metric V1
(9.6%) is not a venue of its own, which leaves **Fluid DEX (7%)** as the one large gap, then Pendle, WOOFi, Curve and GMX
(about 7% together, mostly oracle-priced venues where pool math is not the edge).

## 2. Where liquidations happen (Arbitrum, TVL / borrowed, $M)

| Protocol | TVL / borrowed | Status | Next step |
|---|---|---|---|
| Aave V3 | 509 / 361 | **active** (indexer, sweep, price-feed trigger, exact payout) | none |
| Fluid Lending | 135 / 110 | **active on all 50 T1 vaults** (`auto_vaults`); events recorded on all 95. Of 69 liquidations in 9 days none were on the two vaults first configured (34, 30 and 5 on three others), so the fixed list had been watching the wrong vaults | T2-T4 (smart collateral/debt: 45 vaults) have another `liquidate` shape; an executor allowlist entry is needed per vault |
| Compound V3 | 68 / 24 | **active** (four comets) | verify the comet list against the factory |
| Morpho Blue | 34 / 22 | **active** | none |
| Dolomite | 23 / 11 | crate exists, **disabled** (audit pending) | next lending target: largest unwatched borrow book |
| Contango V2 | 5 / 7 | not watched | liquidations of margin positions; evaluate |
| Euler V2, Curve LlamaLend, Silo V1/V2, Venus, Revert Lend | 0-3 / 0-1 each | crates for Silo exist, inactive | record `Liquidate`-style events; add as borrow grows |
| GMX V2 (perps) | 204 / n.a. | not watched | keeper-executed liquidations; recording only, no capture path |
| Gains, MUX, Ostium, Vertex-style perps | 4-9 | not watched | keeper systems; recording only |

The four active lenders hold about **93%** of Arbitrum's borrowed value. Everything else is a long tail plus perps, whose
liquidations are done by protocol keepers rather than open to a flash-loan liquidator.

## 3. What the system does about recording, new pools and the cache

| Requirement | Before | Now |
|---|---|---|
| Record every event on watched pools | journal of all known event kinds on tracked + watch-tier pools | same, plus **price move in bps vs the cache** on every price-carrying event |
| Record liquidations | classified for Aave, Morpho, Compound, Fluid, **but only seen in the sampled receipts** (the live subscription carried pool events only) | **every liquidation event of every watched lender is polled by block cursor and recorded whole** (`liquidation_event`: 101 contracts; 9 days: Aave 58, Compound absorb 56+56, Fluid 69, Morpho 5) |
| Receipts of every block | `chain_sample_every = 4` (every 4th block) | unchanged (every block ~4x the RPC calls; owner's cost decision) |
| Fluid DEX swaps | not seen | recorded as `dex_swap_event` |
| Detect new pairs | startup scan only; V4 pools at startup | **factory creation events polled** (V3 `PoolCreated`, V2 `PairCreated`, Algebra `Pool`), cursor-based, no gaps; every one recorded as `pool_created` |
| Track new pairs | none until restart | tracked when both tokens are already traded, or a second venue trades the pair; loaded in the background, `pool_tracked` recorded, subscription renewed |
| Evaluate arbitrage from the cache | after **each** log of a block | after the **whole burst** of a block's logs, once per pool (a half-applied block shows an edge another bot just closed) |
| Cache accuracy | 60 s resync; 400 corrections / 4.5 h, median 0.9 bps, p99 130 bps | measured; see section 4 |

## 4. Cache accuracy, measured

On `mev-ohio`, 2026-10-12: the 60-second resync corrected the cache 400 times in 4.5 hours across 90 pools. Median gap
0.9 bps, p90 8 bps, p99 130 bps, max 395 bps; half the corrections had the chain ahead of the cache, half behind. Large
gaps are tentative states from pending feed transactions that did not land (3,295 `FastFeedTxLost` in the same window;
`rollback_unconfirmed = false` keeps them until the next resync). Candidates are about 98% reverts at simulation time
(27,000 simulations since Oct 8) even when the pool math is exact: at one pinned block the integer model's chained output
equalled the executor's final balance to the unit on all 4 routes that completed (a fifth route contained a pool type the test does not model). So the false edges are a state-timing problem,
not a pricing one. The two causes addressed here are per-log evaluation (fixed) and unrolled tentative state (measure
first: the `source` field now on every candidate event separates block-triggered from presim-triggered reverts).

## 5. Order of work

1. Merge the burst evaluation and factory watcher; read `source` on reverts after a day in shadow.
2. Fluid DEX (state + exact quote) and every Fluid vault (liquidations).
3. Dolomite liquidations; Curve exact (`get_dy`).
4. Decide on `rollback_unconfirmed` from the `source` data.
5. Recording-only support (event classification, no capture) for GMX V2, Gains, MUX, Euler, LlamaLend, Venus, Silo.
