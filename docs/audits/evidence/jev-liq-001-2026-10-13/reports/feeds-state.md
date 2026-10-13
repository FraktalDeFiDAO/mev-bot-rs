# JEV-LIQ-001: feed and state inventory

Read-only audit, 2026-10-13. Checkout: `/tmp/mev-jev-liq-001-audit`, HEAD `c6f3aa466fb1230424a0dfb41b65e09b5084d72e`. All unqualified source line numbers below refer to this baseline. Draft line numbers are explicitly marked `d0f7b52` and obtained with `git show feat/liq-scheduler:<path> | nl -ba`.

## Scope and revisions

- Read `CLAUDE.md`, both Copper Jev documents in full, `docs/PROGRESS.md`, sequencer-first invariants, and the sequencer-first task spec. The specification is a requirement, not evidence of implemented behavior.
- No graph exists in the isolated checkout. Queried the shared root graph with `graphify query --budget 800` for navigation, then verified source in the isolated checkout. Shared graph facts were not treated as baseline facts.
- `main` = `968d580b59d7a63338ee9cdf572a848835d3cad1`; scheduler draft = `d0f7b52ff841e96c9f251bd4cf51469e222d9324`.
- Draft stack: `acbcb41` docs -> `91cca68` deployment -> `7113cf8` snapshots -> `a6fc04b` event index -> `830a03e` features -> `d0f7b52` scheduler. This stack starts from `main`, not `c6f3aa4`. Their merge-base is `390c29248644836b5ab6b16562476db6ffda552f`.
- Baseline vs main: no changes in `mev-ingest`, `mev-sequencer`, `mev-lending-aave-v3`, `mev-lending-index`, `mev-liquidate`, or `mev-state`. Main changes Engine trigger-block diagnostics/allowlist logging, `mev-decode`, Fluid protocol identity and simulation; it also lacks the baseline's Jev docs. Do not transplant baseline Engine line numbers onto main/draft. Main's additional trigger block is diagnostic metadata, not canonical borrower integration.

## Existing production callgraph and delivery channels

```text
mev-bot::run (main.rs:38,67-70)
  -> Engine::bootstrap (mev-engine/src/lib.rs)
     -> build_liquidation (:3029)
        -> AaveV3Adapter::new (:3057-3061)
        -> Runner::new (:3097)
     -> optional Fast Feed -> spawn_fast_feed (:3215)
     -> mev_sequencer::run_tracked (:630)
        both -> raw_tx/raw_rx: Unbounded<SequencedTx> (:619)
        -> one fan-out task (:641-714), RecentHashes dedupe by tx hash
           -> decode_call_with_value/decode_all -> dec_tx/dec_rx (:616,653-704)
           -> pre_tx/pre_rx (:632,708) -> optional localsim worker (:759)
              -> mev_presim::run (:803) -> presim_rx
           -> liq_tx/liq_rx (:631,712)
              -> Runner::run(liq_feed, tx) (:839)
              -> RunnerEvent rx in LiqCtx (:837-840)
  -> Engine::run (:932)
     -> pool_logs -> canonical AMM logs -> ingest_log -> PoolStore::apply_log
     -> presim_rx -> PoolStore::apply_log_tentative (:1183)
     -> dec_rx -> PendingSwaps (:1144 onward)
     -> liq_rx -> on_runner_event (:1197)
```

- The shared feed is created only when `cfg.sequencer_feed` is `Some` (`mev-engine/src/lib.rs:617`). Optional paid Fast Feed is nested under that branch and gated by config plus key availability (`:594-598,622`). No additional collector loop is needed.
- Feed fan-out records arrivals in `FeedLog` and tips before delivery (`:647-650`). It sends to liquidation only when setup exists and to presim only when configured (`:634,707-713`). Most channels are unbounded, so bounded queue/backpressure remains a real gap.
- `SequencedTx` (`crates/mev-sequencer/src/lib.rs:37`) carries sequence, receipt time, hash, destination, calldata and decoded transaction; `block()` (`:53`) derives an Arbitrum One height from the constant at `:49`. It carries no block hash, transaction index, log identity, source/finality tag, market key, or event ID.
- Fast Feed wire DTO retains raw receipt (`crates/mev-ingest/src/fast_feed.rs:46-59`), but `FastFeedTx` (`:83`) and `decode` (`:95-113`) already discard it. `spawn_fast_feed` (`mev-engine/src/lib.rs:3215-3240`) further discards `tx_index`, `pga_round`, and sequencer timestamp, mapping only height into `seq`. The hash-deduped standard-feed copy is then dropped; it cannot promote an earlier fast copy to canonical status through this channel.

## RPC/log ingestion and decoder owners

- Canonical AMM subscription: `crates/mev-ingest/src/lib.rs:24` `pool_logs`, Alloy `Log`, buffer 32,768 (`:15,38-42`). It subscribes to tracked pool/manager addresses and recognized AMM topics. Caller owns resubscribe and resync.
- Engine subscribes before reconciliation (`mev-engine/src/lib.rs:1077-1104`), drains a bounded queued log burst, applies it before evaluating touched pools (`:1108-1143`). `ingest_log` (`:1235`) journals, checks continuity, settles pending tx and updates `PoolStore`.
- Rare-event HTTP ingestion: `mev_ingest::poll_logs` (`crates/mev-ingest/src/lib.rs:49`) starts at current head + 1, slices ranges to 2,000 blocks, sorts by block/log index, retries without advancing on failed RPC (`:58-93`). Cursor is process-local, with no persistent checkpoint, overlap/reorg recovery or historical backfill. Its claim of no skipped failed polls is narrower than restart/reorg completeness.
- Engine builds one rare-event poll for factories and all recognized lender liquidation emitters (`mev-engine/src/lib.rs:985-1049`), plus Fluid DEX swaps. `created_rx` dispatches to `on_watched_log` (`:1210,1975`); lending liquidation logs are recorded as metrics events (`:1983-1998`), not delivered to Runner or used to dirty borrower state.
- Decoder registry: `crates/mev-decode/src/lib.rs:29` `protocols`, `:54` `decode_call`, `:78` `decode_calls`; event registry/classification at `crates/mev-decode/src/events.rs:12,20,37,53`. `pool_event_topics` explicitly excludes lending events and includes only four AMM protocol IDs (`:20-33`).
- Aave protocol decoding is currently liquidation event/calldata only: `crates/mev-protocol-aave-v3/src/lib.rs:16,31-51`. Baseline Aave borrower discovery independently scans `Borrow` through its lending adapter; there is no common Aave Borrow/Repay/Supply/Withdraw/transfer/reserve event envelope in the Engine event loop.
- Existing journal DTO (`crates/mev-engine/src/journal.rs:31,62-77`) includes block number, tx/log indexes, emitter, removed flag and described fields, but no block hash, chain key, source/finality or stable event ID. Extend that provenance or share an envelope at existing dispatch seams; do not equate this journal with a canonical borrower checkpoint store.

## Continuity, reorg and market state

- Standard sequencer `SequenceTracker::observe` (`crates/mev-sequencer/src/lib.rs:238`) accepts monotonically increasing sequence numbers, drops stale copies and counts gaps. `run_tracked` (`:301`) reconnects with backoff, sends a resume header (`:348-356`), and bumps `FeedStats::confidence_epoch` on disconnect and gap (`:315-320,386-390`). Malformed broadcast frames are logged and skipped (`:373-379`); that branch itself does not bump confidence.
- Fast Feed `GapDetector` (`crates/mev-ingest/src/fast_feed.rs:144-171`) observes `(height,index)` and resets after reconnect. `stream_once` (`:266`) records gap/decode/reconnect counters. Fast Feed has no confidence epoch tied to Engine's submission guard, no receipt/canonical tx-count reconciliation, and does not drop `Continuity::Stale` before forwarding (`:325-331`). Hash dedupe helps duplicates, not canonicality.
- Canonical AMM `Continuity::accept` (`crates/mev-engine/src/continuity.rs:25`) rejects removed logs, missing identity, conflicting hash at the same height and reorder; ignores logs already covered by snapshot and last-log duplicates. It accepts later block-number jumps because filters omit unrelated blocks; it does not verify parent-hash ancestry or journal/common-ancestor replay.
- `Engine::reconcile_subscription` (`mev-engine/src/lib.rs:1273`) takes a complete pinned pool/tick snapshot, applies it, emits correction events, sets log floor `(number,hash)`, and restores confidence only if feed epoch stayed unchanged (`:1275-1307`). `sending` (`:1500`) checks that confidence plus feed epoch for arbitrage and liquidation. This is existing protection to preserve, not proof of borrower snapshot correctness.
- `crates/mev-rpc/src/lib.rs:98` `snapshot_block` uses one header's hash/number; `crates/mev-discovery/src/lib.rs:418` `snapshot` uses that hash for pool and tick reads and rejects incomplete reads. These are the existing coherent-read primitives to reuse.
- `PoolStore` (`crates/mev-state/src/lib.rs:52`) owns AMM pools, per-pool versions, shared submission revision, pair/token/V4 indexes and tentative backups. `apply_coherent_refresh` (`:188`) rejects stale refreshes by per-pool version (`:209-212`). `apply_log` (`:641`) confirms tentative state; `apply_log_tentative` (`:651`) retains previous confirmed state; `expire_tentative` (`:667`) can restore it and bump versions. This is an AMM cache; it is not a borrower repository or canonical history journal.
- Market confidence remains global at Engine level, and AMM state has both floating-point and exact representations (`mev-state/src/lib.rs:35-47,76`). There is no implemented full state-status vocabulary on every market snapshot.

## Borrower state and oracle invalidation in baseline

- Current borrower ownership: `AaveV3Adapter` (`crates/mev-lending-aave-v3/src/lib.rs:56`) contains `EventIndex`, reserve list/decimals, aggregator-to-reserve masks/assets, user bitmap cache, and a mutex prediction `Snapshot` (`:72`). No canonical `BorrowerSnapshot`, `AccountKey` chain component or CAS repository exists in this baseline slice.
- `AaveV3Adapter::new` (`:81`) configures only `Borrow` historical/live discovery (`:83-93`); `.index` (`:426`) uses topic 2 `onBehalfOf`. `EventIndex::step` (`crates/mev-lending-index/src/lib.rs:94`) scans forward then backwards under request/throttle budgets; its JSON cache carries block cursors and borrower set (`:42-49`). `save` (`:191`) writes directly, with no block-hash ancestry, reorg invalidation or atomic checkpoint in baseline.
- `Health` (`crates/mev-lending/src/lib.rs:29`) is f64 health/debt/collateral, and `liquidatable` (`:38`) compares HF < 1.0. Aave `health` (`crates/mev-lending-aave-v3/src/lib.rs:445-478`) converts returned U256 values to f64. Its helper `multicall` uses `BlockId::latest()` (`crates/mev-rpc/src/lib.rs:114-120`), so chunks and separate bitmap/position/reserve/oracle calls need not share a height.
- Aave prediction cache `refresh_snapshot` (`:343`) separately obtains reserves, positions and direct aggregator answers; `predict` (`:486`) mutates those cached values on predicted reports. It has no hash, canonical revision, event-time history, invalidation reason set or complete per-borrower snapshot contract. Do not expose it to Jev as verified canonical state.
- Oracle graph owner: `mev_oracle::resolve_aggregators` (`crates/mev-oracle/src/lib.rs:27`) follows proxy/composite getters, with access-list fallback (`:125`) and bounded depth. `latest_answers` (`:104`) omits unreadable answers. `FeedWatcher::matches_tx` (`:218`) unwraps forwarders; `report_answer` (`:174`) decodes OCR1/OCR2 median into f64. These established matching/resolution seams should be reused.
- Aave `feed_masks`/`feed_assets` are initialized at startup (`crates/mev-lending-aave-v3/src/lib.rs:136-164`). User exposure bitmaps are refreshed for near-risk accounts (`:187-212`). `affected` (`:508`) is conservative when no bitmap exists, but a retained old bitmap can omit changed exposure until a later sweep. There is no reverse active-asset borrower index in baseline.
- `Runner` owns adapter-level `at_risk` maps and feed watcher (`crates/mev-liquidate/src/lib.rs:88-120`); `on_feed_tx` (`:404`) increments its revision, predicts backruns and starts affected-feed re-poll bursts. `poll` (`:341`) only scans accounts already in `at_risk`, filtered by `affected`; oracle moves cannot immediately discover an account outside that set.
- `Runner::run` (`:492`) is the existing index/sweep/fallback/burst loop. Full sweep uses all adapter accounts (`:269-291`); fallback uses at-risk accounts. Config currently requests 300-second sweep, 30-second fallback, HF 1.05 band (`config/arbitrum.toml:48-51`). These timers are not durable per-account max-age deadlines: awaited scans/health/plan calls in the same Runner loop can delay subsequent work.
- Runner has shared revision/confidence stamps (`mev-liquidate/src/lib.rs:172-199`), complete-sweep length checks (`:303-314`), and preserves loss of full-sweep confidence across subset polls (`:358-376`). It has no immutable exact snapshot/version per borrower, coherent chain reference per plan, or durable dirty-event replay.

## What the d0f7b52 draft actually adds

All references in this section are `d0f7b52` source, not baseline.

- `crates/mev-liq-state/src/domain.rs:7,38,125`: chain/market/account key, ChainRef and exact U256 BorrowerSnapshot. `SnapshotRepository` is SQLite, with `fetch_by_key` (`repo.rs:371`), transactional `upsert_cas` (`:379`), digest/revision/order checks (`:384-424`), expiry/dirty lookup (`:474,488`), canonical checkpoint/promotion (`:550`) and reorg invalidation (`:588`). These implementations are available to integrate; do not create a second repository.
- `crates/mev-lending-aave-v3/src/recheck.rs:16,34`: `AaveAccountRechecker::recheck` uses EIP-1898 canonical hash-pinned account data. It leaves per-reserve balances empty (`:65`), lacks user reserve bitmap/eMode in this DTO, and `recheck_at_head` returns provisional until checkpoint (`:79-94`). Do not report a complete Aave borrower position snapshot from this method alone.
- `crates/mev-liq-state/src/index.rs:152,197,705`: typed `Effect`, `ScanBatch` and transactional `apply_scan` combine dirty effects, reverse dependencies, registry, cursors, ancestry and gaps. APIs include `asset_borrowers` (`:1009`), `reconcile_borrower` (`:1088`), bounded durable fanout (`:1144`), price storage/marks (`:1206,1228,1270`) and `rewind_scan` (`:778`). No type named `EventEnvelope` was found in the draft crates.
- `crates/mev-lending-aave-v3/src/events.rs` and `triggers.rs:326,439`: Aave event/index implementation handles Pool borrower changes, reserve-rate changes, oracle-source changes and upgrades; `step` owns bounded scan/gap/reorg/metadata/dependency/price work. `reconcile` (`triggers.rs:970`) pins bitmap plus account-data reads to one hash but updates dependency registry/active flag, not account snapshot CAS. `poll_prices` (`:1025`) polls actual oracle asset prices at a pinned block; bounded fanout catches accumulated drift without relying only on oracle logs.
- Documented event gaps remain: aToken/debt-token transfers, PoolConfigurator reserve-risk/config events and eMode-category changes (`docs/design/jev-liq-004-event-index.md:37` onward). Reserve events/max-age sweeps are mitigations, not event-exact completeness.
- `AaveV3Adapter::attach_triggers` (`src/lib.rs:204`) imports the legacy known borrower set and installs optional trigger index. Constructor defaults `triggers: None` (`:198`); `index` runs it only if attached (`:504`). `on_price_update_tx` (`:239`) is an available feed adapter but has no call in Engine or Runner. There is no production constructor call to `attach_triggers`.
- Draft modifies the shared legacy `EventIndex` unconditionally: atomic-write cache, bounded retry/gaps, stored ancestry, `locate_reorg` rewind (`crates/mev-lending-index/src/lib.rs:89,211,244`; `scanner.rs:128`). This hardening is already on the normal legacy adapter call path if the draft is run; distinguish it from the unattached new dependency index.
- Draft startup deployment verification is genuinely wired into `Engine::build_liquidation` (`mev-engine/src/lib.rs:3085`): live fails closed, shadow logs and continues. It is not canonical snapshot hydration.
- `Runner::with_shadow_schedule` (`mev-liquidate/src/lib.rs:215`) plus loop tick (`:530`) can run a pure schedule from the SQLite repository. `ShadowSchedule::new` defaults disabled (`shadow.rs:40-43`), and `spawn_pass` uses a blocking worker (`:68-81`). Engine never calls this attachment. The pass records hypothetical job lists; it does not dispatch the rechecker or replace existing polling.
- Confirmed absence by `git grep` across Engine/bot/Runner/Aave production source: no Engine/bot `SnapshotRepository` construction, `attach_triggers`, `AaveAccountRechecker`, `with_shadow_schedule`, `checkpoint_canonical` or `upsert_cas` call. Thus exact snapshots, canonical promotion and new scheduling remain standalone/optional; tests exercising attachments are not production wiring.

## Integration seams and remaining gaps

1. Extend current feed/log DTO delivery around `Engine::bootstrap` raw fan-out, `spawn_fast_feed`, `ingest_log`, and `on_watched_log` with chain/hash/order/source/event identity. Preserve provisional feed preparation separately from canonical identity; recover discarded fast receipt metadata at its decode boundary. Do not add another sequencer or log collector.
2. Use the existing Aave adapter and its draft optional trigger attachment, established `FeedWatcher`, shared `EventIndex` scanner, and draft repository reverse dependencies. Complete transfer/configuration/source-refresh/upgrade coverage at those owners rather than adding a parallel account cache/index loop.
3. Canonical snapshot integration requires one actual owner to open the repository, connect dirty dependencies to bounded exact rechecks, store through CAS, checkpoint verified canonical hashes, and connect rewind/invalidation to that same repository. Existing AMM subscription reconciliation is reusable infrastructure but does not currently do this for borrowers. Draft trigger dependency reconciliation alone does not hydrate snapshots.
4. Preserve Runner's existing full sweeps and fallback while introducing durable account deadlines/fairness through its existing loop. Its current at-risk filter plus timers do not meet Jev P0/P1/max-age completeness. New policy availability alone does not establish dispatch guarantees.
5. Reorg limitations must remain explicit: baseline AMM log rejection causes resnapshot, not common-ancestor replay; rare polling and JSON borrower cache do not follow reorgs. Draft scan rewind and snapshot invalidation are reusable but unconnected to an Engine canonical borrower lifecycle. No authoritative per-block transaction-count comparison or full ancestry journal was verified.

## Verification and limitations

- Static callgraph verified by scoped symbol searches, enclosing source ranges, exact `git show` comparisons and absent-call searches. No credentials or `.env` files read. No source/git edits, RPC/API requests, simulations, bot runs, tests or subagents performed in this context; only this `/tmp` report was written.
- Parent reported isolated baseline Rust gates exit 0. This context did not run or inspect those test logs and makes no independent test-pass claim.
- Retrieval corrections: graph absent in checkout; shared graph used only for navigation. Initial guessed `mev-lending-aave` and `mev-lending/src/index.rs` paths failed; corrected to actual graph-verified `mev-lending-aave-v3` and `mev-lending-index`. Several early large tool outputs truncated; needed ranges were reread or narrowed. No outstanding command/network blocker.
- High confidence in production wiring and standalone-versus-attached distinctions; medium confidence in full event completeness because this task is a static inventory, not chain/fault/replay verification. No claims made about deployment ABI/source validity, current Arbitrum policy/feed schema, provider capabilities, throughput, deadline feasibility or live readiness beyond what code actually implements.
