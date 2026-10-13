# JEV-LIQ-001 — lending, liquidation, simulation, execution inventory

Read-only evidence at `/tmp/mev-jev-liq-001-audit`, HEAD `c6f3aa466fb1230424a0dfb41b65e09b5084d72e` (clean when inspected). Second layer inspected through `git show feat/liq-scheduler:<path>` at `d0f7b52`. All unqualified path:line references below refer to **c6f3aa4**; references prefixed **stack:** refer to **d0f7b52**, not the audit checkout. Root `graphify-out/graph.json` was used only for navigation; checkout source verified each finding. Read both Copper documents in full, CLAUDE.md, planning index, context policy, sequencer invariants/spec/progress. No source edits, git mutations, chain calls, live tests, keys or `.env` reads performed.

## Existing production owners and callgraph (c6f3aa4)

```text
Engine::bootstrap (mev-engine/src/lib.rs:387,407)
  -> build_liquidation (:3029)
     -> AdapterRpc { fast=rpc.http; separate scan/sweep providers } (:3040)
     -> AaveV3Adapter::new (mev-lending-aave-v3/src/lib.rs:81)
     -> Runner::new (mev-liquidate/src/lib.rs:135)
  -> tokio::spawn(runner.run(liq_feed, tx)) (mev-engine/src/lib.rs:839)

Runner::run (mev-liquidate/src/lib.rs:492)
  -> index_all (:237) -> LiquidationAdapter::index (mev-lending/src/lib.rs:71)
     -> AaveV3Adapter::index (:426) -> EventIndex::step (mev-lending-index/src/lib.rs:94)
  -> sweep_all (:269) -> adapter.accounts + adapter.health(fast=false)
     -> AaveV3Adapter::health (:445) -> getUserAccountData multicall
     -> refresh_user_masks (:187), refresh_snapshot (:343)
     -> apply_sweep (mev-liquidate/src/lib.rs:303) -> at_risk HashMap
  -> fallback/burst poll (:341) -> adapter.health(fast=true)
     -> Health::liquidatable (mev-lending/src/lib.rs:38)
     -> AaveV3Adapter::plan (:480)
        -> reserves_info (:226), position (:273), math::plan (math.rs:74)
        -> build_plan (:375) -> stamp_plan (mev-liquidate/src/lib.rs:175)
        -> RunnerEvent::Plan
  -> on_feed_tx (:404) / on_hint (:458)
     -> adapter.predict -> RunnerEvent::Backrun / AtlasBackrun

Engine event loop (mev-engine/src/lib.rs:1196)
  -> on_runner_event (:2603)
     -> on_plan (:2823) -> liq_precheck (:2628) -> route (:1405)
        -> Simulator::simulate_liquidation (mev-sim/src/lib.rs:107)
        -> risk.check / pga_bid / admit (mev-engine/src/accounting.rs:11)
        -> final_profit_floor (mev-engine/src/lib.rs:1323)
        -> Sender::send_liquidation (mev-exec/src/lib.rs:188)
     -> on_backrun (:2686) -> same precheck/risk/admission, without simulation
        -> Sender::send_liquidation_fast (mev-exec/src/lib.rs:205)
     -> on_atlas_backrun for auction plans (:2611; separate Atlas owner)
  -> optional batch::enqueue/flush (batch.rs:139,182,226,236)

Sender -> liquidation_calldata (mev-bindings/src/lib.rs:319)
  -> liquidate / liquidateViaAave -> sign_and_broadcast (mev-exec/src/lib.rs:270)
  -> configured reader + extra broadcast endpoints, same signed envelope
  -> wait_final (:416) -> TxDone -> settle_receipt (mev-engine/src/accounting.rs:48)
```

All Rust paths above are under `crates/`. Reuse these owners; no second borrower scanner, simulator, route finder, sender or nonce manager is needed.

## Borrower discovery and Aave integration

* `EventIndex` owns `(B256 market, Address account)` and JSON `State { next_block, back_block, BTreeSet<Key> }` (`mev-lending-index/src/lib.rs:21,42,51`). First step initializes both cursors at `head + 1`, scans forward, then history backward; max requests and adaptive range shrink bound each pass (:94,141). It sleeps between requests and writes the whole cache with `std::fs::write` (:185), without atomic rename/fsync. An unreadable/unparseable cache silently starts default (:59).
* Aave config supplies Pool, data provider, Oracle, deployment start and scan step (`mev-config/src/lib.rs:109`; `config/arbitrum.toml:54`). Checked-in values are Pool `0x794a...14aD`, data provider `0x243A...c43b`, Oracle `0xb56c...C7C7`, start block `7700000`; these are **configuration observations**, not independently verified chain facts.
* `AaveV3Adapter::new` scans **only Pool Borrow** (:84), extracts debt owner `onBehalfOf` at topic 2 (:426), and uses `aave_v3_borrowers.json` (engine :3057). `backfilled()` means initialized numeric cursor reached configured start (:88); it does not prove complete responses or correct start height. All ever-discovered accounts remain in the root index, including repaid borrowers (:441). No block hashes, removed-log validation, reorg rewind, explicit coverage gaps or chain-id namespace occur in this root index.
* Startup discovers reserves, ERC20 decimals, Oracle sources, reserve ids and source-to-aggregator mappings (`mev-lending-aave-v3/src/lib.rs:94–180`). Pool/Oracle/data-provider values are used directly; root has no PoolAddressesProvider resolution or deployment/code/ABI pinning. Bindings are locally declared typed subsets (`mev-bindings/src/lib.rs:91,101,108`), and liquidationCall is declared in the adapter (:33).
* Oracle reverse filtering exists: aggregator -> reserve mask/assets plus at-risk borrower bitmap (`lib.rs:61,187,503`). Unknown bitmap conservatively returns affected=true. Metadata/source/reserve discovery happens at startup; root has no protocol-upgrade/source-change refresh trigger here. Only observed feed contracts and periodic sweeps drive runner rechecks; Borrow/Repay/Withdraw/aToken/debt-token/ReserveDataUpdated events do not create urgent per-account jobs in this runner.

## Types, coherence and scheduler limits

* `Health { AccountRef, health_factor:f64, debt_usd:f64, collateral_usd:f64 }` and `liquidatable() = health_factor < 1.0` (`mev-lending/src/lib.rs:29,38`). Aave converts WAD and base-currency U256 into f64 and assumes 8-decimal USD base units (:459–470). Exact WAD eligibility is absent on this trading path; rounding can collapse values near 1e18 into the same f64 (inference from the conversion, not a measured test).
* `AccountRef { market:Address, account:Address, sub_id:u64 }` lacks chain id (`mev-types/src/lib.rs:262`). `Plan` has U256 `flash_amount` and exact protocol calldata/approval amounts, but f64 collateral sizing/bonus and no block/hash/account revision/expiry/calldata hash (:287). `StateVersion` holds shared AtomicU64 revision, AtomicBool confidence, expected revision (:10); it is not a chain snapshot identifier.
* The Aave prediction `Snapshot` is a Mutex containing reserve metadata, cached positions and f64 feed answers (`mev-lending-aave-v3/src/lib.rs:72`). It is not a persisted immutable borrower snapshot or CAS repository. Prediction mutates cached prices (:489). Sweep refresh obtains health, reserve configuration/prices, balances and feed answers via separate requests (:343); no common block reference binds them.
* `mev_rpc::snapshot_block` and hash-pinned `multicall_at` already exist (`mev-rpc/src/lib.rs:98,122`). The lending adapter instead calls `multicall`, which uses `BlockId::latest()` (:114); concurrent chunks and subsequent reads can observe different heights/providers. Normal simulation and gas estimate are also separate unpinned reads.
* Pure integer payout helpers exist (`mev-lending-aave-v3/src/exact.rs:11,22,54`), but the model still chooses pairs/close factor and ranks bonus with floats (`math.rs:74,89,103,118`). Constants assume HF 0.95 and a $2000 small-position close-factor rule (`math.rs:7,9`), without deployment-specific rule validation in root. Exact payout takes a floored float-selected debt target (:147), then output is converted back to f64. Flash borrowing uses exact selected-reserve **full debt** plus 0.2%+1 buffer, while calldata uses `debtToCover=U256::MAX` (`lib.rs:375,409`); the contract decides actual repay eligibility/amount.
* Runner uses 300s full sweeps, 30s fallback polls in checked-in config (`config/arbitrum.toml:48–51`), feed bursts at 400/900/1600ms, at-risk HF=1.05 and $20 debt/collateral filters (`mev-liquidate/src/lib.rs:25,232`; config :48). Full-sweep completeness is checked by result count (:303). Plan cooldown doubles 3s→1800s (:32,381). These are timer/cooldown heuristics, **not P0/P1/P2 queues or per-account deadlines**.
* Runner awaits index/sweep/poll/plan work inside one select-loop (`:492–549`); slow requests/backfill sleeps can delay feed handling. Channels are unbounded; no durable recheck lease/ack, reserved tier quotas, maximum-age breach accounting or simulator budget is attached. Plans are stamped with one global runner version/confidence (:175); invalidation affects all plans, and confidence requires every slot reconciled (:184,194). Root does not bind versions to individual account state or chain hash.

## Simulation and execution safety

* `mev-sim::Simulator` is **RPC eth_call of the deployed executor**, then estimate_gas (`src/lib.rs:22,107`). `SimOutcome` contains only U256 profit + u64 gas, or short revert text (:17). It has no input state id, calldata hash, timestamp/expiry, net value, touched storage or hash-pinned liquidation method.
* Normal `on_plan` simulates with `minProfit=0` (engine :2835), then chooses fees/admission and constructs a different calldata profit floor (:2928,2937). No final-calldata resimulation occurs. Batch flush similarly rebuilds floors and batch calldata without batch simulation (`batch.rs:182,210,236`). Thus an exact executor call exists, but **exact submitted-call validation at one coherent state does not**.
* Backrun explicitly bypasses simulation (`mev-engine/src/lib.rs:2683–2790`) and uses float bonus haircut + assumed gas before fees/guard/minProfit. The existing path must not be described as universally simulator-gated.
* REVM=43 (`mev-localsim/Cargo.toml:17`) currently executes **incoming signed feed TxEnvelope** against StateCache+Pending overlay (`mev-localsim/src/lib.rs:208,213,335`; worker.rs:119,250). Cache misses fall back to RPC and initiate proof/code fills (:408,439; worker :290). Worker forwards simulated **pool-event logs** to presim (:258), not `ExecutableLiquidation`. Mainnet REVM context uses synthetic next-block/current wall time, zero gas price/base fee, disabled nonce/balance/basefee/block-gas checks (:231). No own liquidation request adapter, exact profit/gas result or Arbitrum-specific precompile implementation is wired here. Reuse this crate's state/execution seam after correctness work rather than adding another REVM engine.
* `liq_precheck` checks dependencies in live mode, dust/max bonus, route and funding (engine :2628). Route search uses float expected collateral; fallback can perform remote pool discovery/load in-handler (:1405). Balancer-cap check converts flash amount to f64 (:2661). Aave flash fallback uses configured Pool (:1760).
* Preserved safeguards: current runner dependencies checked pre-simulation in live (:2633); final sender guard checks dependencies, engine confidence/feed epoch, global store revision and kill-file existence (:364). `admit` computes integer conservative gas/tip bound from gas_limit × maxFee using integer native price, then reserves durable liability (`mev-engine/src/accounting.rs:11`; `mev-risk/src/lib.rs:190`; ledger.rs:310). Gross/net strategy estimates still arrive as floats. Final profit floor uses integer price ratio and gas bound+minimum profit (engine :1323).
* Sender requires reservation + final guard (:251), validates fee terms, checks state before signing, before broadcast and after durable attach (:270–335). Same raw transaction broadcasts concurrently to reader and extra endpoints (:338). Ambiguous transport outcomes retain nonce and liability (:367); no second execution is constructed. NonceManager serializes held/free/floor state with Mutex (`mev-exec/src/nonce.rs:19,32,45,63,73`); restart reconciliation/restoration exists (`mev-exec/src/lib.rs:156,161`). No account-specific execution lock, expiry/calldata-digest guard or explicit replacement/cancellation policy is supplied by Plan.
* Contract guard owns repayment/profit truth: operator restriction and route endpoints (`contracts/src/ArbExecutor.sol:255,290`), authenticated single-use callbacks (:350,370), starting balances (:403), allowed protocol target+spender and exact approval reset (:409), then debt balance >= startingDebt + loan + actual premium + minProfit (:361,388). ProfitReceiver receives actual surplus. This protects token profit but does not prove scheduler coverage, same-state simulation, net USD accuracy, or snapshot identity.

## Committed scheduler stack: separate status layer

Stack commits: `91cca68` deployment; `7113cf8` snapshots; `a6fc04b` event indexing; `830a03e` features; `d0f7b52` scheduler. Audit HEAD and stack are divergent; merge-base is `390c29248644836b5ab6b16562476db6ffda552f`. Do not infer these new modules exist at c6f3aa4. Direct HEAD→stack diff leaves the neutral Health/Plan, sender/nonce/risk, contracts and localsim unchanged; engine additions include deployment startup checking and newer arbitrage diagnostics.

| Responsibility | Code present in d0f7b52 | Production integration status from source |
|---|---|---|
| Deployment | **stack:** `mev-lending-aave-v3/src/deployment.rs:120,168` reads chain id, provider-resolved Pool/Oracle, EIP1967 implementation/code hash and compares manifest; numeric block-pinned reads. `config/arbitrum.toml:64` pins addresses/hash/block/source commit. | **stack:** engine `src/lib.rs:3084` invokes verifier at startup; live mismatch fails, shadow warns. No chain observation made in this inventory. Data-provider/Oracle runtime code/ABI hashes and future rule behavior are not validated by this function. |
| Exact aggregate snapshot | **stack:** `mev-liq-state/src/domain.rs:7,38,125,147` adds chain+pool+borrower key, ChainRef, U256 amounts/HF and HF<WAD+debt check. `repo.rs:379,474,550,588` provides SQLite transactional CAS, expiry, canonical checkpoints, reorg invalidation. `mev-lending-aave-v3/src/recheck.rs:34,79` pins getUserAccountData by canonical hash. | Library/helper, not replacement for legacy trading Health. Rechecker leaves per-reserve balances empty (:65); no reserve bitmap/eMode field in BorrowerSnapshot. `git grep` finds rechecker/repository usage in tests, not engine/bot startup. No scheduler→recheck→CAS→Plan worker is wired. |
| Borrower backfill durability | **stack:** `mev-lending-index/src/lib.rs:105,192,211,379,414` adds atomic/fsync JSON save, explicit gaps+retry, gap-aware backfilled status, ring-based reorg rewind (keeps borrower superset). `scanner.rs:52,128` owns shared fetch/reorg logic. | Legacy Aave index uses this upgraded owner automatically. Successful getLogs response completeness remains provider-dependent (`scanner.rs:76`), and reorg-check errors can warn/continue (`lib.rs:398`). No measured full-market completeness here. |
| Rich event dependencies | **stack:** `mev-lending-aave-v3/src/events.rs:20,37,163` decodes Pool supply/withdraw/borrow/repay/liquidation/collateral/eMode/reserve/upgrade and Oracle events. `triggers.rs:182,439` -> forward/gap-heal/backfill -> repository scan effects, bounded fanout, reorg/upgrade/reserve checks, dependency reconciliation (:970) and price polling (:1025). | Adapter initializes `triggers:None` (`lib.rs:198`); attach_triggers is an explicit API (:204). Engine/bot never calls it at this commit. Historical backfill registers Borrow owners only (`triggers.rs:191`); reconcile hash-pins bitmap+aggregate debt to update dependency registry (:978–1008), **not** BorrowerSnapshot CAS. Token Transfer/Mint/Burn subscriptions are absent from event decoder/trigger surface, so full trigger coverage is not established. |
| P0/P1/deadlines | **stack:** `mev-liq-sched/src/policy.rs:71` pure schedule(input now,cfg), dedupe/versions/reasons; eligibility P0, unknown/stale/reorg P1 (:420–525); reserved capacity/fair dispatch (:224), deadline misses and capacity violations (:272). `types.rs:117,138,251` exposes pure inputs/jobs/config; advisory bonus always 0. | Functional policy library, **not active dispatcher**. `mev-liquidate/src/shadow.rs:40` default-disabled ShadowSchedule; :68 spawn_blocking -> `mev-liq-sched/src/shadow.rs:120` load_inputs->schedule->record. Optional runner timer hook exists (`mev-liquidate/src/lib.rs:514,530`), but engine/bot never calls with_shadow_schedule. Report “dispatched” counts proposed jobs; no rechecks are dispatched by shadow pass. Durable lease ownership is caller input (`types.rs:101`); shadow passes empty in_flight/urgent/advisory arrays (`shadow.rs:130`). |
| Liquidation exact-call | **stack:** same legacy liquidation eth_call+estimate_gas; `mev-sim/src/lib.rs:107` adds simulate_fast_at only for **arbitrage diagnostics**, before unchanged simulate_liquidation. | No hash-pinned final-call liquidation REVM pipeline, executable snapshot/call binding or mandatory scheduler enforcement added. Existing float Health/Plan/backrun behavior remains. |

Additional stack caveats: scheduler loader makes several separate repository reads (`mev-liq-sched/src/load.rs:33–65`), not a single read transaction; SourceVersion carries snapshot block/revision/digest but no explicit hash (`types.rs:129`). Policy does not classify Provisional source as P1 by source alone (`policy.rs:435–505`). These are facts to address before interpreting shadow output as canonical trading authorization.

## Extension map and remaining acceptance work

At audit HEAD, missing: immutable exact per-account snapshot/CAS; deployment resolution/pinning; complete event/dependency/upgrade/reorg index; P0/P1 mandatory work with durable deadlines/quotas; coherent block-pinned recheck+position/metadata; exact **final liquidation call** REVM simulation and state-id/call-hash/expiry checks; production scheduler worker and coverage evidence. At stack, substantial snapshot/index/policy libraries already exist, but attach/recheck/CAS/dispatch integration and final-call simulation remain absent. Extend them, do not duplicate.

Primary seams: `LiquidationAdapter` (`mev-lending/src/lib.rs:65`) for verified state/eligibility; existing Aave `recheck` and `triggers` plus `mev-liq-state`/`mev-liq-sched` on stack; `mev_rpc::multicall_at` for coherent reads; `mev-localsim::execute`/state cache and `mev-sim` for simulator convergence; `Plan`/`StateVersion` for immutable input identity; `liquidation_calldata` for one frozen call; `Sender`/risk ledger/profit floor for existing final safety checks.

No tests were executed by this inventory context. Parent reports fresh isolated c6 fmt/clippy/workspace tests exit 0: 564 passed, 0 failed, 24 ignored (153 summaries); that does not establish fork/chain coverage or stack validation. Relevant repeatable offline slice command: `scripts/dev.sh cargo test -q --locked -p mev-lending-index -p mev-lending -p mev-lending-aave-v3 -p mev-liquidate -p mev-sim -p mev-localsim -p mev-exec -p mev-risk`. Aave live tests are `#[ignore = "network"]` (`tests/live.rs:31,90,163`) and were not run; historical eligibility/exact-call/receipt parity and deployment observations need separate authorized evidence.
