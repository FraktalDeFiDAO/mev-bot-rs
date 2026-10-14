# JEV-LIQ-004 event dependency indexing

Output: dirty marks + durable indexes in the `mev-liq-state` SQLite repository, consumed by task 005.
Nothing here ranks, decides or trades. Wiring is additive: `AaveV3Adapter::attach_triggers` (not
called by the engine yet); without it `index()` is unchanged apart from the shared `EventIndex`
hardening (atomic cache write, bounded retry + gaps, reorg rewind).

## ABI evidence

Events are declared in `crates/mev-lending-aave-v3/src/events.rs`. Verified against the pinned Pool
implementation `0xF05Fd3cC911b4c5E36e53c00354F645E22922C9A` (manifest, Aave v3.6) using Sourcify
`GET /server/v2/contract/42161/<impl>?fields=abi` (runtime and creation `match`): Supply, Withdraw,
Borrow, Repay, LiquidationCall, ReserveUsedAsCollateralEnabled/Disabled, UserEModeSet,
ReserveDataUpdated, DeficitCreated (v3.6). The Pool proxy ABI declares `Upgraded(address indexed)`;
the pinned AaveOracle (`0xb56c...C7C7`, Sourcify `exact_match`) declares `AssetSourceUpdated`,
`FallbackOracleUpdated`. Each topic0 is also asserted against a real log from Arbitrum
(`crates/mev-lending-aave-v3/tests/fixtures/arbitrum_events.json`, block + tx per entry).
Library-emitted events not in the implementation ABI (`IsolationModeTotalDebtUpdated`) do not move
health factors and are not subscribed.

## Schema (migration 2 in `mev-liq-state`, rollback = `rollback_to(1)`)

`borrowers` (registry, active flag, deps reconciled/stale, discovery block+hash), `borrower_assets`
(reverse index, kind bits collateral=1 debt=2), `scan_cursor`, `scan_blocks` (ancestry),
`scan_touched` (reorg bookkeeping), `coverage_gaps`, `pool_flags` (`full_reverification`,
`backlog_escalated`), `fanout_queue`, `oracle_prices`; `dirty` gains `block_number`, `block_hash`.
One transaction (`apply_scan`) carries a range's marks, registry/dependency edits, ancestry, cursor
and gap changes.

## Operator notes

- `full_reverification` flag set: Pool implementation differs from the manifest (or `Upgraded` was
  seen). Every active borrower is marked `ReserveConfig`. Re-pin `[aave.manifest]`, then
  `SnapshotRepository::clear_flag`.
- `open_gaps(name)` lists ranges not yet fetched; the indexer retries them every step.
- Live check: `scripts/dev.sh cargo test -p mev-lending-aave-v3 --test triggers_live -- --ignored --nocapture`.

## Not covered (task 005 / later)

- aToken/debt-token `Transfer`/`BalanceTransfer` (collateral moved without a Pool event), PoolConfigurator
  events (LTV/liquidation threshold/bonus, `ReserveInitialized`), eMode category config changes: mitigated
  by `ReserveDataUpdated` for rate/index moves, reserve-list change detection, the upgrade flag, and the
  per-account max-age sweep, but not event-exact.
- The legacy JSON `EventIndex` set is not pruned by the registry's deactivations (sweep unchanged).
