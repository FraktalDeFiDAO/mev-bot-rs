# Arbitrum One facts (verified against docs.arbitrum.io on 2026-10-07)

Source of truth for Arbitrum-specific behavior used by the sequencer-first engine
(`docs/tasks/sequencer-first-engine.md`). Every row: fact, source, status.
**Status:** `verified` = read on the cited page today; `CONFLICT` = the task spec says something different;
`unverified` = the docs are silent (must be measured or read from Nitro source before relying on it).
Docs index used: `https://docs.arbitrum.io/llms.txt`. Pages are cited by path under `docs.arbitrum.io`.

## Ordering: Priority Gas Auction (PGA)

| # | Fact | Source | Status |
|---|---|---|---|
| P1 | A round starts every `B/K`; on Arbitrum One block time `B` = 250 ms, rounds per block `K` = 2, so rounds are 125 ms. | `/how-arbitrum-works/priority-gas-auction/pga` | verified |
| P2 | Intake runs for the full round window and collects arrivals into an unordered waiting list. **At the start of each round the entire waiting list becomes a priority queue** keyed on priority fee; the Sequencer moves the queue into the block highest first. A tx that arrives during round R is therefore ordered at the start of R+1. | same | verified |
| P3 | `priority_fee_per_gas = min(max_priority_fee_per_gas, max_fee_per_gas − base_fee_per_gas)`. | same | verified |
| P4 | Anti-starvation boost: every tx still waiting gets `p / (2K)` added, `p` = priority of the last tx included in the previous round (0 if that round included none); it **compounds each round**, shifts queue position only, and does not change the fee charged. | same | verified. Spec wording ("boost only applies to txs that waited more than one round") is equivalent in effect: a tx is only boosted if it was left waiting. |
| P5 | Ties break by the arrival timestamp recorded when the tx first reached the Sequencer. | same | verified |
| P6 | The queue transfer ends when the queue empties, the block reaches 32 Mgas, hits the 95,000-byte calldata limit, or the round time expires. If a block fills early the Sequencer finalizes it and starts the next block's first round at once. | same | verified. Spec says "a round in progress cannot be interrupted": the docs describe early block finalization instead; treat round boundaries as "at most" 125 ms. |
| P7 | Participants bid with `maxPriorityFeePerGas` on a standard EIP-1559 (type 2) tx. The docs give **no** bidding or `maxFeePerGas`-headroom guidance. | same | verified (headroom advice is derived from P3, not documented) |
| P8 | Priority fees are split 97 % to the Arbitrum DAO treasury, 3 % to the Arbitrum Developer Guild. Timeboost's express lane and its 200 ms delay are decommissioned on PGA activation day. | same | verified |
| P9 | **Activation:** the page says "PGA activates on Arbitrum One some time after the onchain vote passes", i.e. it does not state a date. The repo observed tip-ordered blocks on-chain from late September (`docs/research/arbitrum-mev-live-intel-2026-10-06.md`, DECISIONS #59). | pga page; repo research | **unverified date.** Re-check on-chain before each phase that depends on ordering. |

## Fast Feed

| # | Fact | Source | Status |
|---|---|---|---|
| F1 | Publishes each tx "as soon as the Sequencer has ordered and executed it, before the block that contains it reaches the standard sequencer feed". | `/…/priority-gas-auction/fast-feed` | verified |
| F2 | Message = `FeedMessage { version (1), timestamp_ms, pga_round (optional), transaction { block_number (tentative), tx_index, raw_tx (hex RLP), tx_hash, receipt { status, gas metrics, contract address, event logs } } }`. No `block_hash`. Encoded as a JSON object inside a **binary** WebSocket frame; byte fields are hex strings. | same | verified. Field list for the receipt is summarized by the docs, not a full schema: confirm exact receipt keys against a live message before writing the log decoder. |
| F3 | Does not publish state or state diffs; caches nothing (no replay/history). If the Sequencer fails mid-block, already-published txs are lost. | same | verified |
| F4 | Access is by tickets sold by the `Tickets` contract `0x451DD62C26b753d07d6104aaC6865AaE73a6A598` (proxy); paid in USDG `0x004B506865409877C9fA29bfb1ebA929984B9bbC` (6 decimals). **Minimum 17 USDG per ticket**, one ticket valid for **one 24 h round**; 100 target / 200 max tickets per round; first round started 2026-09-23 17:00 UTC. | `/…/use-fast-feed`, fast-feed page | verified |
| F5 | Endpoint `wss://arb1-txfeed.arbitrum.io/`. Auth: `Authorization: Bearer <raw api key>` on the upgrade (the endpoint Keccak-256 hashes it and checks active tickets); upgrade must complete within 5 s; client sends only ping/pong. **One connection per ticket**; more connections than tickets → HTTP 429. | use-fast-feed | verified |
| F6 | A client that lags more than **2048 messages** is closed with code `1013` ("client lagged"): the reader must drain faster than the feed arrives. | use-fast-feed | verified |
| F7 | To stay connected across rounds, buy a ticket for the next round before `roundEnd` with the same API-key hash. | use-fast-feed | verified |
| F8 | **Cost:** at the 17 USDG floor that is ≥ $17/day ≈ **$510/month per connection** (two connections for redundancy ≈ $1,020). The project budget is $200 capital and ~$0/month infra (`CLAUDE.md`). | derived from F4 | **CONFLICT with the project budget; needs a human decision (see design doc §13).** |

| F9 | **Grandfather period:** "Each round opens with a grandfather period that covers about half its length. During that period, only subscribers from the previous round can buy, up to the number of tickets they held." On chain: `grandfatherPeriodFraction()` = 128 (of 256), `roundDuration()` = 86400, `roundStart()`/`roundEnd()`. A first-time buyer therefore buys in the second half of a round. | fast-feed page; Tickets contract reads 2026-10-08 | verified (docs + chain) |
| F10 | Pricing: EIP-4844 style, target 100 / max 200 tickets per round, floor 17 USDG, `priceUpdateFraction` 144 (next round's price within about half to double of this one). Round length, capacity, price parameters and the grandfather fraction are controlled by the Arbitrum DAO and change at round boundaries. | fast-feed page; contract reads | verified |
| F11 | Tickets are tracked per buyer **address**: `thisRoundTicketCount(address)`, `grandfatherCount(address)`, `tokenBalance(address)`; the API-key hash is supplied at purchase. A **smart contract can be the subscriber** (fork-tested: deposit, `purchaseTickets`, next-round renewal). | Tickets contract bytecode selectors; `contracts/test/FastFeedTicketVault.t.sol` | verified on a fork |
| F12 | Foundry 1.8.4 has no environment variable for a raw private key (`--private-key` only); it supports keystores (`--keystore`, `--account`, `--password-file`). Do not set `ETH_PASSWORD` globally: it makes non-signing `cast` calls demand a keystore. | `cast send --help`, tests | verified |

| F13 | **Market so far** (all 209 events of the Tickets contract, rounds 0-14, read 2026-10-08): 102 purchases by 23 buyers; usage grew 2 to 12 tickets/round against a target of 100, so the price has stayed at the 17 USDG floor and no round has sold out; every purchase was exactly 1 ticket and a buyer used at most one key hash per round; 58 of 102 purchases were within the first 10 minutes of a round (median 0.2 % in), 81 % of renewals in the grandfather half, 96 % of first-time purchases in the second half; nobody who skipped a round ever came back; 5 of 23 sampled buyers are contracts; median deposit 17 USDG (just-in-time). | `eth_getLogs` on the Tickets contract (`TicketsPurchased`, `TokensDeposited`) | verified (chain) |
| F14 | **Tickets behaviour found by fork tests:** buying 3 tickets in one call works and costs exactly 3 x price; in the grandfather window asking for more than last round's holding **reverts the whole purchase** with `NotEnoughGrandfatheredTickets(uint256,uint256)` (selector `0x1b350bf6`), while in the second half a larger ask succeeds; `setPricingParams(uint64 minimumPrice, uint40 priceUpdateFraction, uint56)` (the first argument is the floor) takes effect at the next round; `setRoundDuration(uint24)` works for the params-setter role, not for the DEFAULT_ADMIN contract; the round number follows the clock (it is already N+1 after `roundEnd`). | `contracts/test/FastFeedTicketVault.t.sol` | verified on a fork |
| F15 | **Governance:** the proxy admin, `DEFAULT_ADMIN_ROLE` holder and the pricing-params role holder are all contracts (DAO executors); the proxy emitted `Upgraded` once (implementation `0x727152b1...`). | storage slots / role reads | verified (chain) |
| F16 | **Where USDG liquidity is** (read 2026-10-08): supply 26.98 M; **no** USDG pool on Uniswap V3, Camelot V3 or Pancake V3 (every fee tier vs USDC, WETH, USDT0, DAI, ARB). Uniswap V4 has 7 USDG pools; the only deep one is the **hooked, dynamic-fee USDG/USDC** pool (fee flag `0x800000`, tick spacing 1, hook `0x630e9cac0fff557bd3a01daa111ed253c771bcc0`, 135 bytes of code). Real swaps from a contract through `unlock`: 1 USDC costs 1.44 bps, 100 USDC 1.44, 10,000 USDC 1.59, **100,000 USDC 3.2 bps**. The hookless USDG/USDC pools (fee 9, fee 30000) have no active liquidity. The native-ETH/USDG V4 pool works for about $0.26 and reverts `NotEnoughLiquidity` at 0.001 ETH, so ETH must route ETH -> WETH -> USDC (Uniswap V3 0.05 %) -> USDG. Chainlink ETH/USD `0x639Fe6ab...` (8 decimals, 24 h heartbeat) and the L2 sequencer-uptime feed `0xFdB631F5...` are live; USDC/USD reads 0.99976. | V4 `Initialize` logs, `extsload`, V4 quoter, `tests/UsdgConverter.t.sol` | verified (chain + fork) |

## Standard sequencer feed

| # | Fact | Source | Status |
|---|---|---|---|
| S1 | `BroadcastMessage { version, messages: [BroadcastFeedMessage], confirmedSequenceNumberMessage }`; each `BroadcastFeedMessage { sequenceNumber, message (MessageWithMetadata), signature, blockMetadata }`. Transactions are in `l2Msg` (decode with `ParseL2Transactions`). | `/run-a-node/sequencer/read-sequencer-feed` | verified |
| S2 | **L2 block number = sequence number + 22207817** on Arbitrum One (Nova: + 0). The `blockNumber` in the message header is the **L1** block number, not L2. | same | verified |
| S3 | Free, per-block granularity, supports chain following. Public URL `wss://arb1-feed.arbitrum.io/feed`; the repo uses `wss://arb1.arbitrum.io/feed`. A local relay serves port 9642. | same; `/arbitrum-essentials/reference/node-providers` | verified (both hostnames are in use; pick one and record RTTs) |
| S4 | The standard feed carries **transactions, not execution results**: no receipts or logs. State changes from it need local execution or node reads. | read-sequencer-feed (no receipt fields) vs F2 | verified |

## Submission and RPC

| # | Fact | Source | Status |
|---|---|---|---|
| R1 | Public sequencer endpoint `https://arb1-sequencer.arbitrum.io/rpc`: "a best-effort public endpoint with no formal SLA". The docs advise a third-party provider as primary, or as fallback when direct submission fails. | `/arbitrum-essentials/reference/node-providers` | verified. **CONFLICT in emphasis** with the spec's "submit directly to the Sequencer": direct is possible but explicitly unguaranteed; keep multi-endpoint broadcast, label each endpoint. |
| R2 | An RPC node "does not execute or pool the transaction; it forwards it to the Sequencer". | `/how-arbitrum-works/deep-dives/sequencer-transaction-flow` | verified |
| R3 | `eth_sendRawTransactionConditional(tx, { knownAccounts: { addr: storageRootHash \| { slot: value } }, blockNumberMin/Max (an **L1** block-number window), timestampMin/Max (L2 timestamp) })`. | `/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods` | verified. The spec's claims that the check uses "recent past state (~1–2 s)" and is "rate-limited" are **unverified** (the docs do not say). Measure in Phase 12. |
| R4 | `arb` and `debug` namespaces exist; `debug_traceTransaction` supports a `stylusTracer`. | rpc-methods | verified |
| R5 | NodeInterface is at `0xc8`, reachable only through `eth_call` (not deployed on chain). `gasEstimateL1Component(address to, bool contractCreation, bytes data)` returns the L1 cost component; `gasEstimateComponents` is native estimation plus L1 info. | `/arbitrum-essentials/nodeinterface/reference` | verified |
| R6 | ArbGasInfo `0x6c`: `getL1BaseFeeEstimate()`, `getPricesInWei()`, `getMinimumGasPrice()`, `getL1PricingUnitsSinceUpdate()` (ArbOS 20+). | `/arbitrum-essentials/precompiles/reference` | verified |

## EVM differences

| # | Fact | Source | Status |
|---|---|---|---|
| E1 | `ArbSys` is at `0x64`. `function arbBlockNumber() external view returns (uint256)` returns the Arbitrum (L2) block number "distinct from L1 block number"; `function arbBlockHash(uint256) external view returns (bytes32)` reverts unless within the last 256 blocks. Solidity `block.number` is the L1 number approximation. | `OffchainLabs/nitro-precompile-interfaces` `ArbSys.sol`; rpc-methods `l1BlockNumber` note | verified (interface source); the `block.number` text is not on the precompile pages |
| E2 | Stylus programs carry the EOF-style prefix `EFF00000` in their deployed bytecode discriminant; activation goes through `ArbWasm` at `0x71` (`activateProgram(address) returns (uint16 version, uint256 dataFee)`), is not reversible, and can expire (renewed by `codehashKeepalive`). | `/stylus/concepts/activation` | verified for the prefix. **Open:** whether `eth_getCode` returns that prefix on a live Stylus contract: test on chain in Phase 5. |
| E3 | Total tx cost is `effective_gas_price × gas_used`, where `gas_used` includes the L1 component (the Fast Feed receipt carries `gas_used_for_l1`). Reverted txs are included and pay for gas used. | spec §1; Fast Feed receipt fields (F2) | verified in part (receipt fields); fee arithmetic is standard EIP-1559 |
| E4 | Stock REVM has no Arbitrum precompiles. The repo's local simulator only *skips fetching* `0x64..` addresses (`crates/mev-localsim/src/lib.rs:138`); it does not implement them. | repo | verified (repo) |

## Open questions to settle by measurement (not docs)

1. Exact Fast Feed receipt JSON keys and log shape (needs one live ticket; human action, costs ≥ 17 USDG).
2. Fast Feed arrival time vs standard feed arrival time per tx (the central latency question; see design §13, experiment 0).
3. Conditional-tx acceptance, latency and any rate limit (R3).
4. Whether Stylus code carries `EFF00000` via `eth_getCode` (E2).
5. Round-start phase relative to our receive time (what fraction of our sends arrive before the next round starts).
