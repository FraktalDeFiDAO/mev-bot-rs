# Base liquidation winners vs our bot (2026-10-16)

`scripts/liq-winners.py` over the last 14 days of Base (blocks 52,074,827-52,679,627): 2,417 liquidations,
the most recent 250 Morpho Blue and all 211 Moonwell analysed (1,407 public RPC calls).

## How Base orders transactions (Flashbots rollup-boost / op-rbuilder)

- Each 2 s block is built in ten 200 ms flashblocks with cumulative gas budgets of 1/10, 2/10, ... of the limit.
- Within a flashblock, transactions are ordered by priority fee among those that have **arrived**; a later
  higher-fee transaction cannot enter a flashblock already committed. No bundles, no revert protection on Base.
- `pending` reads the block being built; `latest` the last sealed block. `mainnet-sequencer.base.org` accepts
  raw transactions only.

## What the winners do

| | Plain Morpho (116) | Moonwell (211) |
|---|---|---|
| Won through a Chainlink SVR auction | 134 more Morpho wins (54%) went to the auction path | none |
| Contracts winning / top share | 28 / 22% | 24 / 38% |
| Trigger: Chainlink update earlier in the same block / previous block / neither | 77 / 37 / 2 | 40 / 58 / 113 |
| Priority fee gwei p10 / p50 / p90 | 0.008 / 0.15 / 3.3 (92nd pct of block) | ~0.001 median |
| Earliest flashblock p50 / p90 | 3 / 6 | 1 / 2 |
| Rival paid more but landed later | 58 of 101 | 68 of 129 |
| Blocks with reverted rival attempts on the same borrower | 91 of 116 | (few) |

- Earlier wallet-level counts understated concentration: operators rotate sending wallets behind one contract.
- A contract spamming ~600 cheap transactions per block naming the borrower (49,630 in 79 blocks, 5,597 wallets)
  never won.

## Inconsistencies found in our bot, and what changed

| Ours | Winners / chain | Change |
|---|---|---|
| Instant backrun of every public Chainlink report | SVR (DualAggregator) reports are held; backruns see the old price (our 11 Arbitrum reverts) | Held feeds detected by `typeAndVersion()`; re-polled, never backrun |
| Feeds resolved once at start from cached markets (0 on a fresh start) | n/a | Re-resolved as indexing finds markets; runner refreshes its watcher |
| Health reads at `latest` | Moonwell wins mostly follow no Chainlink update: detection is polling | `pending_reads`: `eth_call` at `pending` on Base |
| Sends via public RPC only | Sequencer accepts raw transactions directly | `sequencer_rpc = mainnet-sequencer.base.org` |
| Arbitrum bid defaults (0.05 gwei, $0.25 cap, 125 ms rounds) | 0.15 gwei median (Morpho), 0.001 (Moonwell); arrival beats fee | Base `[pga]` from the measurements |
| Liquidations on Base's 1,200 tx/min feed queue dropped while indexing | n/a | Only transactions to watched feeds reach the runner |
| Balancer only lender for some tokens (no wstETH on Base) | n/a | Aave V3 Base pool as flash lender |

Not measured: our own latency from a flashblock to a landed transaction on Base (needs a server run and,
for landing, an executor on Base).
