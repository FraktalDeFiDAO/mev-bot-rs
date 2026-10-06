# Atlas / Chainlink SVR solver: optional, off, ready to switch on

Status 2026-10-06: **built and off.** Nothing connects, bonds or bids until the config says so.
Why it exists: the contested Aave liquidations are won in Chainlink SVR auctions (the oracle update
and the winning solver's liquidation are one Atlas `metacall` transaction), so a speed race after
the update can't win them (DECISIONS #64). Becoming a solver is permissionless but needs a bonded
balance (≈ 0.1 ETH recommended ≈ $270), which is more than our starting capital, so the pieces are
in place and the switch is a funding decision.

## What is in the repo

| Piece | Where | Notes |
|---|---|---|
| Solver contract | `contracts/src/AtlasSolver.sol` | `AtlasSolver is ArbExecutor`: `atlasSolverCall` from Atlas only, solver op `from == owner`, may only start `liquidate` / `liquidateViaAave`, `maxBid` cap (default 0 = no bidding), gas settled with `reconcile`, bid paid from the contract's WETH/ETH float. 10 unit tests with a mock Atlas. |
| Protocol client | `crates/mev-atlas` | auction feed parsing, `SolverOperation` EIP-712 signing, submit frames, websocket with reconnect. |
| Engine hook | `crates/mev-engine/src/atlas.rs` | auction → hint → runner prediction → build `liquidate` call → bid → sign → submit. |
| Runner input | `mev-liquidate::OracleHint` / `RunnerEvent::AtlasBackrun` | one plan per auction (both parallel auctions of an update get a bid). |
| Config | `[atlas]` in `config/arbitrum.toml` | `enabled`, `bid`, `solver`, `bid_fraction`, `max_bid_eth`, `solver_gas_limit`. |
| Funding / ops | `scripts/atlas-solver.sh` | `status`, `check`, `deploy`, `allow`, `bond`, `float`, `cap`, `unbond`; each spending step asks first. |
| Readiness check | `mev-atlas-check` | read-only: on-chain EIP-712 domain, digest vs the deployed `AtlasVerification`, optional `--listen N`. |

## Verified already (2026-10-06)

- Our EIP-712 digest equals `AtlasVerification.getSolverPayload` on Arbitrum for a sample operation
  (domain `AtlasVerification` / `1.6.4` / 42161 / `0xAC116AbB…`). Run `scripts/atlas-solver.sh check`.
- The auction feed accepts a connection with no credentials: in 150 s it showed 14 Arbitrum
  auctions (plus 33 BNB Chain and 7 Monad). Auctions carry `hints.aggregator` and `hints.medianPrice`.
  Prices come in different scales per feed (8 or 18 decimals).

## Step 0: observe (free, no funds): do this first

Set `[atlas] enabled = true` (leave `bid = false`) and deploy. The bot then logs, per Arbitrum
auction, an `atlas_auction` event (feed, answer, `watched` = whether our liquidation adapters watch
that feed, gas price, deadline) and, when a watched feed's new price would make a position
liquidatable, `atlas_would_bid` / `atlas_unprofitable` events with the expected net. A day or two of
these answers the questions that decide whether funding the bond is worth it:

1. How many Arbitrum auctions per day, on which feeds, and how many are `watched`?
2. How often does a hinted price make a position liquidatable, and for how much?
3. If `watched` is mostly false, the SVR aggregator addresses differ from the ones our adapters
   key on and a mapping is needed before bidding can work (the price scale may differ too).

Do not fund before these numbers exist.

## Step 1..6: bring bidding online (each spends, each asks)

```
scripts/atlas-solver.sh check          # digest vs the deployed verifier: must say OK
scripts/atlas-solver.sh deploy         # AtlasSolver (owner = the bot wallet); records ATLAS_SOLVER_ADDRESS
scripts/atlas-solver.sh allow          # allowlist Aave's pool (and the other targets) on the solver
scripts/atlas-solver.sh bond 0.1       # Atlas.depositAndBond: pays solver gas
scripts/atlas-solver.sh float 0.02     # WETH the contract pays bids from
scripts/atlas-solver.sh cap 0.01       # maxBid; set [atlas] max_bid_eth to the same value
```

Then in `config/arbitrum.toml`: `[atlas] enabled = true`, `bid = true`, `solver = "<address>"`,
and deploy. The caps in `[risk]` (daily gas, failure pause, shadow switch) apply to bids too, and
`bid_fraction` (default 0.7) is the share of expected net profit offered.

## Rules the code follows (from the spec)

- one signed operation per auction; bid on **both** auctions an update produces;
- `maxFeePerGas` exactly the auction's gas price; `gas` ≤ `DappControl.getSolverGasLimit()`;
- `from` is the bonded EOA and must equal the solver's `owner`;
- one auction per block per bonded EOA: with a single EOA only one auction per block can be
  entered. More throughput means more bonded EOAs (not built);
- gas is charged only if the operation executes (solver-fault failures are charged), so unprofitable
  bids are filtered before signing and the on-chain `minProfit` includes bid and gas.

## Known gaps and risks

- **Competition and win rate are unknown.** The weekly whale was ≈ $18K gross but one event; the
  other liquidations are dust. Measure in step 0 before funding.
- Liquidations are only predicted for feeds our adapters watch; the SVR aggregator vs adapter
  feed mapping is unverified (step 0 answers it). Compound III had no absorbs in ≈ 12 days; Morpho
  and Fluid are not in the supported-protocol list.
- The bid float is WETH held by the contract; the profit goes to `profitReceiver` in the debt
  token, so the float has to be topped up from profits manually.
- No result tracking yet (`solver-query-api-fra.fastlane-labs.xyz`); outcomes are visible through
  `atlas_submit_reply` events and on-chain.
- Nothing here has been run against mainnet with funds. Test with the smallest bond and `max_bid_eth`.
