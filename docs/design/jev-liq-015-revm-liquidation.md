# JEV-LIQ-015 - REVM exact-call liquidation simulation and Aave fork evidence

Date: 2026-10-13. Branch `feat/liq-revm-liquidation` (base `feat/liq-jev-resilience`). Spec: `docs/copper_jev_liquidation_architecture.md` sections 0 (inv. 4), 3, 4, 10, 13. No live dispatch decision changed.

## 1. What was added

| Where | What |
|---|---|
| `crates/mev-localsim/src/liquidation.rs` | `LiquidationSimulator<S: StateSource>`; `SimulationRequest`, `SimulationResult`, `Refusal`, `Status`, `RevertReason`, `L1DataFee`, `EthCallOutcome`, `Mismatch`, `compare`, `aave_error_name`; `RpcStateSource` (EIP-1898 hash-pinned reads + `eth_call`/`estimate_gas` baseline + optional `NodeInterface` L1 component). Reuses `StateCache`/`SnapshotDb` and REVM 43 of `mev-localsim`; no new simulator crate, no revm/alloy bump. |
| `crates/mev-bindings` | `FinalCalldata{bytes,hash}` + `final_liquidation_calldata(plan,hops,min_profit,aave_pool)`. |
| `crates/mev-exec` | `liquidation_final_calldata(..)`; `Sender::send_liquidation` and `send_liquidation_fast` now build through it (same bytes as before). |
| `crates/mev-engine` | `Engine::revm_shadow_compare` (shadow only, spawned task, event `liq_revm_compare`). |
| `crates/mev-config` | `[liquidation] revm_shadow` (default false). |
| `crates/mev-sim` | `Simulator::executor()/owner()` accessors. |
| `scripts/test-revm-liquidation-fork.sh`, `crates/mev-localsim/tests/liquidation_fork.rs` | anvil-fork harness and the ignored network tests. |

## 2. API contract

`SimulationRequest { block_pin: (number, hash), calldata, calldata_hash, from, to, value, gas_limit, expiry }`
-> `SimulationResult { input_state_id (= block hash), block_number, calldata_hash, status: Success{output} | Revert{data, reason} | Halt, gas_used, l1_data_fee, executor_reported_profit, state_reads }`.

- A request whose `calldata_hash != keccak(calldata)` or `now > expiry` is a `Refusal`, not a result. A revert is a result.
- Every state read is `eth_getBalance/TransactionCount/Code/StorageAt` with `{"blockHash":..,"requireCanonical":true}`. State is fetched lazily: run, collect misses, fetch (16 concurrent), rerun, until a run misses nothing (`MAX_ROUNDS` 64 else `TooManyRounds`).
- Refusals: `UnknownBlock`, `PinNumberMismatch`, `MixedHeight` (header or any read answered for another hash), `Reorged` (canonical hash at the pinned number differs after the reads), `StaleState`/`CalldataMismatch` (`check_fresh`, `check_binds`).
- Block env is the pinned block's own (number, timestamp), as `eth_call` at that block does. Gas price 0, balance/nonce checks off (like `eth_call`).
- `gas_used` is EVM execution gas only. `l1_data_fee` is `NotIncluded` unless `RpcStateSource::with_l1_component` is set, then `NodeInterface{gas_estimate_for_l1, base_fee, l1_base_fee_estimate, at}` from `gasEstimateL1Component(to,false,calldata)` at the pinned hash. It is never added to `gas_used`.
- `executor_reported_profit` = the returned 32-byte word on success (ArbExecutor entrypoints return `uint256 profit`); `None` otherwise.
- Integer math only; no `f64` in the module.

### Arbitrum L1 data fee: what was verified (2026-10-13)
docs.arbitrum.io NodeInterface reference lists `gasEstimateL1Component(address to, bool contractCreation, bytes data)` and `gasEstimateComponents(...)` but not their return values (page body). Return values taken from `OffchainLabs/nitro-contracts` `NodeInterface.sol` ("Use eth_call to call"): `gasEstimateL1Component -> (uint64 gasEstimateForL1, uint256 baseFee, uint256 l1BaseFeeEstimate)`; `gasEstimateComponents -> (uint64 gasEstimate, uint64 gasEstimateForL1, uint256 baseFee, uint256 l1BaseFeeEstimate)`. Recorded as R5a in `docs/arbitrum-facts.md`. NOT verified: that public providers honour a hash-pinned `eth_call` to `0xc8`, and the numeric result: the code path exists and compiles but was **not exercised against a live node** in this task (anvil has no NodeInterface). Until it is, callers must treat `NotIncluded` as "L1 cost unknown", and profit-after-L1 is not established by this module.

## 3. Calldata parity (gap 5)

Before: `on_plan` simulated `liquidation_calldata(.., min_profit = 0, ..)` but sent the floored `min_profit`, so the simulated and signed bytes differed (different `minProfit` argument, different hash).

Now: `mev_exec::liquidation_final_calldata` is the single builder; `send_liquidation(_fast)` sign its `.bytes`; the shadow branch of `on_plan` builds `FinalCalldata` with the shadow floor, hands those exact bytes to `revm_shadow_compare` and to `shadow_candidate`. The event field `calldata_parity` is `keccak(simulated bytes) == fin.hash`; unit test in `mev-exec` asserts `final.hash == keccak(bytes sent)` and that `min_profit = 0` hashes differently.
Not changed: the existing `eth_call` simulation (still `min_profit = 0`, still the dispatch gate), the live branch, batch items. The floor is computed from the eth_call's gas, so the final calldata cannot exist before that first simulation; the parity guarantee is therefore "REVM simulates exactly the bytes that will be signed", not "the existing gate simulated them". Making the gate itself use the final bytes is a live-decision change and was left for the owner.

Shadow flag: `[liquidation] revm_shadow = true` (shadow mode only): one task per planned liquidation pins the latest block, runs REVM and `eth_call` for the same bytes at that hash and logs `liq_revm_compare` with `mismatch`, `mismatches`, `refused`, `calldata_parity`, gas and profit of both sides. Nothing is dispatched from it. It was **not** run against the live feed in this task.

## 4. Fork evidence (all measured; anvil 1.8.4, forge toolchain container)

Aave deployment under test: Pool proxy `0x794a..14aD`, implementation `0xf05fd3cc911b4c5e36e53c00354f645e22922c9a` (`L2PoolInstance`, solc 0.8.27, Sourcify full match; source read from Sourcify).

### Rules read from the deployed source (`LiquidationLogic.sol`)
- Eligibility: `HealthFactorNotBelowThreshold()` (selector `0x930bb771`, observed on the fork) when HF >= 1e18.
- Close factor: whole reserve debt is liquidatable by default. Only if collateral-reserve value AND debt-reserve value are each >= `MIN_BASE_MAX_CLOSE_FACTOR_THRESHOLD` (2000e8) AND `healthFactor > CLOSE_FACTOR_HF_THRESHOLD` (0.95e18) is the amount capped at `DEFAULT_LIQUIDATION_CLOSE_FACTOR` (50 %) of **total** debt (in base currency) when the reserve debt exceeds that. So: HF in (0.95, 1) and big positions -> 50 %; HF <= 0.95 or small positions -> 100 %.
- This version uses custom errors, not the old numeric strings ("45"); `aave_error_name` maps the relevant ones from the verified `Errors.sol`.

### (a) Real borrower, REVM vs block-pinned eth_call (public RPC, `real_borrowers_revm_account_data_equals_block_pinned_eth_call`)
Result: **pass**. Pinned blocks 514533835, 514533873, 514533907 (re-pinned per user). 3 real borrowers from `Borrow` logs; REVM `getUserAccountData` output byte-identical to `eth_call` at the same block hash (e.g. hf 2173660532317826434 / debt_base 3327696269698 / collateral_base 9273438519210). 59-73 lazy state reads per account, ~170-205k gas, ~12 s each over the public node. Honest note: a first run pinned one block for three users and the third failed with `historical state ... not available` - public nodes keep only a short window of recent state; no archive access was assumed or used.

### (b) Eligibility boundary (anvil fork at 514537429, `anvil_eligibility_boundary_is_exact_in_u256`)
Result: **pass**. WETH price source replaced with `MockPriceSource` (as in `scripts/test-e2e-liquidation.sh`); binary search over the integer price with a frozen block timestamp found the exact boundary (price 236157413513 -> HF >= 1e18; 236157413512 -> HF < 1e18). At block 514537553: HF 999999999998064245 (< 1e18) and the exact ArbExecutor call succeeds; at block 514537554: HF 1000000000002298708 and the same call reverts `HealthFactorNotBelowThreshold`. REVM == anvil eth_call in both account data and executor-call outcome. Near-threshold REAL account: not searched (public RPC sweep not done); only the synthetic boundary was tested.
Close factor, via mined liquidations on the fork (`anvil_close_factor_and_exact_calldata_revm_vs_eth_call_vs_receipt`; position ~$9.9k debt / $12.5k collateral, above the 2000e8 thresholds): HF 0.999999999998 -> **49.99 %** of debt repaid whether the flash/`debtToCover` was 100 % or 50 % of debt; HF 0.900000000003 -> **100.00 %**. Matches the source rule.
Not tested: dust/isolation/eMode accounts, multiple debt reserves, fee changes, aToken transfers (spec 13 list) - out of scope here.

### (c) Exact calldata through ArbExecutor (`liquidateViaAave`, final calldata built by `final_liquidation_calldata`)
Result: **0 mismatches** in three scenarios at pin 514537514 (revm vs anvil `eth_call` at the pinned hash vs mined receipt in a block with the pin's timestamp):

| scenario | REVM profit | receipt receiver delta | REVM gas | receipt gas | `estimate_gas` |
|---|---|---|---|---|---|
| HF~1.0-, flash 100 % debt | 528999808 | 528999808 | 550939 | 550939 | 657385 |
| HF~1.0-, flash 50 % debt | 531460719 | 531460719 | 550958 | 550958 | 681202 |
| HF~0.90, flash 100 % debt | 2283388690 | 2283388690 | 560658 | 560658 | 667574 |

Caveats: anvil runs Ethereum gas semantics, so equal gas shows REVM == a plain EVM, not == Arbitrum ArbOS (no L1 poster component, no Arbitrum precompiles; `estimate_gas` is higher because of search margin, and on Arbitrum because of the L1 component). To get equal receipts the mined block reused the pin's timestamp (`evm_setNextBlockTimestamp`); with a later timestamp interest accrual makes profits differ by dust, which is expected, not a defect. An oversized flash (100 % when only 50 % is closable) yields ~2.46 USDC less profit than the exact 50 % flash - the planner's flash amount matters to net profit.
Not verified on real Arbitrum: REVM vs a real mined liquidation receipt (no historical archive state on public RPCs; no live tx was signed).

### (d) Stale opportunity (`anvil_stale_simulation_is_refused_after_the_state_moves`)
Result: **pass**. Simulation succeeds at pin A; the price moves one block later; `check_fresh(new pin)` returns `StaleState{simulated: A, current: B}`; re-simulating the same bytes at B reverts `HealthFactorNotBelowThreshold`.

### Non-network unit tests (8 in `liquidation.rs`)
Lazy fetch + executor word; revert-as-result with decoded reason; revert decoding (Error/Panic/custom/empty/malformed/truncated, Aave selector); hash binding and expiry; mixed-height refusal (lying slot reader, lying header); unknown/reorged pin; stale refusal; comparison flags.

## 5. Limits (not claimed)
- No long-range historical replay: public RPCs keep minutes of state. 016 needs an archive source (or own node) to label historical checkpoints; this task proves the mechanism on recent blocks and anvil forks.
- Arbitrum specifics REVM does not model: ArbOS precompiles (0x64.. range returns empty), L1 `NUMBER`, `BLOCKHASH` (zero), L1 poster gas. Chainlink/Aave paths worked without them.
- Per-read pinning: a provider is trusted to honour EIP-1898; the module detects a wrong header, a wrong claimed hash and a post-read reorg, but cannot see inside a node that silently serves other state.
- 80+ sequential RPC round trips per simulation on public nodes: fine for shadow evidence, not a latency-grade hot path (needs the diff-fed `StateCache` or a local node).

## 6. What task 016 (ground-truth labels) must consume
- Eligible label: `getUserAccountData` HF as `U256` < `10^18` from `SimulationResult.status` output at a `BlockPin` (`input_state_id`), plus `Status::Revert` reason `HealthFactorNotBelowThreshold` as the contract-level refusal (not hf_bps).
- Actionable label: `SimulationResult{success, gas_used, l1_data_fee, executor_reported_profit}` of the exact final calldata; net profit = profit - gas*price - L1 component. Treat `L1DataFee::NotIncluded` as "actionable not established".
- Always carry `input_state_id`, `block_number`, `calldata_hash`; drop any label whose result fails `check_binds`/`check_fresh` against the checkpoint.
- Close factor: 50 % only if HF in (0.95e18, 1e18) and both reserve values >= 2000e8; the label code must not assume 50 %.
- A `Refusal` is "unknown", never "not eligible".

## 7. Reproduce
```
scripts/dev.sh env CARGO_INCREMENTAL=0 cargo test -q -p mev-localsim --lib liquidation
scripts/dev.sh env CARGO_INCREMENTAL=0 cargo test -q -p mev-localsim --test liquidation_fork -- --ignored --nocapture real_borrowers
scripts/dev.sh bash scripts/test-revm-liquidation-fork.sh        # anvil fork tests (b),(c),(d)
```
