# Ethereum mainnet via Flashbots (MEV-Share backruns)

Owner request 2026-10-22: add a Flashbots-enabled bot on the networks that support it. Only **Ethereum mainnet** has
Flashbots. Base and Arbitrum have a single sequencer and no public bundle market: Base orders by priority fee,
Arbitrum is first-come-first-served or Timeboost.

## Why it could matter

- **Failed attempts are free.** A bundle that would not profit is never included, so no gas is paid. That fits the
  owner's constraint of no new spending.
- **Same-block placement.** MEV-Share streams hints about pending user transactions, and a backrun bundled behind the
  user's transaction lands right after it. On Base we lose arbitrage for lack of exactly this (see the Base shadow
  run, 2026-10-21: every gap was closed inside the trigger's block).

## Against it

- Mainnet is the most contested MEV market. Searchers typically pass 90% or more of a backrun's profit to the
  builder, and MEV-Share refunds 90% to the user by default.
- Mainnet Aave liquidations largely go through Chainlink SVR auctions.
- It needs an executor deployed on mainnet: about $5–15 of gas at current prices, from the existing ~0.02 ETH. This
  needs owner approval before it is spent.

## What the hint stream shows (sampled 2026-10-22, 2 min)

47 hints (~0.4/s). 35 contain swap logs: 24 Uniswap V4, 10 V3, 5 V2. Twenty of the V4 ones are a single pool.
V3 hints reveal only the pool: `data` is `0x`, so there is no amount and no direction. Searchers therefore submit
several guesses (direction × size) per hint, and the builder's simulation keeps any that profit. Guessing is free.

## Phases

1. **Shadow measurement ($0).**
   - Add an Ethereum chain config with public RPCs.
   - Subscribe to `https://mev-share.flashbots.net` (SSE).
   - For each swap hint on a pool we price exactly, simulate the backrun after the user's swap for both directions and
     a size ladder. The user's swap size is unknown, so sweep plausible sizes and record the profit distribution, not
     a single number.
   - Record what we would bid and keep: profit × (1 − 0.9 refund) − builder share − gas.
   - Gate: continue only if the measured kept-profit over 7 days is positive and at least the executor deploy cost.
2. **Bundle path ($0 to build).**
   - Add `mev_sendBundle` signing (Flashbots auth header with a separate, unfunded signing key) and submission to
     MEV-Share and builders.
   - Test it on the Holesky or Sepolia Flashbots relay.
3. **Live (owner-approved spend).** Deploy the executor on mainnet (~$5–15), allowlist nothing beyond swaps, and send
   bundles only. A bundle never lands unless it pays, so there is no gas cap to set beyond the deploy.

## Not in scope

Sandwiching (never: see the project charter). Ethereum liquidations stay out until a measurement shows non-SVR wins.

## Phase 1 first result (2026-10-22): the hinted backrun market looks empty

Method (scratchpad scripts, public data only):
- MEV-Share history API, last 150 blocks: ~21k hints in 30 min, of which ~3.7k were swap hints that expose their
  log data.
- Hint hashes are bundle hashes, not transaction hashes, so each hint was matched to the chain by its swap log
  (pool, topics, data) within 5 blocks.
- 120 sampled; 69 matched a landed swap.

Of those 69:
- **No refund** was paid to any of the users later in the same block, from the builder or from anyone else. MEV-Share
  pays 90% of a backrun's profit to the user, so a backrun would show up as a refund.
- 5 were followed by another transaction on the same pool, and the 3 inspected were ordinary user swaps, not
  arbitrage.

So almost none of this order flow is worth backrunning: the swaps are too small to move prices beyond the fees.
A larger sample could change this, but on this evidence the executor deploy (~$5–15) is not justified. Gate stays
closed.

## Base same-block backruns (2026-10-22): measured, not reachable from here

Measured with `mev-flashblocks-record` (new binary in mev-sequencer): a 10-minute Flashblocks recording joined with
receipts, 299 blocks.
- 1,758 multi-pool (arbitrage-like) transactions. 992 had no trigger earlier in the same block.
- Of the 766 with a same-block trigger, 361 landed in the **same** flashblock as the trigger: they were sent before
  the trigger was public (prediction or spam).
- The other 405 landed **later**, 325 of them in the very next flashblock (~200 ms).
- The most-reverting destinations had 408, 316 and 251 failed transactions in those 10 minutes. On Base the winning
  style is to spray backruns and let unprofitable ones revert (~$0.005 each).

Local shadow run with `[presim]` against the Flashblocks feed, ~50 min:
- 6,120 presimulated updates; mean `eth_simulateV1` time 130 ms once warm (762 ms cold, public endpoints).
- 37 candidates, $7.33 modelled in total (max $0.83). All were `NotProfitable` at verification: the gap was gone.

Conclusion:
- An RPC simulation round trip (150–300 ms from the server or locally) already uses up the 200 ms window to the next
  flashblock.
- Reacting in-process (local decode plus exact math, fire and let the executor revert) would cut compute time but not
  distance. The bot runs in Germany, and the Base sequencer's endpoints answer in ~150 ms round trip, so receive plus
  send is already about one flashblock.
- Winning the next flashblock needs co-location near the sequencer, which costs money. Presim stays off for Base live:
  it roughly triples public-RPC load for no gain.
