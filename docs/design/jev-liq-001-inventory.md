# JEV-LIQ-001 — Repository inventory and baseline

Date: 2026-10-13. Branch `fix/audit-2026-10-10`, HEAD `c75fda3`. No trading behavior changed.
Method: four read-only subagent inventories (file:line from `rg` and range reads), plus three spot-checks by the coordinator (marked ✔). Items marked *not verified* were not opened.

## 1. Baseline test results (measured)

Run in the rootless-Podman dev container (`scripts/dev.sh`) on the **dirty working tree**: HEAD `c75fda3` plus ~17 modified source files and several untracked crates/tests from the earlier audit-fix session (list in the session scratchpad `baseline-git.txt`). This is not a clean-`main` baseline.

| Check | Command | Result |
|---|---|---|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy -q --workspace --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo test -q --workspace` | exit 0: **564 passed, 0 failed, 24 ignored**, 153 test binaries |
| Stub scan | `scripts/check.sh` grep (`todo!`/`unimplemented!`) | not run separately |

Not run: Foundry tests, `--ignored` network tests (24 ignored plus `scripts/test-integration.sh`), the anvil e2e scripts (`test-e2e*.sh`), `python -m unittest scripts/tests`, container image build. CI (`.github/workflows/ci.yml`) runs fmt, clippy, tests (`--locked`), the Python tests, Foundry, gitleaks and the browser tests. It does not run the e2e scripts or the ignored live tests.

## 2. Owners of each spec responsibility

| Spec responsibility | Existing owner | Contract status |
|---|---|---|
| Sequencer / block / log feed | `mev-sequencer/src/lib.rs` (`run_tracked`:301, `SequencedTx`:37), `mev-ingest/src/fast_feed.rs` (`FastFeedTx`), `mev-ingest/src/lib.rs` (`pool_logs`:30, `poll_logs`:53), merged and deduped in `mev-engine/src/lib.rs`:619-638 | **No `EventEnvelope`.** Three unrelated types (`SequencedTx`, `FastFeedTx`, alloy `Log`). No block hash on either feed. The Fast Feed adapter (`spawn_fast_feed`, engine:3215) drops tx_index/pga_round and the provisional tag. Dedup is by tx hash only. All channels are unbounded mpsc. No block-header subscription. |
| Reorg / gap handling | Sequence gaps: `mev-sequencer` (`SequenceTracker`:218). Logs: `mev-engine/src/continuity.rs`:27 (removed/reordered/conflicting block) with reconcile at engine:1107-1296 | Present for chain logs and for sequence gaps. Fast Feed resets on reconnect with no replay. No sequencer-feed URL failover. WS RPC failover present (`mev-rpc`:66). |
| Aave borrower discovery | `mev-lending-aave-v3/src/lib.rs`:81-96, 426-435 → `mev-lending-index/src/lib.rs`:94-200 (`EventIndex`, `BTreeSet<(B256, Address)>`) | Only the `Borrow` event (`onBehalfOf`). Backfill reaches `from_block=7700000` (`config/arbitrum.toml`:59) by polling. No reorg rewind, no atomic checkpoint write (whole-file `fs::write`), accounts never removed. |
| Address book / deployment | `config/arbitrum.toml`:54-60 `[aave]`, `mev-config/src/lib.rs`:109; duplicate `POOL` const in `mev-protocol-aave-v3/src/lib.rs`:21 | Hardcoded. ✔ **No PoolAddressesProvider anywhere** (rg over crates/config/contracts: 0 hits). No chain-ID check in the adapter, no code-hash pinning. |
| Health / borrower state | `mev-lending/src/lib.rs`:29 `Health{health_factor: f64, debt_usd, collateral_usd}` ✔ f64; `mev-lending-aave-v3/src/lib.rs`:445-470 `health()` | **No `BorrowerSnapshot`.** getUserAccountData is lossy-cast to f64 (clamped at `u128::MAX`); the test is `health_factor < 1.0`, not a U256 comparison with 1e18. No revision, block ref or provenance per account. The only revision is one global counter on the runner (`mev-liquidate`:301). |
| Oracle / reserve caches | Adapter caches `lib.rs`:56-77; `mev-oracle/src/lib.rs` (`resolve_aggregators`:27, `latest_answers`:104 → f64); `FeedWatcher::matches_tx`:218 | Feed→reserve-mask index with per-user mask check. No asset→borrowers reverse map. Prices and reserve params (`math.rs`:12) are f64. |
| Scheduler | `mev-liquidate/src/lib.rs`:492-511 `Runner::run` `select!` over index/sweep/fallback/burst timers; `last_plan` cooldown 3s→1800s (:35-39); feed-burst rechecks at 400/900/1600 ms (:24-29) | **No tiers, no pure `RankingPolicy`, no deadline queue, no per-account last-check timestamp or max age.** Full sweep every `sweep_secs` (300), fallback poll 30 s, top-3 backruns per price update. |
| Simulation | `mev-sim/src/lib.rs`:107 `simulate_liquidation(&Plan,&[Hop],min_profit,aave_pool) -> SimOutcome{Profitable{profit,gas}\|Reverted}` ✔ via `eth_call` + `estimate_gas` | **No REVM in the liquidation path** (REVM 43 is only in `mev-localsim`, arb/presim). No `SimulationRequest`, no state/block id (latest block), no calldata hash. The sim runs with `min_profit = 0` (engine:2835) while the send uses a floored `min_profit`, so simulated calldata ≠ sent calldata. |
| Executor | `mev-engine/src/lib.rs` `on_plan`:2823 → `liq_precheck`:2628 → sim → `risk.check`:2882 → `pga_bid` → `admit` → `final_profit_floor`:2923 → `mev-exec` `Sender::send_liquidation`:188 (reservation + guard required, :259); `NonceManager` `mev-exec/src/nonce.rs`; shadow/live via `wire_executor`:2974, `sending()`:1501 | No dedicated validated `ExecutableLiquidation` type; validation lives in engine handlers. Executor is already guarded and gated and **must stay unchanged**. |
| Persistence | JSONL: `mev-metrics` (events, heartbeat), `mev-engine/src/journal.rs`, `mev-outcomes`, `mev-analytics`; SQLite (`rusqlite`) in `seqscope-store`; durable ledger (audit work) | ✔ **No Postgres, Redis or sqlx anywhere** (manifests, compose, Containerfiles). No outbox. |
| Metrics | `mev-metrics/src/lib.rs` | JSONL + heartbeat file. **No Prometheus endpoint**, so every `copper_*` metric in spec §11 needs a new exporter. |
| Containers | `Containerfile`, `Containerfile.dev` (Arch digest-pinned), `compose.yaml` (single `bot` service), `scripts/run.sh` (`podman-compose`) | Already hardened: keep-id, read_only, cap_drop ALL, no-new-privileges, pids_limit, tmpfs. Env-file credentials (no Podman secrets). `network_mode: host`. No rust-toolchain pin (Arch rolling `rust`; CI uses `stable`). |
| Pins | `alloy 2.5` (workspace), `revm 43` (`mev-localsim` only) | Locked versions not verified: a subagent's `Cargo.lock` grep was denied by the permission system and I did not work around it. |

## 3. Callgraphs

- Feed: `main` → `Engine::run` → `sequencer::run_tracked` + `fast_feed::run`→`spawn_fast_feed` → `raw_tx` → dedup (`RecentHashes`) → `dec_tx`/`liq_tx`/`pre_tx`. Chain side: `pool_logs` → `Continuity::accept` (engine:1244) → `ingest_log` → `on_pool_update`.
- Discovery: `Pool.Borrow` log → `EventIndex::step` → `BTreeSet<Key>` → `adapter.accounts()` → `adapter.health()` (multicall `getUserAccountData`) → `Runner.sweep` → `apply_sweep` → `at_risk`.
- Price trigger: sequencer tx → `FeedWatcher::matches_tx` → `affected()` → `poll` → `adapter.plan()` → `RunnerEvent::Plan` → `on_plan`.
- Execution: `on_plan` → `liq_precheck` → `simulate_liquidation` → `sending()`? shadow: `shadow_candidate` : live: `risk.check` → `pga_bid` → `admit` → `final_profit_floor` → `send_liquidation` → `wait_final` → `TxDone`.

## 4. Gaps and conflicts with the spec

Each of these changes the plan for later tasks.

1. **Spec assumes Postgres; repo has none.** Tasks 008 (outbox), 012 (worker) and the §8 DDL need a decision: add Postgres, or use SQLite/redb with a different lease scheme. The "reuse existing persistence" rule points to SQLite.
2. **Spec assumes REVM simulation of liquidations; the repo uses `eth_call`.** Task 015 and invariant 4 ("REVM simulates exact calldata on a state-consistent snapshot") need either a REVM liquidation path in `mev-localsim` or a block-pinned `eth_call` accepted as equivalent. Spec §13 wants a REVM-vs-`eth_call` comparison.
3. **f64 health factors.** The repo's eligibility test uses f64. Invariant: exact U256. Task 003 must add U256 health alongside f64 and migrate the eligibility test. The f64 values also feed the Jev feature bands, so the bands must come from the U256 path.
4. **No tiers or deadlines.** Task 005 is a real new component, not a refactor. It must wrap `Runner::run` rather than duplicate it.
5. **Sim ≠ sent calldata** (`min_profit = 0` vs floored). Needs a redesign before a calldata hash can be bound to the simulation.
6. **No Prometheus.** Task 013 must add an exporter; none exists to extend.
7. **Borrower index is Borrow-only and non-reorg-safe**, and it is polling rather than subscribing. Task 004 scope includes reorg rewind and atomic checkpoints.
8. **Hardcoded Aave addresses with a duplicate constant.** Task 002 must add provider-based resolution and a pin.

Constraints:
- The working tree is dirty with unrelated audit work. Per-task worktrees should branch from a committed base, and the owner has to decide how to land or park that work first. I have not committed or stashed anything.
- `docs/PROGRESS.md` and the sequencer-first invariants (integer math, chain-state-wins, hot-path purity) bind this work. The Jev design is compatible with them, and the f64 findings above are consistent with the audit's open items.

## 5. Dependency graph and task readiness

Spec §14 order stands. Ready now: **002** (no dependencies beyond 001). 003 and 004 follow 002. Track B (007→009→010→012) cannot start until 003 settles.

## 6. Executable baseline commands

```
scripts/dev.sh cargo fmt --all --check
scripts/dev.sh cargo clippy -q --workspace --all-targets -- -D warnings
scripts/dev.sh cargo test -q --workspace          # 564 pass / 24 ignored
scripts/test-integration.sh                       # live RPC, ignored tests
scripts/test-e2e-liquidation.sh                   # anvil fork: closest existing liquidation e2e
scripts/test-e2e-crash.sh                         # batched liquidations
scripts/dev.sh bash -c 'cd contracts && forge test'
```

Measured baseline metrics for the replay comparison (recall, latency p50/p95/p99, deadline compliance) do not exist yet. The current scheduler records no per-account check timestamps, so those baselines must be created in 005/006/016. They cannot be extracted from existing logs.

## 7. Corrections to the audit itself

- Subagents queried graphify unevenly (A used it, C and D did not, B partly). Findings come from `rg` and range reads.
- Subagent D's test counts are `#[test]` / `#[tokio::test]` occurrences, not run results. Run results are in §1.
- *Not verified:* `mev-state`, the other `mev-lending-*` adapters beyond the shared trait, `seqscope-*` feed consumers, `mev-risk`/`mev-search` internals.
