# Liquidation competition across chains (2026-10-15)

**Question.** Is there a chain or lending venue with enough liquidation flow and few enough competitors to be easy
money for a $200, no-colocation bot?

**Answer.** No venue is uncontested. The chains with few liquidators have almost nothing to liquidate (Optimism,
Linea, Scroll, Sonic, Mantle, Metis, Gnosis, Ink, Soneium, Plasma, MegaETH, Unichain, Katana, Celo, X Layer: under
$450 of bonus a month each). Where there is money, there are dozens of operators. Two venues are worth a closer look:

- **Base, Morpho Blue:** the largest pool (~$346k bonus a month) and the least concentrated. 617 sending wallets;
  the top one takes 12% and the top three 28%. Winners pay ~1% of the bonus in fees. Liquidations land a median 2 s
  after the Chainlink update, which is the next Base block, so this is a latency race but not a bidding war.
- **HyperEVM, Morpho Blue:** ~$43k a month, but concentrated (top wallet 47%, top three 75%). HyperLend
  (~$27k a month) is effectively one operator (one wallet takes 96% of the bonus).

Arbitrum Aave, where the bot runs today, pays ~$17k a month to 73 wallets, with the top three taking 74%.

## Method

`scripts/liq-competition.py` (read-only, public RPCs from chainlist.org, never the bot's keyed endpoints):

1. `scan`: all liquidation events of each venue over the last 90 days (Aave V3 `LiquidationCall`, Morpho Blue
   `Liquidate`, Compound v2 `LiquidateBorrow` on every market of the comptroller).
2. `validate`: recounts 5 random 4,000-block ranges per venue with 500-block queries. 38 of 39 venues matched
   exactly; BNB Aave could not be rechecked (endpoints rate-limited).
3. `senders`: the transaction sender of each event, as the operator (contracts undercount: Venus routes every
   liquidation through one contract, and operators reuse contracts across wallets).
4. `summary`: bonus = USD value of collateral seized minus debt repaid, at **today's** DefiLlama prices.
5. `timing`: for 100 liquidations per venue (the 60 largest plus 40 random), the fee paid and the delay after the
   last `AnswerUpdated` of the Chainlink feeds behind the position (Aave oracle sources; Morpho oracle base/quote
   feeds). HyperEVM and Moonwell oracles do not emit `AnswerUpdated`, so their delay is not measured.

About 40k public RPC calls in total (scan 25.7k, timing 4.4k, the rest validation and sender lookups). Venue addresses and how they were checked are in the script's docstring.

## Ranked venues (90 days, per 30 days)

| Chain, venue | Liquidations | Debt repaid | Bonus | Wallets | Top 1 / top 3 wallet share | Biggest day |
|---|---|---|---|---|---|---|
| Base, Morpho Blue | 1,265 | $2.90M | **$346k** | 617 | 12% / 28% | 33% |
| Base, Moonwell | 650 | $767k | **$131k** | 142 | 52% / 69% | 87% |
| HyperEVM, Morpho Blue | 156 | $553k | $43k | 86 | 47% / 75% | 47% |
| HyperEVM, HyperLend | 61 | $167k | $28k | 21 | 96% / 100% | 29% |
| Arbitrum, Aave V3 | 149 | $307k | $17k | 73 | 41% / 74% | 33% |
| Avalanche, Aave V3 | 51 | $106k | $17k | 48 | 48% / 81% | 47% |
| Base, Aave V3 | 142 | $394k | $13k | 65 | 26% / 60% | 23% |
| World Chain, Morpho Blue | 1,825 | $27k | $11k | 32 | 41% / 87% | 34% |
| Polygon, Aave V3 | 116 | $151k | $7k | 209 | 15% / 30% | 16% |
| BNB, Aave V3 | 68 | $66k | $4k | 73 | n/a | 32% |
| Monad, Morpho Blue | 4 | $27k | $2k | 9 | 89% / 100% | 89% |
| Every other venue scanned (26) | | | < $450 each | | | |

"Biggest day" is the share of 90-day bonus earned on the single best day: Base Moonwell's $131k a month is mostly
one crash day.

## Fees and speed (100 sampled liquidations per venue)

| Chain, venue | Fees / bonus | Median / p90 priority fee | Chainlink update found | Same-block backrun | Median delay | Within 2 s | Later than 60 s |
|---|---|---|---|---|---|---|---|
| Base, Morpho Blue | 1% | $0.21 / $5.19 | 73 / 100 | 6 / 73 | 2 s | 46 / 73 | 0 / 73 |
| Base, Aave V3 | 0% | $0.28 / $1.60 | 86 / 100 | 0 / 86 | 2 s | 56 / 86 | 10 / 86 |
| Base, Moonwell | 1% | $0.03 / $52 | not Chainlink | | | | |
| Arbitrum, Aave V3 | 0% | $0.00 / $0.08 | 80 / 100 | 0 / 80 | 2 s | 46 / 80 | 7 / 80 |
| Avalanche, Aave V3 | 1% | $0.02 / $0.45 | 87 / 100 | 43 / 87 | 1 s | 70 / 87 | 5 / 87 |
| HyperEVM, Morpho Blue | 4% | $0.02 / $1.13 (max $5,062) | not Chainlink | | | | |
| HyperEVM, HyperLend | 1% | $0.14 / $11.92 | not Chainlink | | | | |

- Fees are a small share of the bonus everywhere, so liquidators are not bidding the profit away; the contest is
  who sees the price change and sends first.
- Base Morpho winners land within the next block (2 s) in most cases and never later than 60 s in the sample: the
  gap to win is about one block, not one transaction.
- Avalanche winners backrun the oracle update in the same block half the time, the hardest race in the sample.
- Arbitrum Aave matches DECISIONS #51: about half within 2 s, a few minutes-late leftovers.

## What this means for the bot

1. **Arbitrum stays the base.** It is where the bot, executor, and data already are. Its prize is small in calm
   months and large on crash days.
2. **Base Morpho is the only venue whose size and spread justify a second chain.** Needs: a Base config and
   executor deployment, the Morpho adapter (exists) pointed at Base, a Chainlink-update trigger with sub-2-second
   reaction, and RPC capacity for Base (estimate calls per day before enabling; see the RPC quota incident of
   2026-10-15). OP Stack ordering by priority fee matters less than expected here: winners pay ~$0.21.
3. **HyperEVM is not easy money**: concentrated, oracles outside Chainlink, and one operator owns HyperLend.
4. **Skip Optimism and the new chains** until their lending markets grow.

## Not measured

- BNB Venus Core and BNB Morpho: no public endpoint served 90 days of logs (to retry with a keyed RPC).
- Euler v2, Silo, Fluid, Compound V3, Seamless, and other lenders outside Arbitrum.
- Gross bonus, not profit: swap slippage, flash-loan fees, and failed attempts (reverted competitor transactions)
  are not counted. Today's prices, not prices at liquidation time.

## Rerun

```
scripts/liq-competition.py scan --days 90
scripts/liq-competition.py validate
scripts/liq-competition.py senders
scripts/liq-competition.py summary
scripts/liq-competition.py timing --venues base.morpho-blue,arbitrum.aave-v3 --sample 100
```

Output goes to `data/liq-competition/` (git-ignored). A full run takes about an hour and ~40k public RPC calls.
