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
