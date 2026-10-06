# Arbitrum MEV Live Intel — 2026-10-06 (afternoon session)

Purpose: extend today's research with live/current data in search of **wins now**, not
quarter-long build-outs. Everything here is diffed against DECISIONS #52–#63 so it only
carries what is NEW. Sources: Arbitrum official blog (2026-09-24, post-dates #59's
sources), arXiv 2509.22143 + 2511.18328 (Timeboost empirical studies), Liminal Labs live
liquidation feed (today), TokenLogic Aave SVR dashboard, GeckoTerminal live pools,
Defiant/Coinbase/WalletInvestor coverage.

---

## 1. NEW: official PGA guide published Sep 24 (after #59's source set)

`blog.arbitrum.io/competing-for-transaction-priority-on-arbitrum` — the official
technical guide. #59 already has the AIP mechanics; genuinely new details:

1. **Official fee recipe (Go snippet in the post):**
   `maxFeePerGas = 2 × baseFee + priorityFee`.
   The effective ordering priority is `min(maxPriorityFeePerGas, maxFeePerGas − baseFee)`.
   → **VERIFIED 2026-10-06 — PASS, no change needed.** `mev-risk/src/bid.rs:119` builds
   `max_fee_per_gas = 2 × base_fee + tip` (doc comment spells out the intent; tests at
   bid.rs:346–347 assert the formula and the headroom bound). All three live send paths
   (arb `engine:1077`, oracle-backrun liq `engine:1286`, sweep liq `engine:1389`) pass
   `Some(fees)` from `pga_bid`; the tip=0 `no_bid` fallback fires only when
   `pga.enabled = false` or a caller passes `None` (no live caller does). `tx_request`
   (`mev-exec:328`) adds a redundant `maxFee ≥ tip` guard. Only residual: `pga_bid`
   feeds the 10 s-cached `eth_gasPrice` as the base fee — if the node returns
   base+ε, headroom only grows (safe direction), and a tip squeeze would require the
   base fee to more than double within the cache window (not plausible at Arbitrum's
   ~0.01–0.02 gwei base fee).
2. **Ties break by arrival time measured AT the sequencer** (not RPC ingress). Our
   us-east-2 host at 2.7 ms warm RTT (#44) is already right. But ties only matter at equal
   final priority — the tip ladder remains the primary lever.
3. **"A transaction cannot interrupt a round already in progress"** — a send landing
   mid-round waits for the next 125 ms round. Timing sends to round boundaries is a
   (minor) latency lever the docs make explicit.
4. The observe→decide→bid→execute loop is officially described as Fast Feed + PGA
   combined. This is the reference architecture for what #43's presim does less directly.

## 2. NEW: Timeboost empirical studies quantify the field we just joined

arXiv 2509.22143 (48.5M express-lane txs, 494,608 auctions, Apr 2025–Apr 2026) and
arXiv 2511.18328 (43,248 rounds):

- **Express-lane cartel:** Selini + Wintermute + Kairos won 99.74% of all rounds.
  That structural advantage **dissolved when PGA activated** (~Sep 24–30, per our own
  #59 on-chain verification). The tip equilibrium winners 0.02–2.9 gwei (#63) is days
  old and still forming — early-mover calibration window.
- **Wintermute economics** (reference for what "winning" pays): filtered markouts
  $1.275M vs $342K actually paid (recent window); Selini $865K vs $237K. Kairos resale
  was unsustainable (on-chain loss; JetBuilder exited entirely).
- **~30% of express-lane txs reverted** under Timeboost — spam/revert pressure was
  endemic. Under PGA this becomes a *fee-burning* failure: every reverted raced send
  pays base gas + tip. Our cooldown (#50) and win-rate gate (#54) directly counter this.
- **KEY FINDING: "profitable arbitrage often occurs at the end of blocks."** MEV
  clusters in the final round(s). With 2 PGA rounds per 250 ms block, backruns should
  target **round 2 of the block** that consumes the trigger tx. Combined with #62 (we
  reach the winner's round but lose ordering *inside* it), the levers are: tip ladder
  (#63) + *when* in the block we aim to land.

## 3. NEW: live liquidation flow today (Liminal Labs, 2026-10-06)

- **Last 24 h: 1 Aave V3 liquidation on Arbitrum. $7 debt.** Trailing 7 d: 26
  liquidations, $287,289 total — but **one whale = $286,719 (99.8%)**: WBTC debt repaid,
  WBTC collateral seized $304,782 (~6.3% spread, ~$18K gross).
  tx: `0xe04f1ec5387e1474646eb67716454b088d0fe41b17022fc5f4bdb2716047fa6a`
- The other 25 were dust ($0.003–$384) — bots with ~zero marginal cost grind $0.50
  liquidations. Do not chase dust; it only pays the grinder's fixed costs.
- **Confirms #51's crash-day thesis with current data**: calm-day liquidations ≈ zero;
  the entire week's value was one position. The whale tx is a free case study for the
  crash playbook: who won it, their tip, their route, and whether SVR recaptured it.

## 4. RESOLVED 2026-10-06: SVR on Aave Arbitrum — 17 of 20 reserves, and the liquidation game has moved to the Atlas auction

**Authoritative asset list** (ARFC 24241 + bgdlabs snapshot additions, verified against the
on-chain 20-reserve `getReservesList()`):

**SVR-enabled (17):** WETH, WBTC, wstETH, weETH, rETH, rsETH, ezETH, tBTC, USDC (native),
USDC.e, USDT0, DAI, FRAX, LUSD, ARB, LINK, AAVE.
**NOT SVR (3):** GHO, MAI, EURS — minor reserves with negligible liquidatable debt.

On-chain color from the probe: the market oracle `0xb56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7`
prices fine (`getAssetPrice(WETH)` = $2715.38 ✓) but `getAssetSource`/`assetsSources` revert —
consistent with #47 ("SVR/CAPO/custom feeds revert on all aggregator getters"). The oracle
is a custom SVR-routing contract; per-asset sources are not readable via plain `eth_call`.

**Mechanics (CRITICAL — changes #41's expected value):** on L2s, SVR runs on Chainlink
Atlas. The price report goes to Fastlane's Operations Relay FIRST; solvers bid; a Chainlink
node bundles oracle-update + winning solver op(s) into ONE atomic tx submitted to the
sequencer. Consequences:

1. **Primary liquidations on the 17 SVR reserves are won BEFORE the price update is
   on-chain.** Our #41 backrun (decode Chainlink report in the feed → send liq) structurally
   cannot win the primary liquidation anymore — the winner is inside the same tx as the
   update. What remains backrunnable:
   - **Fallback path**: no bids / all solver ops fail → price update lands via the standard
     feed after the cutoff (30 s on Arbitrum per LlamaRisk July 2026; ARFC proposed 60 s).
     These updates ARE backrunnable by us as today.
   - **Layered backruns** (LlamaRisk §3.2, observed on Ethereum): secondary liquidations the
     auction winner didn't capture are taken by fast searchers monitoring execution output —
     our 2.7 ms host + PGA tip is well-suited for exactly this.
2. **The real win: become an Atlas solver.** Onboarding is **permissionless** (no Chainlink
   contact). What it takes (Chainlink searcher-onboarding-atlas spec, v1.6.4):
   - Bond **0.1 ETH** on Arbitrum via `Atlas.depositAndBond` (bonded balance auto-pays gas;
     unbond → escrow → redeem).
   - Connect `wss://svr-bid-endpoint.chain.link/ws/solver`, `solver_subscribe
     ["userOperations"]`. Notifications carry **`hints.medianPrice` + `hints.aggregator`** —
     the feed address and the new price pre-decoded, exactly the inputs #41's repricing
     engine consumes (no report decoding needed at all).
   - Deploy a solver contract inheriting FastLane's `SolverBase` (`atlasSolverCall`
     callback; bid paid to the execution environment inside the call). Our `ArbExecutor`
     liquidation path slots in as the custom logic.
   - Sign per spec: EIP-712 over the solver op; gas price EXACTLY the oracle's stated price;
     gas limit ≤ `DappControl.getSolverGasLimit()`.
   - **Parallel auctions**: BTC/ETH feeds on Arbitrum run TWO independent auctions per
     opportunity (different Chainlink nodes) — must bid on BOTH or take ~50% win probability.
   - One auction per block per bonded EOA → run several solver accounts for throughput.
   - Auction window ~2 s; results traceable via `solver-query-api-fra.fastlane-labs.xyz`
     (EIP-55 checksummed addresses mandatory).
   - Fixed overhead ~250k gas/auction (LlamaRisk) — trivial at Arbitrum gas prices.
   - Reputation: 2 bid slots per tx (1 high-rep, 1 open); low-rep bids CAN win on price.
3. **Compound is also a supported protocol for Atlas SVR** (AAVE, Compound, Venus) — check
   whether Compound III Arbitrum reserves run SVR auctions before assuming our Compound
   adapter's direct liquidations keep full bonuses. Morpho and Fluid are NOT in the
   supported list → they stay direct-liquidation venues with uncapped bonuses.
4. **Planner impact (the original filter ask):** discount Aave liquidation EV for the 17
   SVR reserves (observed recapture 86–100% per-tx on contested events; high-bonus
   collateral trends >80% per LlamaRisk); keep GHO/MAI/EURS at full bonus (negligible
   volume); keep Morpho/Fluid/Compound-until-verified at full bonus. Recapture split:
   ~65% Aave ecosystem / 31.5% Chainlink / 3.5% builders — the auctioned share is real.

## 5. NEW: live pool targets today (GeckoTerminal, 2026-10-06)

Highest churn = where transient dislocations live (aligns with #21 coverage gaps and
#57 long-tail weighting):

| Pool | Liquidity | 24h Vol | Churn | Note |
|---|---|---|---|---|
| **Dory/USDC 1% Uniswap V4** | $3.47M | $11.81M | **3.4×** | highest-churn tracked pool today; V4 = our known coverage gap (#21) |
| ETH/USDC 0.05% Uniswap V4 | $6.99M | $6.54M | 0.9× | V4 gap |
| WETH/USDC 0.05% Uniswap V3 | $36.5M | $47.9M | 1.3× | main arena, contested |
| WBTC/USDC 0.05% Uniswap V3 | $7.97M | $12.96M | 1.6× | |
| **USDC/USDT Fluid** | $12.6M | $6.22M | 0.5× | stablecoin circuit leg |
| USDC/USDC Camelot V3 | $600K | $1.68M | **2.8×** | stablecoin circuit leg (Algebra supported since #22) |
| USDC/USDT Curve | $636K | $822K | 1.3× | stablecoin circuit leg |

- **Stablecoin circuit candidate:** Fluid USDC/USDT ↔ Camelot USDC/USDC ↔ Curve ↔
  Uni V4 0.002% USDC/USDC. Small per-hop edges but dense and repetitive — suits our
  model-exact sends (#48/#60) and uncontested-route gas policy (#50).
- **ARB regime:** +123%/month, −14.7%/week (Robinhood Chain narrative). High-vol regime
  = elevated dislocation frequency on ARB/WETH ($6.35M/day Uni V3) and ARB/USDC across
  Uni V3/V4, Camelot, Pancake. Long-tail ARB routes are where our caps allow aggression.
- Chain-wide DEX volume today: $64.1M/24h.

## 6. Fast Feed re-evaluation with fresh facts (#59 has it OFF)

New from the official guide + docs:

- Tickets are **"priced solely to deter spam"** — EIP-4844-style demand pricing, every
  ticket in a round the same price, target 100 tickets/round, floor. Contract
  `0x451DD62C26b753d07d6104aaC6865AaE73a6A598`, min $17/24h round (per #59).
- Fast Feed publishes each tx **as the sequencer orders and executes it, inside the
  round, before block close** — up to ~2× faster than the block feed our Nitro-style
  feed path consumes. No replay, starts at head.
- For us the value is concrete: #43's presim currently pays an `eth_simulateV1`
  round-trip (PublicNode p50 37 ms) per candidate; Fast Feed delivers ordered txs
  directly, earlier than our feed → faster trigger→broadcast on both arb and the #41
  oracle backrun (where 400 ms decided same-block winners in #40).
- Suggested experiment (1 day, ≤$17 + tip budget): **the client is already built** —
  `crates/mev-ingest/src/fast_feed.rs` exists in the repo (#59 kept it off by default,
  key via `ARBITRUM_FAST_FEED_KEY`). The A/B is a config flip + key purchase, not a
  build. Gate: keep only if measured win-rate uplift > $17/day. Post-cartel (§2) is the
  cheapest this edge will ever be.

## 7. Wins-today list (ordered by effort → payoff)

1. **Verify `maxFeePerGas = 2×baseFee + tip` headroom in mev-exec** (§1.1). Minutes;
   protects the value of every tip #62/#63 already computes. ✅ **DONE — PASS** (§1.1).
2. **SVR reserve enumeration** (§4). ✅ **DONE — 17/20 reserves are SVR-enabled; only
   GHO/MAI/EURS exempt.** Follow-ups: (a) planner EV discount for the 17; (b) the bigger
   prize — Atlas solver onboarding (permissionless, 0.1 ETH bond, spec in §4).
3. **Add today's high-churn pairs to the watchlist** (§5): Dory/USDC V4, stablecoin
   circuit, ARB pairs. Config-level; matches #21/#57 direction.
4. **Dissect the WBTC whale liquidation** `0xe04f1e…7fa6a` (§3): winner, tip, route — and
   now also whether it landed as an Atlas bundle (atomic oracle+liq tx) or a plain backrun.
   ✅ **DONE (§9): Atlas bundle; winner `0x754bedb7` used a 0-fee Morpho flash loan; the
   audit-#40 "winner" was the Atlas contract itself — the contested liquidations were
   auction wins, not speed races.**
5. **Target round-2 landings for backruns** (§2): end-of-block clustering is measured,
   not folklore. Tune send timing relative to round boundaries where the pipeline allows.
6. **One-day Fast Feed A/B** (§6): ≤$17, reversible, addresses our two measured
   losses (#40 400 ms re-poll gap, #62 same-round ordering losses).
7. **Keep recalibrating the tip ladder daily** (#63 was built on 8 h of a 12-day-old
   regime; winners 0.02–2.9 gwei is a moving target — the arXiv economics say the
   big players are re-entering via tips now that the express lane is gone).
8. **NEW — Atlas solver decision** (§4): the primary-liquidation channel on Aave (and
   possibly Compound) Arbitrum. We already have HF census, repricing, executor, revm sim;
   the delta is a `SolverBase` contract + WS bidder + 0.1 ETH bond + dual-auction bidding.
   Highest-ceiling item on this list, ~1–2 build days, and it is permissionless.
   **Whale forensics (§9) strengthens this to the top priority:** the weekly whale was an
   auction win; speed (PGA tips, Fast Feed, 400 ms re-poll) cannot reach these. Winner's
   recipe = Morpho 0-fee flash + our exact machinery, inside a `SolverBase` callback.
9. **Compound absorb-shape check on our own node** (§10): minutes, resolves whether
   Compound direct liquidations keep full bonuses.

## 8. Corrections to earlier docs

- `LIQUIDATION_ARB.md` (written earlier today) recommended Kairos/Timeboost express-lane
  integration (§2.3, A3) and listed "PGA vs Timeboost" as open (§8.1). Both are obsolete:
  PGA live (#59), Timeboost decommissioned. Sections amended today; see that file's
  update note.

## 9. Whale forensics — tx `0xe04f1ec5…7fa6a` (2026-10-06, fully decoded on-chain)

The 7-day whale ($286,719 WBTC debt) was an **Atlas SVR auction bundle**, and it forces a
reinterpretation of audit #40:

| Element | Finding |
|---|---|
| Shape | ONE tx: SVR oracle update + Morpho flash loan + Aave `LiquidationCall` + UniV3 swap + profit sweep, all atomic |
| `to` | `0x8ad1ae9d97c79aa68a0a151e83ff3942f68f86c1` = **the Atlas contract** (`ESCROW_DURATION()` = 128 ✓; calldata selector `0x4317ca01` = `metacall(userOp, solverOps, …)`) |
| Sender | `0x8ada73a5…` — the Chainlink-node bundler, not the solver (matches docs: "the final tx is submitted by the Chainlink oracle network") |
| Solver EOA | `0x0b6065f7…` (embedded in the solver op) |
| Winner's contract | `0x754bedb7d53876b07fe55c4a021b4a21de4bacd5` — Aave's reported liquidator; **flash-borrowed 3.3299 WBTC from Morpho Blue** (event signature `FlashLoan(address,address,uint256)` verified), repaid the whale's Aave WBTC debt, seized 3.5397 WBTC (~$18.1K gross), swapped via UniV3 (pool `0x2f5e87c9…`) plus a V2-style hop (`0x078f3582…`), repaid Morpho, took profit (custom event at log 29) |
| Liquidated user | `0x07e0dd45…` (collateral WBTC, debt WBTC) |
| Gas | `maxFee = tip = 39,820,200 wei` exactly — the **Chainlink-set gas price** per the Atlas spec (solvers have no tip choice in the auction path); base fee 21.5M → effective priority ≈ 0.0183 gwei; 1.15M gas ≈ **$0.12 total** |
| SVR feed | Log 0: `SecondaryRoundIdUpdated(uint32)` from `0xe7c522c6…` — the WBTC SVR dual aggregator confirming the private-route update |

**The #40 reinterpretation (important):** audit #40's "winner `0x8ad1aE9D…`" on every
contested same-block liquidation was the **Atlas contract, not a competitor EOA**. Those
liquidations were won in the off-chain SVR auction seconds before inclusion — there was
never a 400 ms speed race to win. #41's decode-and-send backrun targets a competition
that no longer exists for primary liquidations on the 17 SVR reserves. The paths that
remain winnable without being a solver: the **fallback window** (auction unbid/failed →
public update after the cutoff) and **layered backruns** (matches #51's timing data: of
60 crash-day liquidations, ~half landed ≥1 block after the report — those are the
fallback/layered population).

**Executor note:** the winner funded with **Morpho Blue flash loans (0 fee)** — the bot
already has Morpho allowlisted (#26, `0x6c247b1F…` in config). The solver-contract build
reuses `ArbExecutor`-style logic behind FastLane's `SolverBase`.

## 10. Compound III Arbitrum SVR status — partially resolved (2026-10-06)

All 4 configured Comets' asset feeds enumerated on-chain (22 unique):

- **5 canonical Chainlink proxies** (readable `aggregator()`): ETH/USD `0x639Fe6ab…`,
  ARB/USD `0xb2A82404…`, WBTC/USD `0xd0C7101e…`, GMX/USD `0xDB98056F…`,
  TBTC/USD `0xE808488e…`.
- **17 non-standard wrappers** (revert on `aggregator()` — the same signature family as
  SVR/CAPO feeds, per #47): `0x15Eb9487…`, `0x250BD3Da…`, `0x31193088…`, `0x4F12633d…`,
  `0x5372Bcf3…`, `0x5998a5C5…`, `0x5D173813…`, `0x5Eba6Ec9…`, `0x60F20583…`,
  `0x675DAfae…`, `0x92014e7f…`, `0xA2699232…`, `0xA4F2e977…`, `0xb2988bDA…`,
  `0xC35137f6…`, `0xcc392d2c…`, `0xFa454dE6…`.
- **SVR vs CAPO: unresolved.** The decisive probe (scanning feeds for
  `SecondaryRoundIdUpdated`) is blocked by the public RPC silently truncating ranged
  getLogs — even the known-SVR WBTC aggregator `0xe7c522c6…` returns 0 in windows that
  provably contain its event (#29's arb1 limitation). Chainlink docs list Compound as an
  Atlas-supported protocol, so SVR is plausible for the wrapper feeds.
- **Decisive follow-up (minutes on our own node/index):** pull recent `AbsorbDebt`
  events on the Comets and check the shape of those txs — `to == 0x8ad1ae9d…` (Atlas
  metacall) ⇒ Compound liquidations are SVR-auctioned and direct-absorb EV must be
  discounted like Aave's; plain liquidator-EOA txs ⇒ full bonus stands. Until then,
  treat Compound liquidation EV with the same recapture haircut as Aave.

## 11. Verification against our own on-chain data and corrections (2026-10-06, evening)

Method: read-only RPC (public Arbitrum nodes), scripts kept out of the repo. Samples are small
and the arbitrage classifier is a heuristic; treat every number as indicative.

**Latest 50 liquidations** (Aave v3 40, Morpho Blue 10; blocks 509,07x,xxx–511,611,498, ≈ 8.8
days; Compound III: **0** `AbsorbCollateral` events in ≈ 12 days across all four Comets):

| Finding | Number |
|---|---|
| Transactions sent to the Atlas contract (`0x8ad1ae9d…`) | 12 of 50 |
| Net bonus minus gas, 28 liquidations with oracle prices | Atlas txs (5 priced) +$88.5; all other (23 priced) −$0.30 |
| Position of the tx in its block | ≤ 3rd in 28 of 50; tip > 0 in 28 of 50 |
| Median gas used | ≈ 504k (max 8.38M, a bundle with 539 logs) |
| Concentration | one contract was `tx.to` of 10 txs, another 5, another 4 |

So the doc's central claim holds in our sample: the money in Aave liquidations moves through
Atlas auctions; everything else is dust that nets ≈ $0 after gas. (The whale `0xe04f1e…` is in
the set at block 509,868,075 ≈ 6.9 days ago; its USD pricing failed in my script, so it is not
in the totals above.)

**Latest successful arbitrage candidates** (32 in ≈ 24 minutes of chain, blocks
512,231,434–512,237,108; a multi-pool swap tx where the caller or target ends net-positive in
tokens and is not a pool; excludes reverted attempts, priced at current oracle prices):

| Finding | Number |
|---|---|
| Profit per trade | median $0.03, p75 $0.28; two outliers of $1,346 and $543 (USDC, 2 swaps, **zero tip**) |
| Gas per trade | median $0.042 (≈ 500k gas); net positive in 18 of 32 |
| Tip paid by the winning arb | median 0.002 gwei; only one above 1 gwei |
| Hops | 26 of 32 are two-pool |
| Concentration | one contract did 11 of 32, two more did 5 and 3 (59 % together) |
| Position in block | 20 of 32 in the last 3 slots (supports §2: end-of-block clustering) |

Reading: most winning arbs tip almost nothing, which means they are **uncontested**, not that
low tips win contested races (our losses are on pools several bots watch, where winners paid
0.02–2.9 gwei). Value sits in a few large, rare trades. Routes with no competitor are the
target, not tip escalation.

**Corrections and caveats to sections above**

- §3: the whale is 6.9 days old, consistent with "trailing 7 d" but not "last 24 h". The claim
  "1 Aave liquidation in the last 24 h ($7 debt)" is **not reproduced**: our scan finds none in
  the last ≈ 1.8 days (latest event at block 511,611,498). Unverified.
- §4.2 (Atlas solver): "~1–2 build days" omits the **capital**: the 0.1 ETH bond is ≈ $270 at
  $2,715/ETH, more than the whole $200 starting capital, and the hot wallet holds ≈ 0.006 ETH.
  It needs an explicit funding decision. Before funding, measure how many solver operations
  compete per auction (FastLane solver query API) and the real win rate of a small solver;
  the weekly whale was ≈ $18K gross but is one event, with dual auctions and unknown rivals.
- §10 (Compound): resolved by volume. No Compound absorbs in ≈ 12 days, so whether they are
  SVR-auctioned does not change expected value now.
- §5 (pool targets): GeckoTerminal figures are unverified, the "Dory" token is unvetted
  (thin new tokens are a common honeypot/manipulation risk), and Uniswap V4 is a coverage gap
  that needs an adapter, so "config-level" does not apply to the V4 pools.
- §6 (Fast Feed): ≥ $17/day is 8.5 % of the starting capital per day; we have zero wins to
  attribute an uplift to. Defer until the bid changes (#62, #63) produce wins.

**Status of §7 after this pass**

| # | Item | Status |
|---|---|---|
| 1, 2, 4 | maxFee headroom, SVR reserves, whale forensics | done (§1.1, §4, §9) |
| 3 | watchlist additions | blocked: V4 adapter and vetting needed |
| 5 | round-2 / end-of-block landings | supported by the arb sample; no pipeline hook yet |
| 6 | Fast Feed A/B | deferred (cost vs zero measured wins) |
| 7 | recalibrate tip ladder | ongoing; `bid` events log feed/race estimates |
| 8 | Atlas solver | gated on funding and a competition measurement |
| 9 | Compound absorb check | done: no volume |
| new | `mev-liq-audit` labels Atlas bundles (`Verdict::AtlasAuction`) | implemented |
