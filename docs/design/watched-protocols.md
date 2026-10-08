# What we watch, what we decode, and what is missing

Last verified against the code, `config/arbitrum.toml` and live traffic on 2026-10-08. Update this file whenever a venue,
protocol or decoder is added; the coverage numbers come from `scripts/analyze-outcomes.py` (DECISIONS #91).

Three layers are kept apart, because "watching" a protocol means different things:

| Layer | Question it answers | Source of truth |
|---|---|---|
| **State** | What is this pool/market's price and liquidity now? | Chain event logs (`Swap`, `Sync`, `Mint`, `Burn`, ...) and periodic reads. A reverted transaction emits no logs, so logs are always what actually happened. |
| **Intent decoding** | What does a pending sequencer-feed transaction *try* to do? | Its calldata (`mev-decode`). Success is **not** knowable from the feed: it may revert. Decoded effects stay tentative. |
| **Event classification** | What happened on chain, for coverage and charts? | `mev-decode::events` table, applied to sampled block receipts (`mev-outcomes`). |

The standalone crate `mev-decode` holds layers 2 and 3 (no RPC, engine or state dependency); `mev-decode events`,
`mev-decode calldata` and `mev-decode feed` run it alone.

## 1. Exchanges (pricing state)

Configured as `[[dexes]]` plus `[uniswap_v4]`. Pools are found by factory lookups for the configured pairs, plus
auto-discovery from `Swap`/`Sync` logs whose emitter's `factory()` is one of ours.

| Venue | Kind in code | Factory / manager | Fees | State from logs | Intent decoder | Event classified |
|---|---|---|---|---|---|---|
| Uniswap V3 | `UniV3` | `0x1F98431c...F984` | 100/500/3000/10000 | Swap, Mint, Burn applied; tick maps (window 800, reload 300 s) | via SwapRouter02 `exactInput` and unidentified A/B only | swap_v3, mint_v3, burn_v3, NFT inc/dec |
| Sushi V3 | `UniV3` | `0x1af415a1...231e` | same | same | as above (any router that targets its pools) | swap_v3, mint/burn |
| PancakeSwap V3 | `PancakeV3` | `0x0BFbCF9f...1865` | 100/500/2500/10000 | Swap (different layout), Mint, Burn | as above | swap_v3_pancake, mint/burn |
| Camelot V3 (Algebra) | `AlgebraV3` | `0x1a3c9B1d...D35B` | dynamic, directional | Swap (same shape as Uniswap V3), fee read from `globalState` | as above | swap_v3 (same topic) |
| Uniswap V2 | `UniV2` | `0xf1D7CC64...bcF9` | 30 bps | Sync (reserves) | none | swap_v2, sync_v2, mint_v2, burn_v2 |
| Sushi V2 | `UniV2` | `0xc35DADB6...74C4` | 30 bps | Sync | none | same |
| Camelot V2 | `CamelotV2` | `0x6EcCab42...A652` | directional | Sync; output priced with on-chain `getAmountOut` | none | same |
| Uniswap V4 | `UniV4` | PoolManager (`[uniswap_v4]`), hookless pools only | per pool | Swap, ModifyLiquidity applied | none | swap_v4, modify_liquidity_v4 |

Notes: V2 `Mint`/`Burn` are classified for coverage but pool state follows `Sync`. V4 pools with hooks (for example the
hooked dynamic-fee USDG/USDC pool) are not modelled.

## 2. Lending and liquidations

Configured per protocol; each is a `mev-lending-*` crate behind one trait, found by indexing events, scored by periodic
health sweeps, and triggered early by Chainlink price-feed transactions seen on the sequencer feed.

| Protocol | Status | Detail |
|---|---|---|
| Aave V3 | active | pool `0x794a6135...14aD`, data provider, oracle `0xb56c2F0B...02C7C7`; `LiquidationCall` classified |
| Compound III (Comet) | active | four comets configured under `[[compound_v3.comets]]`; `AbsorbCollateral`/`AbsorbDebt` classified |
| Morpho Blue | active | `0x6c247b1F...8F5e`, indexed from block 250,000,000; `Liquidate` classified; unrealisable liquidations were seen, see DECISIONS |
| Fluid | active | vault resolver plus two vaults (ETH/ARB, WBTC/USDT0) |
| Radiant, Silo v2 | inactive | crates exist; no meaningful debt to liquidate |
| Spark | not on Arbitrum | crate is a placeholder |
| Dolomite, Gearbox v3 | audit pending | crates exist, not enabled |

**Intent decoders for liquidation calls: none yet.** We see liquidations by their events (after the fact) and by health
sweeps, but we do not decode a competitor's pending `liquidationCall`/`liquidate`/`absorb` from the feed.

## 3. Oracles, flash-loan sources and other infrastructure

| Component | Used for |
|---|---|
| Chainlink price feeds (ETH/USD `0x639Fe6ab...a612` plus per-asset feeds) | liquidation triggers: feed transmit transactions are watched on the sequencer feed |
| Aave oracle | USD prices for Aave/Morpho/Fluid loan tokens |
| Chainlink SVR / FastLane Atlas | observation only (`[atlas]`, bids off) |
| Balancer Vault (`0xBA122222...F2C8`) | flash-loan source for arbitrage; `Swap` classified |
| Aave pool | flash-loan source above the Balancer cap (5 bps) |
| Uniswap V3 pools | flash-swap source (no fee) |

## 4. Sequencer-feed transactions we decode today

Share of the feed transactions that moved a watched pool (10 minutes, 669 transactions, 2026-10-08):

| Destination | Selector | Share | Identity | Decoded |
|---|---|---|---|---|
| `0x278d858f...F8D2` | `a00597a0` | 57% | unidentified (swap with price limit and deadline) | yes (`ROUTER_A`) |
| `0x96DaA0B8...ab73` | `000000e7` | 24% | unidentified (swap to a price limit) | yes (`ROUTER_B`) |
| `0x68b34658...Fc45` | `b858183f` | 13% | **Uniswap SwapRouter02** `exactInput` (verified: selector and ABI layout match the documented function) | yes, single-hop |
| `0xA51afAFe...81a3` | `3593564c` | 1% | Uniswap Universal Router `execute` | no |
| `0xE592427A...1564` | `414bf389` | <1% | Uniswap V3 SwapRouter v1 `exactInputSingle` | no |
| `0x11111112...2A65` | `07ed2379` | <1% | 1inch AggregationRouter v6 `swap` | no |
| `0x4752ba5D...aD24` | `7ff36ab5` | <1% | Camelot Router (V2) | no |
| `0x00000000...da032` | `765e827f` | <1% | ERC-4337 EntryPoint v0.7 `handleOps` (wraps arbitrary calls) | no |

Measured accuracy of the decoder against the node simulation for A and B: 93% exact, 1.5% close, 5% differ
(DECISIONS #89). Price-limited swaps end at their limit when the bot sized the input to reach it; routers with no limit
are judged on the size of the move. Revert rates are not measured yet (B reverted in 6 of 12 sampled transactions); the
chain-outcome sampler fills that in.

## 5. What needs to be added, in order

Selectors were computed with `cast sig`. The ranking is by observed share of pool-moving transactions first, then by
the protocols named in the request (swaps, liquidity, liquidations). The coverage report replaces this ranking with
measured numbers once a day of `chaintx` data exists.

**Swaps**
1. SwapRouter02: `exactInputSingle` `04e45aaf`, `exactOutput` `09b81346`, `exactOutputSingle` `5023b4df`, V2-style
   `swapExactTokensForTokens` `472b43f3` and `swapTokensForExactTokens` `42712a67`, multi-hop `exactInput` paths, and the
   `multicall` wrappers `ac9650d8` / `5ae401dc` (recurse into the inner calls).
2. Universal Router `execute` `3593564c` / `24856bc3`: command bytes for V2 and V3 exact-in/out and V4 swaps.
3. Uniswap V3 SwapRouter v1, Camelot Router, Sushi routers, direct pool `swap(...)` calls.
4. Aggregators: 1inch v6, then Odos, ParaSwap, KyberSwap, 0x. Their calldata is a route description; the realistic
   target is the first-hop pool and amount, with the rest left to the node simulation.
5. ERC-4337 `handleOps`: unwrap `UserOperation` calldata and decode the inner call.
6. Identify the unidentified A and B contracts (owners, deployer, bytecode), so we know whether they are one
   operator's bots or a public aggregator.

**Liquidity (add / remove)**
7. NonfungiblePositionManager: `mint` `88316456`, `increaseLiquidity` `219f5d17`, `decreaseLiquidity` `0c49ccbe`,
   `collect`; and direct pool `mint`/`burn`. Their *state* effect is already applied from `Mint`/`Burn` logs; decoding
   adds the early view.
8. V2 routers: `addLiquidity` `e8e33700`, `removeLiquidity` `baa2abde` (and the ETH variants).
9. V4: PoolManager `modifyLiquidity` through the Universal Router and the V4 position manager.

**Liquidations**
10. Aave V3 `liquidationCall` `00a718a9`; Morpho `liquidate` `d8eabcb8`; Compound III `absorb` `c3cecfd2`; Fluid
    vault `liquidate`. Decoding competitors' pending liquidations tells us a position is being taken before the event.
11. Radiant, Silo, Dolomite, Gearbox, Spark: only when their crates are enabled.

**Venues not watched at all** (known to exist on Arbitrum; not yet verified in-chain by us): Trader Joe Liquidity Book,
Ramses and other Solidly forks, Curve pools, Balancer weighted/stable pools as swap venues (the vault `Swap` event is
classified but no pool is modelled), Maverick, DODO, Pendle PT markets, Uniswap V4 hooked pools. Each needs discovery,
a pool-state model, and its events added to `mev-decode::events` before it can be priced.

## 6. How "all of it is decoded" is proved

1. `mev-outcomes` samples blocks and records every transaction that emitted a swap, liquidity or liquidation event.
2. `scripts/analyze-outcomes.py` reports, per event class, the share of successful chain transactions whose feed
   record carries a decoded swap, and ranks the destinations that make up the rest.
3. A decoder is "done" when its destination drops out of that ranking and its shadow comparison stays above 95% exact.
4. The report is statistical (sampled blocks) and covers only event shapes in the table; a protocol with an event shape
   not in `mev-decode::events` is invisible to it until added.

## 7. Prices, pairs and charts

- **Current price after a reverted transaction:** logs are the answer: only transactions that executed leave a `Swap` or
  `Sync`. `mev-decode::events::price_update` extracts `sqrtPriceX96`, liquidity and tick (V3, Pancake, Algebra, V4) or the
  reserves (V2) from a log, so a price series per pool is a stream of those. The feed alone cannot give it.
- **Pools, pairs and markets:** the pool store and discovery crate; the recorded `chaintx` lines carry the pool address,
  topics and data of every classified event.
- **Charts:** build from `chaintx-DATE.jsonl` (sampled blocks only, 1 in 4) or, for a continuous series, from the bot's
  existing log subscription; a dedicated price-history writer is not built yet.
