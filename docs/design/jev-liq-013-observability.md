# JEV-LIQ-013: metrics, traces, dashboards, runbooks

Spec: `docs/copper_jev_liquidation_architecture.md` sections 0, 11, 12, 14, 15. Runbook:
`docs/runbooks/copper-liq-alerts.md`. Rules: `infra/monitoring/copper-liq.rules.yml`. Dashboard:
`infra/monitoring/copper-liq-dashboard.json`.

## Decision: one metrics system, extended

Inventory (001): no Prometheus existed; `mev-metrics` writes JSONL events plus a heartbeat file. A second metrics stack was
rejected. `mev-metrics` now also has:

* `prom` - a hand-written text exposition renderer (format 0.0.4). `prometheus-client` was not added: the renderer is about 200
  lines, stateless, renders from plain snapshots on scrape, and a crate would bring a second counter/atomic model and a new
  dependency into a signer-free process. No new dependency was added anywhere.
* `registry` - scrape-time registry. `Registry::observe_event(kind, &json)` accumulates the shadow scheduler and advisory events
  the bot already emits (`liq_sched_counters`, `liq_advisory_shadow`); `add_collector` registers a closure that formats a
  prepared snapshot. Series exist only after something measured them; there are no placeholder zeros.
* `server` - optional `GET /metrics` listener on `std::net` (one accept thread, 2 s total deadline per connection, 4 KiB request
  cap, `GET /metrics` only). **Default off. Loopback only** unless an explicit override is given (`parse_listen_addr`; hostnames
  refused).
* `textcheck` - strict 0.0.4 parser used by tests.
* Cargo feature `jsonl` (default on) keeps the event log; `default-features = false` gives the renderer without `mev-types`
  and the workspace alloy, which is required because `copper-jev-advisor` must not link signers (012 isolation test).

Safety rules enforced by the renderer, not by callers: label values over 48 bytes or address-like (`0x...` or a 16+ hex run)
become `other`; at most 64 series per family and 512 KiB per scrape (extras are dropped and counted); invalid metric or label
names drop the sample; label values are escaped. Label sets are fixed enums everywhere (`tier`, `reason`, `result`, `status`,
`state`, `event`); nothing carries a borrower, pool or request key.

## Exporter wiring and defaults

| Process | Enable | Override | Serves |
|---|---|---|---|
| `copper-jev-advisor` | `--metrics-addr ip:port` / `ADVISOR_METRICS_ADDR` | `--metrics-allow-non-loopback` / `ADVISOR_METRICS_ALLOW_NON_LOOPBACK` | Jev client metrics, advisor queue, worker counters |
| bot (`mev-engine`) | `MEV_METRICS_ADDR=ip:port` | `MEV_METRICS_ALLOW_NON_LOOPBACK=1` | scheduler and advisory series fed from `Metrics::event` |

* Off by default in both. Bad address: advisor exits 2 (`configuration error`), bot fails to start with the same message.
* Advisor: a refresher task copies `Worker::jev_metrics()`, `queue_stats()` (on the blocking pool) and `counters()` into a
  `SnapshotCache` every 5 s; the scrape collector only formats the cache. A scrape cannot touch the worker, SQLite or HTTP client.
  Refresh stops with the shutdown token; the listener drops on exit.
* Bot: `Metrics::event` (already called from the off-hot-path shadow pass thread) now also calls `Registry::observe_event`, a
  string match that locks a small mutex only for the two scheduler/advisory kinds. `Metrics::serve_prometheus` starts the listener.
  The only `mev-engine` edit is 4 lines after `Metrics::new` reading the env.
* Inside a container, `127.0.0.1` is the container's own loopback. Task 014 must either keep scraping inside the pod/network
  namespace, or set the override with the bind address `0.0.0.0` inside the container and publish the port only as
  `127.0.0.1:<port>:<port>` on the host. Never publish to a public interface.
* The scheduler shadow pass (`ShadowSchedule`) is disabled by default and not yet wired to engine config, so the bot exporter shows
  no scheduler series until it is enabled and given `metrics.event` as its sink (`with_sink`). That wiring belongs to the task that
  enables the pass.

## Produced vs not produced (spec section 11)

| Spec metric | Status | Source / reason / task |
|---|---|---|
| `copper_liq_recheck_queue_depth{tier}` | produced (bot) | `liq_sched_counters.candidates` p0..p3 of the last pass |
| `copper_liq_recheck_deadline_miss_total{tier,reason}` | produced (bot) | tier `p0`,`p1`,`discretionary` (P2+P3 are aggregated by the shadow counters); reason `backlog` (undispatched) or `late_dispatch`. New fields `deadline_misses_backlog_*` added to `ShadowCounters`. Counts overdue jobs per pass, so a job that stays overdue is counted once per pass |
| `copper_liq_account_last_check_age_seconds{band}` | **not produced** | the shadow counters carry no per-account ages; needs an age histogram computed in `run_shadow_pass` from `BorrowerSnapshot.last_successful_check_ns`. Follow-up to the scheduler (005) or the task that wires the pass |
| `copper_liq_eligible_detected_total{source}` | **not produced** | no event distinguishes detection sources; needs the liquidation runner to label them. Later task (runner wiring) |
| `copper_liq_revm_simulation_duration_seconds{result}` | **not produced** | REVM liquidation simulator lands in JEV-LIQ-015 |
| `copper_liq_revm_revert_total{reason}` | **not produced** | JEV-LIQ-015 |
| `copper_jev_request_duration_seconds{status}` | produced (advisor) | histogram from `MetricsSnapshot.request_duration`, bounds `DURATION_BUCKETS_SECS` |
| `copper_jev_requests_total{result}` | produced (advisor) | `MetricsSnapshot.requests_total` |
| `copper_jev_circuit_state` | produced (advisor) | 0 closed / 1 half-open / 2 open or misconfigured; plus extra `copper_jev_circuit_misconfigured` |
| `copper_jev_fallback_total{reason}` | produced (advisor and bot) | advisor: client reasons (`circuit_open`, budgets, ...); bot: policy verdict reasons, accumulated per pass. Same name, different `job` |
| `copper_jev_stale_response_total` | produced (advisor and bot) | advisor: `discarded_stale_response`; bot: policy stale verdicts accumulated per pass |
| `copper_jev_applied_bonus_total{tier}` | produced (bot) | shadow would-be bonuses accumulated per pass (nothing is dispatched differently) |
| `copper_jev_input_tokens_total` | produced (advisor) | plus extra `copper_jev_output_tokens_total` |
| `copper_jev_estimated_cost_usd_total` | produced (advisor) | `estimated_cost_micro_usd_total / 1e6` |
| `copper_feed_reorg_total` | **not produced** | the engine continuity guard keeps its counters inside `mev-engine`'s hot path; exporting them needs an engine edit outside this task's remit. Needs a small additive hook in the engine (later task) |
| `copper_feed_lag_seconds` | **not produced** | no lag measurement exists; same follow-up |
| `copper_liq_policy_actionable_recall{policy}` | **not produced** | evaluation-only; JEV-LIQ-017 |

Additional series (real, not in section 11, needed by the alerts or the runbook): `copper_liq_coverage_violation_total`,
`copper_liq_sched_passes_total`, `copper_liq_sched_last_pass_timestamp_seconds`, `copper_liq_advisory_passes_total`,
`copper_liq_advisory_mandatory_changed_total` (counts passes with `mandatory_unchanged == false`), and on the advisor
`copper_jev_cache_hits_total`, `copper_jev_coalesced_total`, `copper_jev_inflight`, `copper_jev_window_*_used`,
`copper_jev_advisor_queue_rows{state}`, `copper_jev_advisor_queue_missed_total`, `copper_jev_advisor_queue_stats_ok`,
`copper_jev_advisor_events_total{event}`.

Accumulation: 011 counters are per-pass; the registry adds each event once (scheduler passes are also de-duplicated by `now_ns`,
so a replayed or older pass is ignored). Counters are saturating `u64` and never decrease within a process.

## Alerts

Critical (`page="true"`): `CopperLiqMandatoryDeadlineMiss` (P0/P1), `CopperLiqCoverageViolation`, `CopperLiqSchedulerPassStale`,
`CopperLiqAdvisoryChangedMandatory` (`mandatory_unchanged == false`, must be impossible). Advisory-only (`page="false"`,
`severity="warning"`, `class="advisory-only"`): Jev 429/529/5xx, circuit open, misconfigured (401/403), budget exhaustion,
schema/model mismatch, decision expiry, cost spike, advisor queue backlog, lease/dead rows, queue stats failing, advisor down.

Spec critical alerts with **no rule** because no series is produced: stale state data, indexer gap or reorg mismatch,
snapshot-DB backlog or lease errors (the advisor queue is an advisory DB and is classed advisory-only), transaction simulator
state mismatch, REVM provider inconsistency. They need series from the indexer (004), the snapshot repository (008) and the REVM
simulator (015). The rules file does not contain alerts on series that do not exist.

Validation evidence:

* `scripts/promtool.sh check rules infra/monitoring/copper-liq.rules.yml` runs promtool from the pinned image
  `docker.io/prom/prometheus:v3.5.0` (digest `sha256:8672a850efe2f9874702406c8318704edb363587f8c2ca88586b4c8fdb5cea24`):
  `SUCCESS: 15 rules found`.
* `crates/copper-jev-advisor/tests/observability.rs` parses the rules (no YAML dependency: a small structural reader for the
  single-line `expr`/`labels` layout, which the test also enforces), extracts every selector, and fails unless the metric is
  produced and every `=`/`=~` label matcher can match a value that is actually rendered. Same check for each dashboard query.
  Advisory alerts must have `severity=warning, page=false`; the rest `critical`/`true`; every alert must have a runbook heading.
* The exposition itself is checked with a strict parser (`mev_metrics::textcheck`) and with `promtool check metrics` (see the
  task report).

## Dashboard

One Grafana JSON (schema 39, datasource variable `datasource`) with four rows: Hot-path SLOs, Mandatory coverage, Jev quality and
cost, Replay outcomes. The replay row is a text panel pointing at the 006 report artifacts (`docs/audits/evidence/jev-liq-006/`)
because replay results are offline files, not live series.

## Tracing

* `jev_request{request_id, feature_digest, item, attempt}` is created with `parent: None` (root span, separate trace), verified by
  012's e2e test (`jev_request must be a trace root`) and left unchanged.
* New span `liq_shadow_pass{chain_id, pass_id, block, revision, jobs}` around each shadow scheduler pass
  (`mev-liquidate/src/shadow.rs`, runs on the blocking pool, off the hot path). `pass_id` is the scheduler clock in ns (the job id
  of the pass); `block` and `revision` are the newest snapshot block and highest snapshot revision among the recorded jobs and
  backlog (bounded by `max_records`). No pool or account address is a span field; tested with a capture layer.
* The full spec trace chain (collector -> snapshot -> scheduling -> recheck -> REVM -> executor) is not implemented: the collector,
  REVM and executor stages are not part of this task or are other tasks (015). OpenTelemetry export is not added (no collector
  exists in the deployment); spans are JSON logs.

## Tests

`mev-metrics`: prom unit tests (escaping, caps, histogram shape), registry (accumulation, replay ignore, malformed input),
server (loopback parsing, 404/405/431, stalled client bounded by the deadline, shutdown), `tests/exposition.rs` (strict
parse, exactly the declared families, counters monotonic across scrapes, producer not blocked by a concurrent scraper,
property test: no address-like or unbounded labels, no 32+ hex run, <= 64 series per family). `copper-jev-advisor`
`tests/observability.rs`: complete advisor exposition, property test over random snapshots (valid, bounded, address-free,
counters monotonic), cache writer not blocked by scrapes, alert/dashboard references, real-binary tests (non-loopback refused,
default has no exporter flag, loopback scrape valid, exits 0 on SIGTERM and the listener is gone). `mev-liquidate`: span fields.

## Gaps

* The metrics listed as not produced above; the spec critical alerts without series.
* `copper_liq_recheck_deadline_miss_total` counts per pass (a persistently overdue job grows it every pass); alerts use `increase`.
* The advisor snapshot is up to 5 s old; the bot registry is as fresh as its last event.
* The hot-path bot's own funnel counters (`Counters`) are not exported; JSONL remains their sink.
* Single-threaded listener: a stalled client can delay other scrapes for at most 2 s.
* No TLS or auth on the listener; it is loopback by default and must stay unpublished.

## What task 014 consumes

* Flags/env: `--metrics-addr` / `ADVISOR_METRICS_ADDR` (default off), `ADVISOR_METRICS_ALLOW_NON_LOOPBACK` for a container bind;
  bot `MEV_METRICS_ADDR`, `MEV_METRICS_ALLOW_NON_LOOPBACK`.
* Scrape jobs expected by the rules: `copper-bot` and `copper-jev-advisor` (`up{job="copper-jev-advisor"}`).
* `infra/monitoring/copper-liq.rules.yml`, `infra/monitoring/copper-liq-dashboard.json`, `scripts/promtool.sh`.
* Do not publish the metrics ports publicly; bind published ports to the host loopback only.
