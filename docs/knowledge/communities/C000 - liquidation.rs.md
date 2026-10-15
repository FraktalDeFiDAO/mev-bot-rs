---
graphify-generated: true
graphify-community: "0"
graphify-node-count: 111
graphify-internal-edges: 336
tags:
  - graphify/community
---

# liquidation.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-localsim/src/liquidation.rs` — 57 nodes
- `graph` — 33 nodes
- `crates/mev-localsim/tests/liquidation_fork.rs` — 21 nodes

## Nodes (111)

- **liquidation.rs** — `crates/mev-localsim/src/liquidation.rs:L1`
- **B256** — `graph`
- **.simulate()** — `crates/mev-localsim/src/liquidation.rs:L565`
- **Fixture** — `crates/mev-localsim/src/liquidation.rs:L766`
- **Result** — `graph`
- **Fork** — `crates/mev-localsim/tests/liquidation_fork.rs:L124`
- **.new()** — `crates/mev-localsim/src/liquidation.rs:L561`
- **SimulationRequest** — `crates/mev-localsim/src/liquidation.rs:L62`
- **Refusal** — `crates/mev-localsim/src/liquidation.rs:L120`
- **RpcStateSource** — `crates/mev-localsim/src/liquidation.rs:L356`
- **.run()** — `crates/mev-localsim/src/liquidation.rs:L667`
- **anvil_close_factor_and_exact_calldata_revm_vs_eth_call_vs_receipt()** — `crates/mev-localsim/tests/liquidation_fork.rs:L371`
- **liquidation_fork.rs** — `crates/mev-localsim/tests/liquidation_fork.rs:L1`
- **SimulationResult** — `crates/mev-localsim/src/liquidation.rs:L253`
- **BlockPin** — `crates/mev-localsim/src/liquidation.rs:L56`
- **.account_data()** — `crates/mev-localsim/tests/liquidation_fork.rs:L194`
- **.fetch()** — `crates/mev-localsim/src/liquidation.rs:L651`
- **req()** — `crates/mev-localsim/src/liquidation.rs:L842`
- **U256** — `graph`
- **.set_price_and_mine()** — `crates/mev-localsim/tests/liquidation_fork.rs:L166`
- **Option** — `graph`
- **.account()** — `crates/mev-localsim/src/liquidation.rs:L789`
- **Address** — `graph`
- **anvil_eligibility_boundary_is_exact_in_u256()** — `crates/mev-localsim/tests/liquidation_fork.rs:L316`
- **block_on()** — `crates/mev-localsim/src/liquidation.rs:L854`
- **compare()** — `crates/mev-localsim/src/liquidation.rs:L524`
- **stale_opportunity_is_refused_after_the_state_moves()** — `crates/mev-localsim/src/liquidation.rs:L980`
- **.account()** — `crates/mev-localsim/src/liquidation.rs:L437`
- **.at()** — `crates/mev-localsim/src/liquidation.rs:L374`
- **.load()** — `crates/mev-localsim/tests/liquidation_fork.rs:L137`
- **.slot()** — `crates/mev-localsim/src/liquidation.rs:L457`
- **.slot()** — `crates/mev-localsim/src/liquidation.rs:L799`
- **account_data_req()** — `crates/mev-localsim/tests/liquidation_fork.rs:L54`
- **AccountState** — `crates/mev-localsim/src/liquidation.rs:L311`
- **Answered** — `crates/mev-localsim/src/liquidation.rs:L319`
- **anvil_stale_simulation_is_refused_after_the_state_moves()** — `crates/mev-localsim/tests/liquidation_fork.rs:L509`
- **calldata_hash_is_bound_and_expiry_enforced()** — `crates/mev-localsim/src/liquidation.rs:L919`
- **EthCallOutcome** — `crates/mev-localsim/src/liquidation.rs:L500`
- **h()** — `crates/mev-localsim/src/liquidation.rs:L761`
- **lazily_fetches_pinned_state_and_reports_executor_word()** — `crates/mev-localsim/src/liquidation.rs:L862`
- **now()** — `crates/mev-localsim/tests/liquidation_fork.rs:L30`
- **U256** — `graph`
- **.boundary_price()** — `crates/mev-localsim/tests/liquidation_fork.rs:L213`
- **.header_by_hash()** — `crates/mev-localsim/src/liquidation.rs:L776`
- **.l1_data_fee()** — `crates/mev-localsim/src/liquidation.rs:L469`
- **.liquidation_calldata()** — `crates/mev-localsim/tests/liquidation_fork.rs:L260`
- **.new()** — `crates/mev-localsim/src/liquidation.rs:L78`
- **comparison_flags_every_disagreement()** — `crates/mev-localsim/src/liquidation.rs:L998`
- **pin_number()** — `crates/mev-localsim/tests/liquidation_fork.rs:L46`
- **real_borrowers_revm_account_data_equals_block_pinned_eth_call()** — `crates/mev-localsim/tests/liquidation_fork.rs:L68`
- **revert_is_a_result_with_decoded_reason()** — `crates/mev-localsim/src/liquidation.rs:L876`
- **Status** — `crates/mev-localsim/src/liquidation.rs:L230`
- **.canonical_hash_at()** — `crates/mev-localsim/src/liquidation.rs:L786`
- **.check_binds()** — `crates/mev-localsim/src/liquidation.rs:L275`
- **.check_fresh()** — `crates/mev-localsim/src/liquidation.rs:L292`
- **.header_by_hash()** — `crates/mev-localsim/src/liquidation.rs:L417`
- **.hf_at_price()** — `crates/mev-localsim/tests/liquidation_fork.rs:L249`
- **.rpc()** — `crates/mev-localsim/tests/liquidation_fork.rs:L153`
- **Bytes** — `graph`
- **check_height()** — `crates/mev-localsim/src/liquidation.rs:L747`
- **Fetch** — `crates/mev-localsim/src/liquidation.rs:L742`
- **HeaderInfo** — `crates/mev-localsim/src/liquidation.rs:L304`
- **L1DataFee** — `crates/mev-localsim/src/liquidation.rs:L238`
- **pin_back()** — `crates/mev-localsim/tests/liquidation_fork.rs:L41`
- **refuses_an_unknown_or_reorged_pin()** — `crates/mev-localsim/src/liquidation.rs:L963`
- **RevertReason** — `crates/mev-localsim/src/liquidation.rs:L170`
- **StateSource** — `crates/mev-localsim/src/liquidation.rs:L325`
- **String** — `graph`
- **.canonical_hash_at()** — `crates/mev-localsim/src/liquidation.rs:L429`
- **.eth_call()** — `crates/mev-localsim/src/liquidation.rs:L379`
- **.price_for_hf()** — `crates/mev-localsim/tests/liquidation_fork.rs:L233`
- **.usdc()** — `crates/mev-localsim/tests/liquidation_fork.rs:L301`
- **.validate()** — `crates/mev-localsim/src/liquidation.rs:L100`
- **DynProvider** — `graph`
- **LiquidationSimulator** — `crates/mev-localsim/src/liquidation.rs:L555`
- **LiquidationSimulator<S>** — `crates/mev-localsim/src/liquidation.rs:L560`
- **Mismatch** — `crates/mev-localsim/src/liquidation.rs:L508`
- **provider()** — `crates/mev-localsim/tests/liquidation_fork.rs:L37`
- **refuses_when_the_provider_answers_for_another_block()** — `crates/mev-localsim/src/liquidation.rs:L942`
- **.fmt()** — `crates/mev-localsim/src/liquidation.rs:L162`
- **.new()** — `crates/mev-localsim/src/liquidation.rs:L363`
- **Address** — `graph`
- **decode_revert()** — `crates/mev-localsim/src/liquidation.rs:L182`
- **DynProvider** — `graph`
- **env()** — `crates/mev-localsim/tests/liquidation_fork.rs:L116`
- **Self** — `graph`
- **Vec** — `graph`
- **.provider()** — `crates/mev-localsim/src/liquidation.rs:L370`
- **aave_error_name()** — `crates/mev-localsim/src/liquidation.rs:L222`
- **Key** — `graph`
- **S** — `graph`
- **.success()** — `crates/mev-localsim/src/liquidation.rs:L270`
- **BlockId** — `graph`
- **Display** — `graph`
- **Error** — `graph`
- **F** — `graph`
- **Formatter** — `graph`
- **getUserAccountDataReturn** — `graph`
- **HashMap** — `graph`
- **Mutex** — `graph`
- **MutexGuard** — `graph`
- **Output** — `graph`
- **revert_decoding()** — `crates/mev-localsim/src/liquidation.rs:L891`
- **Self** — `graph`
- **Send** — `graph`
- **String** — `graph`
- **Sync** — `graph`
- **T** — `graph`
- **T** — `graph`
- **Value** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C054 - transport.rs|transport.rs]] — 7 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 2 cross-community edges
- [[docs/knowledge/communities/C002 - StateCache|StateCache]] — 1 cross-community edges
- [[docs/knowledge/communities/C009 - audit.rs|audit.rs]] — 1 cross-community edges
