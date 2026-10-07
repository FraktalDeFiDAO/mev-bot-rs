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
