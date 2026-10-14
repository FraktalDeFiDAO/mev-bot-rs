# JEV-LIQ-012: async advisor worker (Process B)

Crate `copper-jev-advisor` (`crates/copper-jev-advisor`; the spec's `bins/` maps to `crates/`). Library entry point
`Worker::new(Config, ClientConfig)` + `Worker::run(CancellationToken)`; the binary only parses flags/env and wires
signals. Spec: `docs/copper_jev_liquidation_architecture.md` sections 0 (inv. 1, 6, 7, 8), 2, 6, 11, 12, 14, 15.
It composes existing pieces only: `jev-advisor` (`TypeSafeClient`, `ResilientClient`, `TokioClock`, `SeededRng`),
`mev-liq-state` (`AdvisorStore`, `apply_decision`), `mev-liq-features` (`RedactedFeatures`).

## Loop

1. `pull` export rows from the snapshot DB (read-only `SnapshotReader`) into the advisory outbox, then `claim` up to
   `free = max_inflight - running` rows with a lease of `ADVISOR_TTL_SECONDS`.
2. Per row, in its own task (bounded by `JEV_MAX_INFLIGHT`):
   * Re-validate `feature_json`: parse the flat string object, map every value into the closed `FeatureV1` enums, build
     `RedactedFeatures`, require the re-rendered canonical JSON to equal the stored text byte for byte and the digest to
     equal the row's `feature_digest`. Failure: `dead_letter` (new `AdvisorStore::dead_letter`), never sent.
   * Skip (ack as `superseded`, no request) if the account's current export is no longer `(revision, digest)`.
   * `build_request` then `ResilientClient::call(req, deadline = lease_until - now, cancel)`.
   * `Ok(answer)`: build an `AdvisoryDecision` (`decision_mode = shadow`; `confidence` = min of the watch-tier and
     monitoring-value confidences; `issued_at = now`, `expires_at = now + ADVISOR_TTL_SECONDS`) and `apply_decision`.
     Stored/refreshed: `ack`. Obsolete (Expired, Stale/Future revision, DigestMismatch, NoCurrentExport, UnknownAccount,
     SupersededResponse): `ack` and count `discarded_stale_response`. Configuration-class (Malformed, policy/schema/chain/model
     mismatch): `dead_letter`. Anything else: `fail` (retry with backoff).
   * `Err(Unavailable{reason})` (advisory only; the baseline never waits):

| FallbackReason | Queue action | Worker action |
|---|---|---|
| CircuitOpen, Misconfigured, RequestBudget, TokenBudget, CostBudget, InflightSaturated | `release` (attempt refunded) | pause claiming for `ADVISOR_FALLBACK_PAUSE_MS` |
| DeadlineExceeded, RetriesExhausted, NegativeCached | `fail` (capped exponential backoff, dead at `ADVISOR_MAX_ATTEMPTS`) | none |
| NonRetryable (400/401/403/422, schema drift, wrong model, ...) | `dead_letter` | none |
| Cancelled (shutdown) | `release` | none |

3. A `Misconfigured` breaker (401/403) is sticky by design; the worker does not hot reload, so fix the key and restart.

## Configuration (flags or environment; validated once at startup)

| Env var | Default | Notes |
|---|---|---|
| `TYPESAFE_ENDPOINT` | `https://api.typesafe.ai/v1/systemone` | https only, default port, no credentials, host must be in `TYPESAFE_ALLOWED_HOSTS` |
| `TYPESAFE_ALLOWED_HOSTS` | `api.typesafe.ai` | comma separated |
| `TYPESAFE_API_KEY_FILE` | none | preferred; regular file, mode must have no group/other bits (0600 or 0400) else `KeyFilePermissions` |
| `TYPESAFE_API_KEY` | none | alternative to the file; setting both is an error; at least one is required |
| `JEV_MODEL` | `jev-1.13.0` | must be in `ALLOWED_MODELS` |
| `ALLOWED_MODELS` | `JEV_MODEL` | comma separated; also the response allowlist |
| `JEV_MODE` | `shadow` | `active` is rejected with `ActiveModeNotSupported` (task JEV-LIQ-019); anything else `UnknownMode` |
| `JEV_POLICY_VERSION` | required | must equal the producer's export policy version |
| `ADVISOR_CHAIN_ID` | 42161 | chain the decisions belong to |
| `JEV_MAX_INFLIGHT` | 4 | concurrent requests and leased rows |
| `JEV_TIMEOUT_MS` | 1500 | per-attempt, clamped to the item's remaining TTL |
| `JEV_BUDGET_PER_HOUR` | 200 | HTTP attempts per hour, retries included |
| `JEV_BUDGET_TOKENS_PER_HOUR` | off | optional |
| `JEV_BUDGET_USD_MICRO_PER_HOUR` | off | needs both `JEV_PRICE_*` vars (prices are never assumed) |
| `JEV_PRICE_INPUT_MICRO_USD_PER_MTOK`, `JEV_PRICE_OUTPUT_MICRO_USD_PER_MTOK` | off | both or neither |
| `ADVISOR_DB` | required | advisory SQLite file; Process B is its only writer |
| `SNAPSHOT_DB` | required | Process A's SQLite file, opened read-only; must differ from `ADVISOR_DB` |
| `ADVISOR_TTL_SECONDS` | 60 | lease, decision expiry and answer-cache TTL |
| `ADVISOR_POLL_MS` | 250 | idle poll interval |
| `ADVISOR_MAX_PENDING` | 1000 | cap on pending outbox rows |
| `ADVISOR_MAX_ATTEMPTS` | 5 | per row, then dead-lettered |
| `ADVISOR_RETRY_BASE_MS` / `ADVISOR_RETRY_MAX_MS` | 1000 / 60000 | queue retry backoff |
| `ADVISOR_FALLBACK_PAUSE_MS` | 2000 | claim pause after a systemic fallback |
| `ADVISOR_SHUTDOWN_GRACE_SECONDS` | 10 | in-flight grace after SIGTERM/SIGINT |
| `ADVISOR_METRICS_INTERVAL_SECONDS` | 30 | metrics log line period |
| `RUST_LOG` | `info` | JSON logs on stdout |

`DATABASE_URL` from spec section 12 is replaced by the two SQLite paths (the repo has no Postgres; see 008). Retry,
breaker and cache tuning other than the above keep the `ResilienceConfig` defaults of 010.

## Permissions and isolation (invariant 7)

* No RPC URL, signer, wallet, nonce, executor address or database URL setting exists (`tests/isolation.rs` scans the clap
  argument ids and env names for these).
* Dependency graph: `cargo tree -p copper-jev-advisor -e normal,build` must not reach `mev-exec`, `mev-risk`, `mev-engine`,
  `mev-bot`, `mev-rpc`, `mev-sequencer`, `mev-liquidate`, `mev-liq-sched`, `alloy-signer*`, `alloy-provider`,
  `alloy-rpc-client`, `alloy-transport-*`, `k256`, and no alloy feature containing `signer`/`wallet`/`keystore` may be on
  (`tests/isolation.rs`). A control test proves the detector sees them in `mev-bot`. `cargo tree -p` is used instead of
  `cargo metadata` because metadata unifies features across the whole workspace and would report the workspace-wide
  `signer-local`, which `cargo build -p copper-jev-advisor` does not enable.
* To make that true, `alloy` in `jev-advisor`, `mev-liq-state`, `mev-liq-features` and this crate is now
  `default-features = false, features = ["std"]` (primitives only) instead of the workspace alloy (`essentials`, which includes
  `signer-local`, plus providers). The sources only use `alloy::primitives`, so nothing else changed.
* The snapshot DB handle is `SQLITE_OPEN_READ_ONLY` + `query_only` (008). The only writes are to the advisory DB.
* The container needs: read access to the snapshot DB directory (WAL needs the `-shm` file), write access to the advisory DB
  directory, outbound HTTPS to the allowlisted host, and the key file. Nothing else.

## Secrets and logs

* The key is a `Secret` (Debug redacts, no Display); `RawArgs` carries it as `RawSecret` (redacted, parse infallible so clap
  never echoes it in an error; env values hidden from `--help`). Errors name the key file path, never its content.
* Logs: JSON via `tracing`, fields are queue row id, request id, 16-hex feature-digest prefix, fallback label and error class.
  No borrower/pool address (the AccountKey is omitted entirely), no key, no auth header, no URL. `tests/e2e.rs` captures all
  tracing output at DEBUG across the whole test process and asserts: the key is absent, no `authorization`/`bearer`, and no hex
  run of 40+ characters.
* Metrics line (target `copper_jev_advisor::metrics`, message `metrics`, every `ADVISOR_METRICS_INTERVAL_SECONDS` and at exit):
  Jev counters from `MetricsSnapshot` (`circuit_state`, `requests_total`, `fallback_total`, tokens, estimated cost, cache,
  coalesced, window usage), queue stats (`queue_pending`, `queue_in_flight`, `queue_done`, `queue_dead`, `queue_missed_total`)
  and worker counters (`claimed`, `applied`, `superseded`, `discarded_stale_response`, `dead_*`, `failed_retry`, `released`,
  `lease_lost`). Map-valued counters are compact JSON strings. Task 013 should turn the same `MetricsSnapshot` into the exporter.
* Tracing: each request runs in a root span `jev_request{request_id, feature_digest, item, attempt}` created with
  `parent: None`, so it is a separate trace even when the worker is started under an execution span (tested).

## Lifecycle

* Start: config validated (no network), signal handlers installed first, both databases opened, loop started.
* SIGTERM/SIGINT: cancel token; stop pulling/claiming; wait up to `ADVISOR_SHUTDOWN_GRACE_SECONDS` for in-flight rows; then
  cancel the remaining calls (they return `Cancelled` and `release` their row, refunding the attempt); after 5 more seconds
  abort what is left; `release` any lease still held; log a final metrics line; exit 0.
* Crash (no clean shutdown): leases lapse after `ADVISOR_TTL_SECONDS`; the next process re-claims the rows (attempts count).
  Rows are `ack`ed at most once (008 lease rule), so a restart cannot double-apply.
* Readiness: `copper-jev-advisor check` validates config, opens the snapshot DB read-only and requires schema version
  `mev_liq_state::SCHEMA_VERSION` (4), opens/creates the advisory DB read-write and requires `ADVISOR_SCHEMA_VERSION` (1),
  runs one stats query, prints `{"ready":true,...}` and exits 0; otherwise prints `not ready: ...` and exits 1 (config errors
  exit 2). It never contacts the Jev API.

## Runbook (what exists today)

| Symptom | Meaning | Action |
|---|---|---|
| exit 2, `JEV_MODE=active is not supported` | active mode is gated | set `JEV_MODE=shadow` (promotion is JEV-LIQ-019) |
| exit 2, `permissions 0644 ... chmod 0600` | key file readable by others | `chmod 600` the file |
| `check` says snapshot schema is N | Process A not migrated to 4 | start/migrate Process A first |
| log `fallback=circuit_open` / metrics `circuit_state=2` | Jev failing; breaker open | none needed; baseline continues; recovers by half-open probe after 30 s |
| `circuit_misconfigured=true`, `fallback=misconfigured` | 401/403 from Jev | fix `TYPESAFE_API_KEY(_FILE)`/endpoint and restart |
| `fallback=request_budget` | hourly request budget used | raise `JEV_BUDGET_PER_HOUR` or wait for the window |
| `queue_dead` growing | rows failed validation or hit max attempts | grep logs for `dead-lettered`; reasons are in `advisor_outbox.last_error` |
| `queue_missed_total` growing | export log shed rows before the advisor copied them | the worker is too slow or stopped; check `queue_pending`, raise `JEV_MAX_INFLIGHT` or the producer's `max_export_rows` |
| `lease_lost` > 0 | a row's TTL elapsed while its request ran | normal under load; row is retried |

Smoke: `copper-jev-advisor check`, then `copper-jev-advisor` and watch for the `advisor worker started` and `metrics` lines.

## Tests

`crates/copper-jev-advisor/tests/`: `e2e.rs` (real SQLite files filled through `SnapshotRepository::enqueue_with_snapshot`, a
real local TCP server fixture copied minimally from 009; export -> claim -> HTTP -> shadow decision -> ack; 429s trip the breaker
and recover; unreachable API degrades without crashing; shutdown mid-flight releases leases and a restart finishes each account
exactly once; crash replay after lease expiry; tampered export dead-lettered and never sent; superseded export skipped; readiness;
log hygiene and root spans), `config.rs` (validation, 0644 key rejected, active mode rejected, redaction, and the real binary:
`check`, exit codes, SIGTERM exit 0), `isolation.rs` (dependency graph and config surface). Unit tests: `features.rs`.

## Gaps

* The item TTL is measured from the claim (lease), not from export time: the outbox row's `enqueued_at_ns` is not exposed on
  `Claimed`. A row that sat in the queue is still given a full TTL. Exposing it is a small 008 change.
* A cached or coalesced answer is stamped `issued_at = now` when stored (it may be up to the cache TTL older).
* No config reload; `Misconfigured` needs a restart. No Prometheus exporter yet (013). No retention of done/dead rows (008 gap).
* `alloy` is now pinned lean in four crates; other crates keep the workspace alloy. A new crate that needs signers must not be added
  to this crate's dependency tree (the isolation test will fail).
* Container: see `docs/runbooks/jev-advisor.md` (014). Compose runs it as the host uid (keep-id) rather than 65532, so it can write the bot's data dir.

## What later tasks consume

* 013 (observability): the `metrics` log line fields above, `Worker::jev_metrics()` (`MetricsSnapshot`), `Worker::queue_stats()`,
  `Worker::counters()`; request spans are named `jev_request`.
* 014 (deployment): binary `copper-jev-advisor`, healthcheck `copper-jev-advisor check`, env table above, volumes (advisory DB
  writable; snapshot DB directory readable including `-shm`), key as a `0600` file via `TYPESAFE_API_KEY_FILE`, graceful stop
  within `ADVISOR_SHUTDOWN_GRACE_SECONDS + 5` seconds (set the container stop timeout above that), user `65532`.
* 011/019: decisions are written with `decision_mode = shadow`; reading them is `AdvisorStore::valid_decision_for`.
