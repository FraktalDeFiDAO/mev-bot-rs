# JEV-LIQ-008: transactional outbox and advisor storage

Code: `crates/mev-liq-state/src/advisor.rs`, migration 4 (`SCHEMA_VERSION = 4`). Spec:
`docs/copper_jev_liquidation_architecture.md` sections 0 (inv. 5, 7, 8), 2, 7, 8, 14, 15.
The repo has no Postgres; this is the SQLite translation of spec section 8
(WAL, `synchronous=FULL`, `schema_migrations` with tested down scripts).

## Isolation design (two files)

SQLite has no roles and cannot make a transaction atomic across two WAL files (ATTACH is atomic
per file only), so "state update + enqueue in one transaction" and "advisor writes only advisory
storage" pull in opposite directions. Chosen split:

| File | Owner (writer) | Tables |
|---|---|---|
| snapshot DB | Process A (`SnapshotRepository`) | snapshots, dirty, ..., **`advisor_export`** (append-only log), **`advisor_current`**, **`advisor_shed`** (migration 4) |
| advisory DB | Process B (`AdvisorStore`) | **`advisor_outbox`** (claimable queue), `advisory_decision`, `advisor_account_high`, `policy_outcome`, `advisor_meta` |

* `enqueue_with_snapshot` runs `upsert_cas_in` (the shared CAS body, no duplicated logic) and the
  export insert in one `BEGIN IMMEDIATE`; any failure rolls both back (tested with real SQLite
  triggers raising mid-transaction).
* The advisor opens the snapshot DB through `SnapshotReader`: `SQLITE_OPEN_READ_ONLY` plus
  `PRAGMA query_only=ON`; INSERT/DELETE/UPDATE/DDL and `PRAGMA query_only=OFF` are tested to fail.
  `AdvisorStore::pull` copies export rows (by id watermark, idempotent on `event_key`) into its
  own `advisor_outbox`; lease/ack/retry state never touches the snapshot DB.
* Spec `advisor_outbox` columns (event_key, account_key, revision, feature_digest, feature_json,
  enqueued_at, available_at, lease_owner, lease_until, attempts, consumed_at) live in the advisory
  `advisor_outbox`, plus `state` (pending|done|dead) and `last_error`. `account_key` is stored as
  chain/pool/borrower columns and stays local (never sent to Jev).

Tradeoffs: one extra copy hop (pull) and two files to back up; in exchange the invariant-7 boundary
is enforced by the OS/SQLite, not by convention. The decision CAS cannot be atomic with the
snapshot read (two files), so it is double-guarded: the read-only check against the current stored
revision/digest, then a per-account revision high-water mark inside the advisory transaction
(`advisor_account_high`). A window where the snapshot advances between the two is closed by
`valid_decision_for`, which re-checks revision and digest at read time.

## Export log and shedding

`event_key = keccak256("mev-liq-state/advisor-event/v1\0" || chain || pool || borrower || revision
|| feature_digest || len(policy_version) || policy_version)`; `UNIQUE`, so repeats dedup
(`EnqueueOutcome.enqueued == false`). `feature_digest`/`feature_json` are
`RedactedFeatures::digest()`/`json()` from JEV-LIQ-007 (this crate cannot depend on the features
crate). Export refuses JSON containing the account's own pool/borrower hex in any case/`0x` form,
non-object or > 16 KiB (defence in depth behind the features crate's redaction).

`advisor_current(chain,pool,borrower) -> (revision, feature_digest)` is upserted in the same
transaction; it is what decisions are compared to, and survives shedding.

Bound `OutboxConfig.max_export_rows`: after the insert the oldest rows over the bound are deleted
and one `advisor_shed(first_id,last_id,count,shed_at_ns)` row is written in the same transaction
(`shed_total()`, `EnqueueOutcome.shed`). The CAS is never refused or shed for queue size; only
optional advisory requests are. Rows shed before the advisor copied them show up as id gaps and are
counted in `QueueStats.missed_total`. Advisor backpressure: `pull` stops at
`QueueConfig.max_pending` pending rows.

## Queue semantics (all times are caller-supplied ns)

* `claim(owner, now, lease_ns, max)`: one `BEGIN IMMEDIATE` transaction, `UPDATE ... WHERE id IN
  (SELECT ... LIMIT n) RETURNING` (SQLite's replacement for `FOR UPDATE SKIP LOCKED`). Claimable:
  `state=pending AND available_at<=now AND (lease_until IS NULL OR lease_until<=now)`. A lease is
  held while `lease_until > now`. Each claim is an attempt. `max_in_flight` caps simultaneous
  leases. An expired lease that already used `max_attempts` is dead-lettered on the next claim.
* `ack` / `fail` / `release` require the owner to hold an unexpired lease on a pending row, else
  `LeaseLost`: a row is acked at most once and a late worker cannot settle a re-claimed row.
* `fail`: attempts < max -> retry at `now + min(base * 2^(attempts-1), cap)`; else `dead`.
  `release` refunds the attempt (graceful shutdown).

## Decisions

`apply_decision(d, now, cfg)` rejects, in order: NaN/inf/out-of-range confidence, prep_probability
([0,1]), value_score ([0,4]), `expires <= issued`, bad model/policy/schema tokens
(`Malformed`); expired (`expires <= now`); issued in the future; policy-version, feature-schema,
chain, model-allow-list mismatch; unknown account; revision != current stored revision (Stale /
Future); no current export for that revision; digest != current export digest; revision below the
advisory high-water mark. Storage is the spec UNIQUE key
`(chain,pool,borrower,revision,feature_digest,policy_version,prompt_digest,model)` with
`ON CONFLICT DO UPDATE` refreshing issued/expires/response/scores; an older `issued_at` than the
stored one is refused (`SupersededResponse`). No API keys or token values are stored (only counts).

`valid_decision_for(snapshot, now, cfg)` returns the freshest stored decision for the snapshot's
revision and current export digest with configured policy/schema, `expires > now` and an allowed
model. It does NOT check mode (shadow|active), `mandatory_tier == P2`, Jev health, answer-schema
or confidence thresholds: those belong to the policy (JEV-LIQ-011).

## Migrations

Snapshot DB migration 4 `advisor_export_log`; down drops `advisor_shed`, `advisor_current`,
`advisor_export` only (v1-3 data survives, tested). Advisory DB has its own `schema_migrations`
(version 1 `advisor_outbox_decisions`, `rollback_to(0)` tested). `SnapshotReader::open` refuses a
snapshot DB below schema 4.

## Known gaps

* `policy_outcome` has insert/read helpers only; the evaluator (016/017) defines what fills it.
* No retention/pruning of done/dead outbox rows, expired decisions or old policy outcomes yet.
* The advisory file is created by Process B; deployment (014) must give it a volume the snapshot
  DB's owner cannot be tricked into reading as authoritative, and give Process B read access to
  the snapshot DB's directory (WAL needs the `-shm` file).

## What later tasks consume

* 010 (retry/breaker/cache): `AdvisorStore::{pull, claim, ack, fail, release, stats}`;
  `Claimed.feature_json` is the body to send; identity of a request is `Claimed.event_key` plus
  the 007 `RequestKey::id()`; cache identity must include `SCHEMA_VERSION` of features and the
  registry version (007).
* 011 (shadow policy): `valid_decision_for` plus the remaining section 7 predicate terms.
* 012 (worker): `AdvisorStore::open(advisor_path, snapshot_path, QueueConfig)`, `apply_decision`
  with a `DecisionConfig`, and `ExportItem` production from the scheduler side via
  `SnapshotRepository::enqueue_with_snapshot`.
